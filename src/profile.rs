use crate::admin::{body, css, one};
use crate::error::{Error, Result};
use scraper::Html;
use serde::Serialize;

pub type Version = [u16; 4];
pub const REVIEWED: Version = [1, 0, 1, 13];

pub fn parse_version(raw: &str) -> Option<Version> {
    let mut parts = raw.strip_prefix('V').unwrap_or(raw).split('.');
    let mut version = [0; 4];
    for word in &mut version {
        let field = parts.next()?;
        if field.is_empty() || !field.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *word = field.parse().ok()?;
    }
    parts.next().is_none().then_some(version)
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Binding(Option<Version>);

impl Binding {
    pub fn detect(page: &Html) -> Self {
        let cells = css("#sysinfoContainer .hid_info_cell");
        let mut fields = page.select(&cells).filter_map(|row| {
            let mut children = row.child_elements();
            (body(children.next()?) == "ml089").then(|| children.next().map(body))
        });
        let version = fields.next().flatten().and_then(|raw| parse_version(&raw));
        Self(if fields.next().is_none() {
            version
        } else {
            None
        })
    }

    pub fn reviewed_ui(self) -> bool {
        self.0 == Some(REVIEWED)
    }

    pub fn newer(self) -> bool {
        self.0.is_some_and(|version| version > REVIEWED)
    }

    pub fn supported(self) -> bool {
        self.0.is_some_and(|version| version >= REVIEWED)
    }

    pub fn version(self) -> Option<String> {
        self.0.map(|[a, b, c, d]| format!("V{a}.{b}.{c}.{d}"))
    }

    fn write_policy(self) -> &'static str {
        match self.0 {
            Some(REVIEWED) => "reviewed operation handlers and their preflight checks",
            Some(version) if version > REVIEWED => {
                "only operations with a matched device form are eligible"
            }
            _ => "firmware V1.0.1.13 or newer is required",
        }
    }

    pub fn require_write(self, recognized_newer_operation: bool) -> Result<()> {
        if self.reviewed_ui() || (self.newer() && recognized_newer_operation) {
            return Ok(());
        }
        if self.newer() {
            return Err(Error::Usage(
                "no verified device form for this operation on the newer firmware",
            ));
        }
        Err(Error::Usage(self.write_policy()))
    }

    pub fn report(self) -> FirmwareReport {
        FirmwareReport {
            model: "GS308Ev4",
            version: self.version(),
            read_profile: match self.0 {
                Some(REVIEWED) => "reviewed-ui",
                _ => "unverified",
            },
            write_policy: self.write_policy(),
        }
    }
}

#[derive(Serialize)]
pub struct FirmwareReport {
    pub model: &'static str,
    pub version: Option<String>,
    pub read_profile: &'static str,
    pub write_policy: &'static str,
}

// The V1.0.1.13 identity form declares its local POST destination and the
// complete set of named controls. Limit forward compatibility to this shape.
pub fn identity_form(page: &Html) -> bool {
    let root = page.root_element();
    let Ok(form) = one(root, "#sysinfoContainer form[name=sysInfo]") else {
        return false;
    };
    let names = form.select(&css("[name]")).count();
    let exact_field = |query| one(form, query).is_ok();
    form.attr("method")
        .is_some_and(|m| m.eq_ignore_ascii_case("post"))
        && matches!(
            form.attr("action"),
            Some("switch_name.cgi" | "/switch_name.cgi")
        )
        && form
            .attr("enctype")
            .is_none_or(|e| e.eq_ignore_ascii_case("application/x-www-form-urlencoded"))
        && names == 3
        && page
            .select(&css("base[href], [form], [formaction]"))
            .next()
            .is_none()
        && form
            .select(&css("[disabled], [readonly], [onsubmit]"))
            .next()
            .is_none()
        && form.attr("onsubmit").is_none()
        && exact_field("input#switchName[type=text][name=switch_name]")
        && exact_field("input#selLang[type=hidden][name=SET_LANG]")
        && exact_field("input#hash[type=hidden][name=hash]")
}
