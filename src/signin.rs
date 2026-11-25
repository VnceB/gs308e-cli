//! Structural evidence from the normal management login page. Keep all diagnostic
//! labels fixed: a switch can include deployment data in any header or HTML node.
use crate::admin::{body, css};
use crate::model;
use reqwest::{
    Url,
    header::{HeaderMap, SET_COOKIE},
};
use scraper::{ElementRef, Html};
use std::time::{Duration, SystemTime};

pub enum Sid<'a> {
    NoSetCookie,
    OtherCookies,
    Rejected(&'static str),
    Present {
        value: &'a str,
        expires_at: Option<SystemTime>,
    },
}

impl<'a> Sid<'a> {
    pub fn inspect(headers: &'a HeaderMap, origin: &Url) -> Self {
        let mut state = Self::NoSetCookie;
        for header in headers.get_all(SET_COOKIE) {
            let raw = header.as_bytes();
            // Some senders incorrectly combine Set-Cookie fields. Never select
            // one SID when another assignment is visible after a delimiter.
            if raw
                .split(|&b| matches!(b, b',' | b';'))
                .skip(1)
                .any(|part| {
                    part.trim_ascii_start()
                        .strip_prefix(b"SID")
                        .is_some_and(|tail| tail.trim_ascii_start().starts_with(b"="))
                })
            {
                return Self::Rejected("multiple-sids");
            }
            let pair = raw.split(|&b| b == b';').next().unwrap_or_default();
            let mut parts = pair.splitn(2, |&b| b == b'=');
            let name = parts.next().unwrap_or_default().trim_ascii();
            if name != b"SID" {
                if matches!(state, Self::NoSetCookie) {
                    state = Self::OtherCookies;
                }
                continue;
            }
            if !matches!(state, Self::NoSetCookie | Self::OtherCookies) {
                return Self::Rejected("multiple-sids");
            }
            state = if !raw
                .iter()
                .all(|b| b.is_ascii_graphic() || matches!(b, b' ' | b'\t'))
            {
                Self::Rejected("non-text-sid-header")
            } else if let Some(value) = parts.next() {
                // RFC 6265 §5.2 trims surrounding whitespace. Preserve the value's
                // representation (including paired quotes) for the outgoing Cookie.
                let field = std::str::from_utf8(raw).expect("checked ASCII header");
                let value = std::str::from_utf8(value.trim_ascii()).expect("checked ASCII value");
                match parse_sid(field, value, origin, SystemTime::now()) {
                    Ok(expires_at) => Self::Present { value, expires_at },
                    Err(label) => Self::Rejected(label),
                }
            } else {
                Self::Rejected("sid-missing-equals")
            };
        }
        state
    }

    pub fn needs_index_reply(&self) -> bool {
        matches!(self, Self::Present { value, .. } if value.contains('\\'))
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::NoSetCookie => "no-set-cookie",
            Self::OtherCookies => "no-sid",
            Self::Rejected(label) => label,
            _ if self.needs_index_reply() => "sid-backslash",
            Self::Present { value, .. } if value.starts_with('"') => "usable-quoted-sid",
            Self::Present { .. } => "usable-sid",
        }
    }
}

// This client uses root-level management routes on one exact origin. Refuse
// issuance when a cookie's scope or lifetime cannot be honored there.
fn parse_sid(
    field: &str,
    value: &str,
    origin: &Url,
    now: SystemTime,
) -> Result<Option<SystemTime>, &'static str> {
    let mut max_age = None;
    let mut expires = None;
    for attr in field.split(';').skip(1) {
        let (name, attr_value) = attr.trim().split_once('=').unwrap_or((attr.trim(), ""));
        let attr_value = attr_value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "max-age" if max_age.is_some() => return Err("sid-invalid-lifetime"),
            "expires" if expires.is_some() => return Err("sid-invalid-lifetime"),
            "max-age" => max_age = Some(attr_value),
            "expires" => expires = Some(attr_value),
            "secure" if origin.scheme() != "https" => return Err("sid-scope-mismatch"),
            "path" if attr_value != "/" => return Err("sid-scope-mismatch"),
            "domain"
                if !origin.host_str().is_some_and(|host| {
                    host.eq_ignore_ascii_case(attr_value.strip_prefix('.').unwrap_or(attr_value))
                }) =>
            {
                return Err("sid-scope-mismatch");
            }
            _ => {}
        }
    }
    let expires_at = if let Some(age) = max_age {
        let digits = age.strip_prefix('-').unwrap_or(age);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err("sid-invalid-lifetime");
        }
        if age.starts_with('-') || digits.bytes().all(|b| b == b'0') {
            return Err("deleted-sid");
        }
        let seconds = digits.parse().map_err(|_| "sid-invalid-lifetime")?;
        Some(
            now.checked_add(Duration::from_secs(seconds))
                .ok_or("sid-invalid-lifetime")?,
        )
    } else if let Some(date) = expires {
        let end = httpdate::parse_http_date(date).map_err(|_| "sid-invalid-lifetime")?;
        if end <= now {
            return Err("expired-sid");
        }
        Some(end)
    } else {
        None
    };
    let octets = if value.starts_with('"') || value.ends_with('"') {
        value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or("sid-invalid-quotes")?
    } else {
        value
    };
    if octets.is_empty() {
        return Err("empty-sid");
    }
    if octets.contains('"') {
        return Err("sid-invalid-quotes");
    }
    if octets.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err("sid-whitespace");
    }
    // RFC 6265 §5.2 receives the value verbatim. Add only the backslash
    // case verified against an independent jar; the credential POST must
    // also return the known index page before this candidate can be used.
    if octets.contains(',') {
        return Err(if octets.contains('\\') {
            "sid-comma-and-backslash"
        } else {
            "sid-comma"
        });
    }
    if !octets
        .bytes()
        .all(|b| matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x7e))
    {
        return Err("sid-invalid-octet");
    }
    Ok(expires_at)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    Form,
    LoginNavigation,
    InvalidPassword,
    SessionLimit,
    Locked,
    ConflictingNotices,
    OtherNotice,
    IndexNavigation,
    Dashboard,
    Empty,
    OtherScript,
    OtherForm,
    Other,
}

impl PageKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Form => "login-form",
            Self::LoginNavigation => "login-navigation",
            Self::InvalidPassword => "invalid-password",
            Self::SessionLimit => "session-limit",
            Self::Locked => "temporary-lock",
            Self::ConflictingNotices => "conflicting-notices",
            Self::OtherNotice => "unrecognized-notice",
            Self::IndexNavigation => "index-navigation",
            Self::Dashboard => "model-dashboard",
            Self::Empty => "empty-body",
            Self::OtherScript => "other-script-document",
            Self::OtherForm => "other-form",
            Self::Other => "other-document",
        }
    }
}

fn notice(text: &str) -> Option<PageKind> {
    match text {
        "Invalid Password" => Some(PageKind::InvalidPassword),
        "The maximum number of sessions has been reached. Wait a few minutes and then try again."
        | "Maximum sessions reached. Please wait." => Some(PageKind::SessionLimit),
        _ => {
            let minutes = text
                .strip_prefix("Your account is temporarily locked. Try again in ")?
                .strip_suffix(" minutes.")?
                .trim_matches('@');
            (!minutes.is_empty() && minutes.bytes().all(|b| b.is_ascii_digit()))
                .then_some(PageKind::Locked)
        }
    }
}

pub fn classify(doc: &Html) -> PageKind {
    let mut found = None;
    for node in doc.select(&css(".pwdErrStyle, p")) {
        let inert = std::iter::once(node)
            .chain(node.ancestors().filter_map(ElementRef::wrap))
            .any(|el| {
                matches!(el.value().name(), "template" | "noscript")
                    || el.attr("hidden").is_some()
                    || el.attr("aria-hidden") == Some("true")
            });
        if inert {
            continue;
        }
        let text = body(node);
        if text.is_empty() {
            continue;
        }
        let class = if node.value().classes().any(|c| c == "pwdErrStyle") {
            Some(notice(&text).unwrap_or(PageKind::OtherNotice))
        } else {
            // Some session-limit pages use a standalone paragraph instead of the form notice.
            notice(&text).filter(|kind| *kind == PageKind::SessionLimit)
        };
        if let Some(class) = class {
            if found.is_some_and(|prior| prior != class) {
                return PageKind::ConflictingNotices;
            }
            found = Some(class);
        }
    }
    if let Some(class) = found {
        return class;
    }
    // A standalone vendor message can also be returned without its normal container.
    if doc
        .select(&css(
            "form, script, style, template, noscript, [hidden], [aria-hidden=true]",
        ))
        .next()
        .is_none()
        && let Some(message) = doc
            .select(&css("body"))
            .next()
            .and_then(|el| notice(&body(el)))
    {
        return message;
    }
    if doc.select(&css("form[name=login]")).next().is_some() {
        return PageKind::Form;
    }
    if model::is_login(doc) {
        return PageKind::LoginNavigation;
    }
    if model::check_model(doc).is_ok() {
        return PageKind::Dashboard;
    }
    let script_nodes = css("script");
    let mut scripts = doc.select(&script_nodes);
    if let Some(script) = scripts.next() {
        // Match only the vendor's literal local landing-page script. Never execute it.
        let source = body(script);
        let compact: String = source.split_ascii_whitespace().collect();
        if scripts.next().is_none()
            && script.attr("src").is_none()
            && script
                .attr("type")
                .is_none_or(|kind| kind.eq_ignore_ascii_case("text/javascript"))
            && doc
                .select(&css(
                    "*:not(html):not(head):not(body):not(meta):not(script)",
                ))
                .next()
                .is_none()
            && source
                .split('"')
                .skip(1)
                .step_by(2)
                .eq(["index.cgi", "/", "/index.cgi"])
            && compact
                == r#"varurl=document.location.href;if(url.indexOf("index.cgi")==-1){document.location.href=url.substring(0,url.lastIndexOf("/"))+"/index.cgi";}"#
        {
            return PageKind::IndexNavigation;
        }
        return PageKind::OtherScript;
    }
    if doc.select(&css("form")).next().is_some() {
        return PageKind::OtherForm;
    }
    if doc
        .select(&css("body"))
        .next()
        .is_some_and(|el| el.child_elements().next().is_none() && body(el).is_empty())
    {
        return PageKind::Empty;
    }
    PageKind::Other
}

pub fn explain(
    enabled: bool,
    method: &'static str,
    status: Option<u16>,
    page: &'static str,
    cookie: &'static str,
) {
    if enabled {
        let http = status.map_or_else(|| "none".to_owned(), |s| s.to_string());
        eprintln!("login-check method={method} http={http} page={page} cookie={cookie}");
    }
}
