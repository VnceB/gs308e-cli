use crate::error::{Error, Result};
use crate::model;
use crate::signin::{self, PageKind, Sid};
use reqwest::{
    StatusCode, Url,
    blocking::{Client, RequestBuilder, Response},
    header::{COOKIE, HeaderValue},
};
use scraper::Html;
use std::{
    io::Read,
    time::{Duration, SystemTime},
};
use zeroize::Zeroizing;

const MAX_PAGE: u64 = 1024 * 1024;

#[derive(Clone, Copy)]
pub enum Page {
    Dashboard,
    Settings(crate::settings::Feature),
    PowerSavingEditor,
    SntpParent,
    TimeTab(u8),
    Clock,
    Cable,
    Reboot,
    ConfigFile,
    Firmware,
    Statistics,
    Vlan,
}

impl Page {
    fn path(self) -> &'static str {
        match self {
            Self::Settings(f) => f.get_path(),
            Self::PowerSavingEditor => "/powerSaving.cgi",
            Self::TimeTab(_) => "/sntpSub.cgi",
            Self::Clock => "/sntpSystemTime.cgi",
            Self::Cable => "/cableTester.cgi",
            Self::Reboot => "/rebootSwitch.cgi",
            Self::Firmware => "/firmware.cgi",
            Self::ConfigFile => "/confFile.cgi",
            Self::SntpParent => "/sntpMain.cgi",
            Self::Dashboard => "/dashboard.cgi",
            Self::Statistics => "/portStatistics.cgi",
            Self::Vlan => "/vlan.cgi",
        }
    }
}

pub struct Switch {
    origin: Url,
    client: Client,
    explain_login: bool,
}

struct LoginReply {
    document: Html,
    kind: PageKind,
    sid: Option<SessionCookie>,
    cookie: &'static str,
    status: u16,
}

pub struct Session<'a> {
    switch: &'a Switch,
    sid: SessionCookie,
    binding: std::cell::OnceCell<crate::profile::Binding>,
    identity_form: std::cell::Cell<bool>,
}

struct SessionCookie {
    header: HeaderValue,
    expires_at: Option<SystemTime>,
}

impl SessionCookie {
    fn header(&self) -> Result<HeaderValue> {
        if self.expires_at.is_some_and(|end| SystemTime::now() >= end) {
            return Err(Error::ExpiredCookie);
        }
        Ok(self.header.clone())
    }
}

fn send(request: RequestBuilder, method: &'static str, path: &'static str) -> Result<Response> {
    request.send().map_err(|err| Error::Request {
        method,
        path,
        reason: if err.is_timeout() {
            "request timed out; check reachability or increase --timeout"
        } else if err.is_connect() {
            "connection failed; check the management origin, network and TLS certificate"
        } else {
            "no complete HTTP response was received"
        },
    })
}

fn checked(request: RequestBuilder, method: &'static str, path: &'static str) -> Result<Response> {
    let response = send(request, method, path)?;
    accepted(response.status(), method, path)?;
    Ok(response)
}

fn accepted(status: StatusCode, method: &'static str, path: &'static str) -> Result<()> {
    match status {
        status if status.is_success() => Ok(()),
        status @ (StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) => Err(Error::Denied {
            method,
            path,
            status: status.as_u16(),
        }),
        status if status.is_redirection() => Err(Error::Redirect),
        status => Err(Error::Status {
            method,
            path,
            status: status.as_u16(),
        }),
    }
}

fn html(response: Response) -> Result<Html> {
    let mut buf = Vec::new();
    response
        .take(MAX_PAGE + 1)
        .read_to_end(&mut buf)
        .map_err(|_| Error::Network("failed to read the switch response"))?;
    if buf.len() as u64 > MAX_PAGE {
        return Err(Error::Page("page exceeds one MiB"));
    }
    Ok(Html::parse_document(
        std::str::from_utf8(&buf).map_err(|_| Error::Page("HTML encoding"))?,
    ))
}

fn proof(password: &str, nonce: &str) -> Zeroizing<String> {
    // The management UI interleaves the password and decimal challenge before MD5.
    let mut merged = Zeroizing::new(Vec::with_capacity(password.len() + nonce.len()));
    let mut pw = password.bytes();
    let mut salt = nonce.bytes();
    for _ in 0..password.len().max(nonce.len()) {
        merged.extend(pw.next());
        merged.extend(salt.next());
    }
    Zeroizing::new(format!("{:x}", md5::compute(&*merged)))
}

impl Switch {
    pub fn new(
        address: &str,
        timeout: u64,
        explain_login: bool,
        cacert: Option<&std::path::Path>,
    ) -> Result<Self> {
        let origin_err = || {
            Error::Usage(
                "use a host or http(s):// origin with no credentials, path, query or fragment (e.g. 192.0.2.1)",
            )
        };
        let trimmed = address.trim();
        if trimmed.is_empty() {
            return Err(origin_err());
        }
        let candidate = if trimmed.contains("://") {
            trimmed.to_owned()
        } else {
            format!("http://{trimmed}")
        };
        let origin = Url::parse(&candidate).map_err(|_| origin_err())?;
        if !matches!(origin.scheme(), "http" | "https")
            || !origin.has_host()
            || origin.path() != "/"
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(origin_err());
        }
        let mut builder = Client::builder()
            .http1_only()
            .http1_title_case_headers()
            // The embedded HTTP server can silently close an idle connection between requests.
            .pool_max_idle_per_host(0)
            .default_headers(
                [(
                    reqwest::header::CONNECTION,
                    HeaderValue::from_static("close"),
                )]
                .into_iter()
                .collect(),
            )
            .timeout(Duration::from_secs(timeout))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .user_agent(concat!("gs308e/", env!("CARGO_PKG_VERSION")));
        if let Some(path) = cacert {
            let pem = std::fs::read(path)
                .map_err(|_| Error::Usage("could not read --cacert certificate file"))?;
            let cert = reqwest::Certificate::from_pem(&pem)
                .map_err(|_| Error::Usage("invalid PEM certificate in --cacert"))?;
            builder = builder.add_root_certificate(cert);
        }
        let client = builder
            .build()
            .map_err(|_| Error::Network("could not initialize the HTTP client"))?;
        Ok(Self {
            origin,
            client,
            explain_login,
        })
    }

    fn url(&self, path: &'static str) -> Url {
        let mut url = self.origin.clone();
        url.set_path(path);
        url
    }

    fn login_exchange(&self, request: RequestBuilder, method: &'static str) -> Result<LoginReply> {
        let response = send(request, method, "/login.cgi").inspect_err(|_| {
            signin::explain(
                self.explain_login,
                method,
                None,
                "transport-error",
                "unobserved",
            );
        })?;
        let status = response.status();
        let sid = Sid::inspect(response.headers(), &self.origin);
        let cookie = sid.label();
        let needs_index = sid.needs_index_reply();
        if let Err(err) = accepted(status, method, "/login.cgi") {
            let page = if status.is_redirection() {
                "http-redirect"
            } else if matches!(err, Error::Denied { .. }) {
                "http-denial"
            } else {
                "http-error"
            };
            signin::explain(
                self.explain_login,
                method,
                Some(status.as_u16()),
                page,
                cookie,
            );
            return Err(err);
        }
        // Only a SID issued by the credential POST is eligible for this session.
        let sid = match (method, sid) {
            ("POST", Sid::Present { value, expires_at }) => {
                let mut header = HeaderValue::from_str(&format!("SID={value}"))
                    .map_err(|_| Error::Page("session cookie encoding"))?;
                header.set_sensitive(true);
                Some(SessionCookie { header, expires_at })
            }
            _ => None,
        };
        let document = match html(response) {
            Ok(document) => document,
            Err(err) => {
                signin::explain(
                    self.explain_login,
                    method,
                    Some(status.as_u16()),
                    "unreadable-body",
                    cookie,
                );
                if let Some(sid) = sid.filter(|_| !needs_index) {
                    let session = Session {
                        switch: self,
                        sid,
                        binding: Default::default(),
                        identity_form: Default::default(),
                    };
                    if let Err(reason) = session.logout() {
                        eprintln!("warning: could not confirm logout ({reason})");
                    }
                }
                return Err(err);
            }
        };
        let kind = signin::classify(&document);
        let confirmed_index =
            method == "POST" && status == StatusCode::OK && kind == PageKind::IndexNavigation;
        let sid = sid.filter(|_| !needs_index || confirmed_index);
        let cookie = if needs_index && sid.is_some() {
            "usable-backslash-sid"
        } else {
            cookie
        };
        signin::explain(
            self.explain_login,
            method,
            Some(status.as_u16()),
            kind.label(),
            cookie,
        );
        let rejection = match kind {
            PageKind::SessionLimit => Some(
                "the switch has reached its HTTP session limit; allow an idle session to expire",
            ),
            PageKind::Locked => Some(
                "the switch reports a temporary account lock; wait for the displayed lock period to end",
            ),
            PageKind::InvalidPassword if method == "POST" => {
                Some("the switch explicitly rejected the submitted password")
            }
            PageKind::ConflictingNotices => {
                Some("the switch returned conflicting sign-in notices; inspect the normal UI")
            }
            PageKind::OtherNotice if method == "POST" => {
                Some("the switch returned an unrecognized sign-in notice; inspect the normal UI")
            }
            _ => None,
        };
        if let Some(reason) = rejection {
            return Err(Error::LoginNotice { method, reason });
        }
        Ok(LoginReply {
            document,
            kind,
            sid,
            cookie,
            status: status.as_u16(),
        })
    }

    pub fn login(&self, password: &str) -> Result<Session<'_>> {
        let url = self.url("/login.cgi");
        let form = self.login_exchange(self.client.get(url.clone()), "GET")?;
        let nonce = model::challenge(&form.document)?;
        let digest = proof(password, &nonce);
        let reply = self.login_exchange(
            self.client.post(url).form(&[("password", digest.as_str())]),
            "POST",
        )?;
        let sid = reply.sid.ok_or(Error::NoSession {
            status: reply.status,
            cookie: reply.cookie,
            reply: match reply.kind {
                PageKind::Form | PageKind::LoginNavigation => "received the login page",
                PageKind::IndexNavigation => "received the vendor index-navigation page",
                PageKind::Dashboard => "received a model dashboard",
                _ => "received a response",
            },
        })?;
        Ok(Session {
            switch: self,
            sid,
            binding: Default::default(),
            identity_form: Default::default(),
        })
    }
}

impl Session<'_> {
    pub fn binding(&self) -> crate::profile::Binding {
        self.binding.get().copied().unwrap_or_default()
    }

    pub fn ensure_bound(&self) -> Result<()> {
        if self.binding.get().is_none() {
            self.dashboard()?;
        }
        Ok(())
    }

    pub fn require_write(&self, identity_operation: bool) -> Result<()> {
        self.ensure_bound()?;
        self.binding()
            .require_write(identity_operation && self.identity_form.get())
    }

    pub fn dashboard(&self) -> Result<Html> {
        let dashboard = self.read_page(Page::Dashboard)?;
        model::check_model(&dashboard)?;
        let observed = crate::profile::Binding::detect(&dashboard);
        if !observed.supported() {
            return Err(Error::Firmware(observed.version()));
        }
        if *self.binding.get_or_init(|| observed) != observed {
            return Err(Error::Page(
                "dashboard firmware changed during this session",
            ));
        }
        self.identity_form
            .set(observed.newer() && crate::profile::identity_form(&dashboard));
        Ok(dashboard)
    }

    pub fn read(&self, page: Page) -> Result<Html> {
        if matches!(page, Page::Dashboard) {
            self.dashboard()
        } else {
            self.read_page(page)
        }
    }

    fn read_page(&self, page: Page) -> Result<Html> {
        let mut url = self.switch.url(page.path());
        if matches!(page, Page::Settings(crate::settings::Feature::SntpClient)) {
            url.set_query(Some("TAB_NUM=2"));
        }
        if let Page::TimeTab(n) = page {
            url.query_pairs_mut().append_pair("TAB_NUM", &n.to_string());
        }
        let request = self
            .switch
            .client
            .get(url)
            .header(COOKIE, self.sid.header()?);
        let doc = html(checked(request, "GET", page.path())?)?;
        if model::is_login(&doc) {
            return Err(Error::LoginPage { path: page.path() });
        }
        Ok(doc)
    }

    pub fn upload_firmware(&self, content_type: &str, data: Vec<u8>) -> Result<()> {
        self.require_write(false)?;
        let mut url = self.switch.url("/firmware.cgi");
        url.set_query(Some("cmd=fw_upgrade"));
        let response = checked(
            self.switch
                .client
                .post(url)
                .header(COOKIE, self.sid.header()?)
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(data),
            "POST",
            "/firmware.cgi",
        )
        .map_err(|e| match e {
            Error::Denied { .. } | Error::Auth | Error::Redirect => e,
            _ => Error::Unverified,
        })?;
        let page = html(response).map_err(|_| Error::Unverified)?;
        let text = page.root_element().text().collect::<String>();
        if model::is_login(&page) || text.trim() == "CHECK HASH FAILED" {
            return Err(Error::Auth);
        }
        if text.trim() != "SUCCESS" {
            return Err(Error::Unverified);
        }
        Ok(())
    }
    pub fn backup(&self) -> Result<Zeroizing<Vec<u8>>> {
        let mut url = self.switch.url("/confFile.cgi");
        url.set_query(Some("cmd=backup_conf"));
        let reply = checked(
            self.switch
                .client
                .get(url)
                .header(COOKIE, self.sid.header()?),
            "GET",
            "/confFile.cgi",
        )?;
        let disposition = reply
            .headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !disposition
            .split(';')
            .next()
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("attachment"))
        {
            return Err(Error::Page("expected an attached configuration file"));
        }
        let mut bytes = Zeroizing::new(Vec::new());
        reply
            .take(MAX_PAGE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Network("incomplete configuration download"))?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_PAGE {
            return Err(Error::Page("unexpected configuration file size"));
        }
        Ok(bytes)
    }

    pub fn submit(&self, path: &'static str, fields: &[(String, String)]) -> Result<()> {
        self.require_write(path == "/switch_name.cgi")?;
        // Callers supply fixed paths from model-specific handlers. No user-provided URL.
        let response = checked(
            self.switch
                .client
                .post(self.switch.url(path))
                .header(COOKIE, self.sid.header()?)
                .form(fields),
            "POST",
            path,
        )
        .map_err(|err| match err {
            Error::Denied { .. } | Error::Auth | Error::Redirect => err,
            _ => Error::Unverified,
        })?;
        let page = html(response).map_err(|_| Error::Unverified)?;
        if model::is_login(&page) {
            return Err(Error::Auth);
        }
        let reply = page.root_element().text().collect::<String>();
        if reply.trim() == "CHECK HASH FAILED" {
            return Err(Error::Auth);
        }
        let vlan = matches!(
            path,
            "/vlanMod.cgi"
                | "/portBasedBasic.cgi"
                | "/portBasedAdvanced.cgi"
                | "/8021qBasic.cgi"
                | "/8021qAdvanced.cgi"
                | "/portPVID.cgi"
                | "/OUITable.cgi"
        );
        let acknowledgement = reply.trim() == "SUCCESS"
            || vlan
                && page
                    .select(&crate::admin::css("body > div"))
                    .next()
                    .is_some();
        if !acknowledgement && path != "/portStatistics.cgi" {
            return Err(Error::Unverified);
        }
        Ok(())
    }

    // The V1.0.1.13 UI posts without form data and navigates on completion.
    fn post_logout(&self) -> Result<u16> {
        let request = self
            .switch
            .client
            .post(self.switch.url("/logout.cgi"))
            .header(COOKIE, self.sid.header()?)
            .header(reqwest::header::CONTENT_LENGTH, 0)
            .body("");
        checked(request, "POST", "/logout.cgi").map(|r| r.status().as_u16())
    }

    // A successful logout response acknowledges the request. Only loss of access to a
    // previously protected page is evidence about the old session.
    fn verify_logout(&self) -> Result<&'static str> {
        match self.read_page(Page::Dashboard) {
            Err(Error::LoginPage { .. }) => Ok("dashboard returned the login page"),
            Err(Error::Denied { status: 401, .. }) => {
                Ok("dashboard denied the previous session with HTTP 401")
            }
            Err(Error::Denied { status: 403, .. }) => {
                Ok("dashboard denied the previous session with HTTP 403")
            }
            Ok(page) if model::check_model(&page).is_ok() => Err(Error::Logout(
                "the previous session still opened the dashboard",
            )),
            Ok(_) => Err(Error::Logout(
                "unrecognized dashboard response after logout",
            )),
            Err(error) => Err(error),
        }
    }

    pub fn logout_and_verify(self) -> (Result<u16>, Result<&'static str>) {
        let request = self.post_logout();
        let verification = self.verify_logout();
        (request, verification)
    }

    pub fn logout(self) -> Result<()> {
        match self.post_logout() {
            Ok(_) | Err(Error::Denied { .. }) => Ok(()),
            // Some firmware closes the socket while ending the session.
            Err(Error::Request { .. }) => self.verify_logout().map(|_| ()),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_cookie_is_withheld_at_request_time() {
        let issued = SessionCookie {
            header: HeaderValue::from_static("SID=synthetic"),
            expires_at: Some(SystemTime::UNIX_EPOCH),
        };
        assert!(matches!(issued.header(), Err(Error::ExpiredCookie)));
    }
}
