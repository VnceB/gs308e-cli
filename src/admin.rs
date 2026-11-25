use crate::error::{Error, Result};
use crate::session::{Page, Session};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use std::fmt;

pub fn css(s: &str) -> Selector {
    Selector::parse(s).expect("internal UI selector")
}
pub fn one<'a>(root: ElementRef<'a>, selector: &str) -> Result<ElementRef<'a>> {
    let sel = css(selector);
    let mut nodes = root.select(&sel);
    let node = nodes
        .next()
        .ok_or(Error::Page("required control missing"))?;
    if nodes.next().is_some() {
        return Err(Error::Page("ambiguous control"));
    }
    Ok(node)
}
pub fn attr<'a>(node: ElementRef<'a>, key: &str) -> Result<&'a str> {
    node.attr(key)
        .ok_or(Error::Page("control attribute missing"))
}
pub fn body(node: ElementRef<'_>) -> String {
    node.text().collect::<String>().trim().to_owned()
}
pub fn input<'a>(root: ElementRef<'a>, sel: &str) -> Result<&'a str> {
    attr(one(root, sel)?, "value")
}
pub fn token(page: &Html) -> Result<String> {
    token_value(one(page.root_element(), "input[type=hidden]#hash")?)
}
fn token_value(node: ElementRef<'_>) -> Result<String> {
    let value = attr(node, "value")?;
    if value.is_empty() || value.len() > 256 || !value.bytes().all(|v| v.is_ascii_graphic()) {
        return Err(Error::Page("form token"));
    }
    Ok(value.into())
}
pub fn numbers(key: &str) -> Result<u8> {
    key.parse::<u8>()
        .ok()
        .filter(|p| (1..=8).contains(p))
        .ok_or(Error::Page("physical port number"))
}
pub fn label(value: &str, max: usize) -> Result<()> {
    if value.len() > max || !value.bytes().all(|v| (32..=126).contains(&v)) {
        return Err(Error::Usage(
            "name is too long or contains non-printable characters",
        ));
    }
    Ok(())
}
fn form_token(session: &Session<'_>, which: Page, doc: &Html) -> Result<String> {
    use crate::settings::Feature;
    match which {
        Page::Settings(Feature::SntpClient) | Page::TimeTab(_) => {
            token(&session.read(Page::SntpParent)?)
        }
        Page::PowerSavingEditor => toggle_editor_token(
            doc,
            "#powStateModSet, #powStateMod",
            "powStateModSet",
            "button[name=submitPwsv]",
            "submitPowerSave('#powStateModSet')",
        ),
        Page::Settings(Feature::Leds) if session.binding().reviewed_ui() => toggle_editor_token(
            doc,
            "#ledSetting, #ledMod",
            "ledSetting",
            "button[name=submitLed2]",
            "submitLED()",
        ),
        _ => token(doc),
    }
}

// These two Settings pages have page-local Apply handlers and exactly one
// hidden hash input. Never obtain a token from a dashboard or another editor.
fn toggle_editor_token(
    doc: &Html,
    toggle_selector: &str,
    toggle_id: &str,
    button_selector: &str,
    handler: &str,
) -> Result<String> {
    let root = doc.root_element();
    let toggle = one(root, toggle_selector)
        .map_err(|_| Error::Page("settings preflight: expected one editor checkbox"))?;
    if toggle.value().name() != "input"
        || toggle.attr("id") != Some(toggle_id)
        || toggle.attr("type") != Some("checkbox")
        || toggle.attr("disabled").is_some()
    {
        return Err(Error::Page(
            "settings preflight: expected the enabled Settings editor checkbox",
        ));
    }
    let button = one(root, button_selector)
        .map_err(|_| Error::Page("settings preflight: expected one editor Apply button"))?;
    if button
        .attr("onclick")
        .map(|s| s.trim().trim_end_matches(';'))
        != Some(handler)
    {
        return Err(Error::Page(
            "settings preflight: unrecognized editor Apply control",
        ));
    }
    if button.attr("disabled").is_some()
        && !(toggle_id == "ledSetting" && led_edit_enables_apply(root, toggle, button))
    {
        return Err(Error::Page(
            "settings preflight: disabled Apply button without a recognized edit-to-enable handler",
        ));
    }
    let key = one(root, "#hash, [name=hash]").map_err(|_| {
        Error::Page("settings preflight: expected one page-local hidden form token")
    })?;
    if key.value().name() != "input"
        || key.attr("type") != Some("hidden")
        || key.attr("id") != Some("hash")
        || key.attr("name") != Some("hash")
        || key.attr("disabled").is_some()
    {
        return Err(Error::Page(
            "settings preflight: unrecognized editor form-token control",
        ));
    }
    token_value(key)
        .map_err(|_| Error::Page("settings preflight: missing or invalid editor form-token value"))
}

// V1.0.1.13 emits the LED Apply button disabled until the editable checkbox
// calls enableButtons(), which enables .submit_btn .button_theme_flat.
fn led_edit_enables_apply(
    root: ElementRef<'_>,
    toggle: ElementRef<'_>,
    button: ElementRef<'_>,
) -> bool {
    let Ok(editor) = one(root, "#settingsStatusContainer") else {
        return false;
    };
    let blocked = |el: ElementRef<'_>| {
        (el.value().name() == "fieldset" && el.attr("disabled").is_some())
            || el.attr("inert").is_some()
            || el.attr("aria-disabled").is_some()
    };
    editor.value().name() == "div"
        && button.attr("disabled") == Some("")
        && toggle.attr("onclick") == Some("enableButtons()")
        && editor
            .select(&css(".checkbox > input#ledSetting"))
            .any(|el| el == toggle)
        && editor
            .select(&css(".submit_btn button.button_theme_flat"))
            .any(|el| el == button)
        && [toggle, button].into_iter().all(|el| {
            !std::iter::once(el)
                .chain(el.ancestors().filter_map(ElementRef::wrap))
                .any(blocked)
        })
}

pub fn clear(session: &Session<'_>, apply: bool) -> Result<Change> {
    let page = session.read(Page::Statistics)?;
    let rows = crate::model::counters(&page)?;
    let key = token(&page)?;
    let outcome = if !apply {
        "preview"
    } else if rows
        .iter()
        .all(|r| r.rx_bytes == 0 && r.tx_bytes == 0 && r.crc_errors == 0)
    {
        "unchanged"
    } else {
        session.submit(
            "/portStatistics.cgi",
            &[field("clearCounters", 1), field("hash", key)],
        )?;
        let after = session
            .read(Page::Statistics)
            .and_then(|p| crate::model::counters(&p))
            .map_err(|_| Error::Unverified)?;
        if after
            .iter()
            .any(|r| r.rx_bytes != 0 || r.tx_bytes != 0 || r.crc_errors != 0)
        {
            return Err(Error::Unverified);
        }
        "verified"
    };
    Ok(Change {
        operation: "clear-counters",
        outcome,
        details: "all eight ports; counter history is lost".into(),
    })
}
pub type Fields = Vec<(String, String)>;
pub fn field(k: &str, v: impl ToString) -> (String, String) {
    (k.into(), v.to_string())
}

#[derive(Serialize)]
pub struct Change {
    pub operation: &'static str,
    pub outcome: &'static str,
    pub details: String,
}
impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}  {}  {}",
            self.outcome,
            self.operation,
            crate::printable(&self.details)
        )
    }
}
pub struct Submission {
    pub path: &'static str,
    pub fields: Fields,
}
pub struct Plan<T> {
    pub operation: &'static str,
    pub details: String,
    pub submission: Option<Submission>,
    pub expected: T,
}

/// Compare complete parsed preimages, taking the form token only from the latest read.
/// An ambiguous POST outcome is never retried. Only an independent GET verifies success.
pub fn change<T: PartialEq>(
    session: &Session<'_>,
    page: Page,
    apply: bool,
    parse: impl Fn(&Html) -> Result<T>,
    plan: impl Fn(&T) -> Result<Plan<T>>,
    matches: impl Fn(&T, &T) -> bool,
) -> Result<Change> {
    let baseline = session.read(page)?;
    let before = parse(&baseline)?;
    let p = plan(&before)?;
    let outcome = match p.submission {
        None => "unchanged",
        Some(request) => {
            form_token(session, page, &baseline)?;
            if apply {
                let fresh = session.read(page)?;
                if parse(&fresh)? != before {
                    return Err(Error::Usage(
                        "configuration changed during preflight; inspect again",
                    ));
                }
                // Re-evaluate any dependencies on other pages immediately before the one submission.
                let checked = plan(&before)?;
                let Some(latest) = checked.submission else {
                    return Err(Error::Usage(
                        "configuration dependencies changed during preflight",
                    ));
                };
                if latest.path != request.path
                    || latest.fields != request.fields
                    || checked.expected != p.expected
                {
                    return Err(Error::Usage(
                        "configuration dependencies changed during preflight",
                    ));
                }
                let mut fields = latest.fields;
                let key = if matches!(
                    page,
                    Page::Settings(crate::settings::Feature::SntpClient) | Page::TimeTab(_)
                ) {
                    "HASH"
                } else {
                    "hash"
                };
                fields.push(field(key, form_token(session, page, &fresh)?));
                session.submit(request.path, &fields)?;
                let current = session
                    .read(page)
                    .and_then(|h| parse(&h))
                    .map_err(|_| Error::Unverified)?;
                if !matches(&p.expected, &current) {
                    return Err(Error::Unverified);
                }
                "verified"
            } else {
                "preview"
            }
        }
    };
    Ok(Change {
        operation: p.operation,
        details: p.details,
        outcome,
    })
}
