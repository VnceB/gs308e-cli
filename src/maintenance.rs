use crate::Report;
use crate::admin::{self, Change, attr, body, css, field, one, token};
use crate::error::{Error, Result};
use crate::session::{Page, Session};
use clap::Subcommand;
use scraper::{ElementRef, Html};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Subcommand)]
pub enum Action {
    /// Read the last cable-test results already present on the switch
    CableResults,
    /// Preview starting a physical cable diagnostic on one port. It can interrupt that link
    CableTest {
        #[arg(value_parser=clap::value_parser!(u8).range(1..=8))]
        port: u8,
    },
    /// Preview restarting the switch. All links will be interrupted
    Reboot {
        #[arg(long, required = true)]
        accept_outage: bool,
    },
    /// Install the reviewed NETGEAR V1.0.1.13 binary. Installation interrupts all links
    Upgrade {
        image: std::path::PathBuf,
        #[arg(long, required = true)]
        accept_outage: bool,
    },
    /// Export the switch configuration to explicitly redirected stdout
    Backup {
        #[arg(long, required = true)]
        stdout: bool,
    },
}
#[derive(Serialize)]
pub struct CableResult {
    pub port: u8,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<String>,
}
// The vendor table gives result and distance cells the same CSS class. Only
// the panel's complete, ordered column headings disambiguate those cells.
fn cable_columns(panel: ElementRef<'_>) -> Result<bool> {
    let headers = css(".testResult_title");
    let mut matches = panel.select(&headers);
    let Some(header) = matches.next() else {
        return Ok(false);
    };
    let columns: Vec<_> = header.child_elements().collect();
    if matches.next().is_some()
        || header.value().name() != "ul"
        || columns.iter().any(|el| {
            el.value().name() != "li" || !el.value().classes().any(|c| c == "testResult_title_li")
        })
        || columns.into_iter().map(body).ne(["Port", "ml078", "ml445"])
    {
        return Err(Error::Page(
            "unrecognized or ambiguous cable-result column headings",
        ));
    }
    Ok(true)
}

struct CableCells<'a> {
    result: ElementRef<'a>,
    distance: Option<ElementRef<'a>>,
}

fn cable_cells<'a>(
    row: ElementRef<'a>,
    port: ElementRef<'a>,
    has_columns: bool,
) -> Result<CableCells<'a>> {
    if !has_columns {
        let result = one(row, ".testResult_content_result").map_err(|_| {
            Error::Page("expected one stored result cell in a row without column headings")
        })?;
        return Ok(CableCells {
            result,
            distance: None,
        });
    }
    let container = one(row, ".testResultContainer")
        .map_err(|_| Error::Page("expected one cable-result cell container"))?;
    if container.value().name() != "div"
        || container.parent() != Some(*row)
        || row.child_elements().count() != 1
    {
        return Err(Error::Page(
            "unexpected structure in stored cable-result row",
        ));
    }
    if [row, container].iter().any(|el| {
        el.children()
            .filter_map(|n| n.value().as_text())
            .any(|s| !s.trim().is_empty())
    }) {
        return Err(Error::Page(
            "unexpected text outside the stored cable-result cells",
        ));
    }
    let cells = container.child_elements().collect::<Vec<_>>();
    let [first, result, distance] = cells.as_slice() else {
        return Err(Error::Page("expected port, test result and distance cells"));
    };
    let value_class = |el: &ElementRef<'_>| {
        el.value()
            .classes()
            .any(|c| c == "testResult_content_result")
    };
    if *first != port
        || !value_class(result)
        || !value_class(distance)
        || row.select(&css(".testResult_content_result")).count() != 2
    {
        return Err(Error::Page(
            "cable-result cells disagree with column headings",
        ));
    }
    Ok(CableCells {
        result: *result,
        distance: Some(*distance),
    })
}

// V1.0.1.13 clears both display buffers before each port and emits both
// spans even when neither buffer was filled. This is absence on the page,
// independent of whether the device has ever tested the port.
fn empty_display_cells(cells: &CableCells<'_>, port: ElementRef<'_>, number: u8) -> bool {
    let exact_span = |el: ElementRef<'_>, role: &str| {
        el.value().name() == "span"
            && el.value().attrs().count() == 1
            && el.value().classes().count() == 2
            && el.value().classes().any(|c| c == "testResult_content")
            && el.value().classes().any(|c| c == role)
    };
    let empty_value = |el: ElementRef<'_>| {
        exact_span(el, "testResult_content_result") && el.children().next().is_none()
    };
    if !empty_value(cells.result)
        || !cells.distance.is_some_and(empty_value)
        || !exact_span(port, "testResult_content_port")
    {
        return false;
    }
    let mut nodes = port.children();
    let Some(prefix) = nodes.next().and_then(|n| n.value().as_text()) else {
        return false;
    };
    let id = number.to_string();
    match (nodes.next(), nodes.next()) {
        (None, None) => **prefix == id,
        (Some(child), None) => {
            prefix.replace('\u{a0}', " ") == format!("{id} - ")
                && ElementRef::wrap(child).is_some_and(|name| {
                    name.value().name() == "b"
                        && name.value().attrs().count() == 1
                        && name.attr("common").is_some()
                        && name.children().count() == 1
                        && name
                            .first_child()
                            .and_then(|n| n.value().as_text())
                            .is_some_and(|s| !s.trim().is_empty())
                })
        }
        _ => false,
    }
}

fn reviewed_result(
    cells: CableCells<'_>,
    port: ElementRef<'_>,
    number: u8,
    binding: crate::profile::Binding,
) -> Result<Option<(String, Option<String>)>> {
    let result = body(cells.result);
    let distance = cells.distance.map(body);
    if result.is_empty() {
        if binding.reviewed_ui() && empty_display_cells(&cells, port, number) {
            return Ok(None);
        }
        return Err(Error::Page(match distance.as_deref() {
            None => "stored cable result is blank without verified distance headings",
            Some("") => "stored cable result and distance are both blank",
            Some("--") => "stored cable result is blank with a distance placeholder",
            Some("Not test") => {
                "stored cable result is blank with the untested marker in the distance column"
            }
            Some(_) => "stored cable result is blank with a populated distance",
        }));
    }
    if result == "Not test" {
        let Some(distance_el) = cells.distance else {
            return Err(Error::Page(
                "untested cable marker without verified distance headings",
            ));
        };
        let plain_value = |el: ElementRef<'_>| {
            el.value().name() == "span"
                && el.value().classes().any(|c| c == "testResult_content")
                && el.child_elements().next().is_none()
        };
        let mut names = port.child_elements();
        let recognizable_port = port.value().name() == "span"
            && port.value().classes().any(|c| c == "testResult_content")
            && names.next().is_none_or(|name| {
                name.value().name() == "b"
                    && name.attr("common").is_some()
                    && name.child_elements().next().is_none()
            })
            && names.next().is_none();
        if !plain_value(cells.result) || !plain_value(distance_el) || !recognizable_port {
            return Err(Error::Page(
                "unrecognized cells around the untested cable marker",
            ));
        }
        if !matches!(distance.as_deref(), Some("" | "--")) {
            return Err(Error::Page(
                "untested cable marker conflicts with a populated distance",
            ));
        }
        return Ok(None);
    }
    if cells.distance.is_some()
        && !matches!(
            result.as_str(),
            "OK" | "No Cable"
                | "Open Cable"
                | "Short"
                | "ERROR"
                | "Failed"
                | "Not support"
                | "Disabled"
        )
    {
        return Err(Error::Page(
            "unrecognized status in the stored cable result column",
        ));
    }
    let clean_distance = distance.filter(|d| !d.is_empty() && d != "--");
    Ok(Some((result, clean_distance)))
}

pub(crate) fn cable_results(
    page: &Html,
    binding: crate::profile::Binding,
) -> Result<Vec<CableResult>> {
    let panel = one(page.root_element(), "#testResult")
        .map_err(|_| Error::Page("expected one cable-results panel"))?;
    let list = one(panel, ".testResult_list")
        .map_err(|_| Error::Page("expected one stored cable-results list"))?;
    let has_columns = cable_columns(panel)?;
    if list.value().name() != "ul"
        || list.child_elements().any(|el| {
            el.value().name() != "li" || !el.value().classes().any(|c| c == "testResult_li")
        })
        || list
            .children()
            .filter_map(|node| node.value().as_text())
            .any(|t| !t.trim().is_empty())
    {
        return Err(Error::Page(
            "unrecognized content in the stored cable-results list",
        ));
    }
    let mut rows = BTreeMap::new();
    for li in list.select(&css(".testResult_li")) {
        let port = one(li, ".testResult_content_port")
            .map_err(|_| Error::Page("expected one port cell per cable-result row"))?;
        let label = body(port);
        let number = label
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap_or("");
        let p = admin::numbers(number)?;
        let cells = cable_cells(li, port, has_columns)?;
        if li.parent() != Some(*list) {
            return Err(Error::Page(
                "unexpected nesting in stored cable-result rows",
            ));
        }
        if rows.contains_key(&p) {
            return Err(Error::Page(
                "duplicate port identifier in stored cable results",
            ));
        }
        if has_columns {
            let label = label.replace('\u{a0}', " ");
            let id = p.to_string();
            if label != id
                && !label
                    .strip_prefix(&id)
                    .and_then(|rest| rest.strip_prefix(" - "))
                    .is_some_and(|name| !name.is_empty())
            {
                return Err(Error::Page(
                    "unrecognized port label in stored cable results",
                ));
            }
        }
        let result = reviewed_result(cells, port, p, binding)?;
        // An omitted row still occupies its port in the table.
        rows.insert(
            p,
            result.map(|(result, distance)| CableResult {
                port: p,
                result,
                distance,
            }),
        );
    }
    Ok(rows.into_values().flatten().collect())
}
impl Action {
    pub fn run(
        &self,
        session: &Session<'_>,
        apply: bool,
        protected: &[u8],
        dashboard: Option<&Html>,
    ) -> Result<Report> {
        match self {
            Self::Upgrade { image, .. } => {
                let fetched;
                let dash = match dashboard {
                    Some(d) => d,
                    None => {
                        fetched = session.dashboard()?;
                        &fetched
                    }
                };
                Ok(Report::Change(crate::firmware::upgrade(
                    session, image, apply, protected, dash,
                )?))
            }
            Self::CableResults => Ok(Report::CableResults(cable_results(
                &session.read(Page::Cable)?,
                session.binding(),
            )?)),
            Self::Backup { .. } => {
                let page = session.read(Page::ConfigFile)?;
                let button = one(page.root_element(), "button[name=downloadCf]")?;
                let link = button
                    .ancestors()
                    .filter_map(scraper::ElementRef::wrap)
                    .find(|e| e.value().classes().any(|s| s == "download-conf-btn"))
                    .ok_or(Error::Page("configuration download control"))?;
                if attr(link, "onclick")?.trim().trim_end_matches(';')
                    != "document.location.href='/confFile.cgi?cmd=backup_conf'"
                {
                    return Err(Error::Page("unknown configuration download action"));
                }
                Ok(Report::Backup(session.backup()?))
            }
            Self::CableTest { port } => {
                if apply && protected.contains(port) {
                    return Err(Error::Usage(
                        "a cable test can interrupt a protected or dependent path",
                    ));
                }
                let page = session.read(Page::Cable)?;
                let root = page.root_element();
                one(root, "button[name=submitCbtst]")?;
                let ports = one(root, ".cable_test_port_list")?
                    .select(&css("li .port_circle_num"))
                    .map(|n| admin::numbers(&body(n)))
                    .collect::<Result<Vec<_>>>()?;
                if ports.iter().copied().ne(1..=8) {
                    return Err(Error::Page("ordered cable-test port selectors"));
                }
                let key = token(&page)?;
                if apply {
                    session.submit(
                        "/cableTester.cgi",
                        &[
                            field("hash", key),
                            field(&format!("port{}", port - 1), "checked"),
                        ],
                    )?;
                }
                Ok(Report::Change(Change {
                    operation: "cable-test",
                    outcome: if apply { "accepted" } else { "preview" },
                    details: format!(
                        "port {port}; inspect maintenance cable-results when the test finishes"
                    ),
                }))
            }
            Self::Reboot { .. } => {
                if apply && !protected.is_empty() {
                    return Err(Error::Usage("reboot interrupts all ports"));
                }
                let page = session.read(Page::Reboot)?;
                one(page.root_element(), "[name=submitReboot]")?;
                let key = token(&page)?;
                if apply {
                    session.submit("/device_reboot.cgi", &[field("hash", key)])?;
                }
                Ok(Report::Change(Change {
                    operation: "reboot",
                    outcome: if apply { "accepted" } else { "preview" },
                    details: "switch-wide outage; use info in a new session after recovery".into(),
                }))
            }
        }
    }
}
