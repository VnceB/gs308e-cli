use crate::error::{Error, Result};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;

const PORTS: usize = 8;

fn css(query: &str) -> Selector {
    Selector::parse(query).expect("internal CSS selector")
}

fn value<'a>(root: ElementRef<'a>, query: &str) -> Result<&'a str> {
    root.select(&css(query))
        .next()
        .and_then(|e| e.attr("value"))
        .ok_or(Error::Page("missing input"))
}

fn text(node: ElementRef<'_>) -> String {
    node.text().collect::<String>().trim().to_owned()
}

pub fn challenge(page: &Html) -> Result<String> {
    let form = page
        .select(&css("form[name=login]"))
        .next()
        .ok_or(Error::Page("login form"))?;
    if form.attr("action") != Some("/login.cgi")
        || !form
            .attr("method")
            .is_some_and(|v| v.eq_ignore_ascii_case("post"))
        || form
            .select(&css("input#submitPwd[name=password]"))
            .next()
            .is_none()
    {
        return Err(Error::Page("login form"));
    }
    let nonce = value(form, "input#rand")?;
    if nonce.is_empty() || nonce.len() > 128 || !nonce.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Page("login challenge"));
    }
    Ok(nonce.into())
}

pub fn is_login(page: &Html) -> bool {
    page.select(&css("form[name=login]")).next().is_some()
        || page
            .select(&css("title"))
            .any(|e| text(e).eq_ignore_ascii_case("Redirect to Login"))
}

pub fn check_model(page: &Html) -> Result<()> {
    let selector = css("#sysinfoContainer input#prctName");
    let mut models = page.select(&selector);
    let model = models
        .next()
        .and_then(|el| el.attr("value"))
        .ok_or(Error::Page("missing model"))?;
    if models.next().is_some() {
        return Err(Error::Page("ambiguous dashboard model"));
    }
    if model == "GS308Ev4" {
        Ok(())
    } else {
        Err(Error::Model)
    }
}

#[derive(Debug, Serialize)]
pub struct Device {
    pub model: &'static str,
    pub firmware: String,
    pub name: String,
    pub mac_address: String,
    pub serial_number: String,
}

pub fn device(page: &Html) -> Result<Device> {
    let mut fields = std::collections::HashMap::new();
    for cell in page.select(&css("#sysinfoContainer .hid_info_cell")) {
        let mut children = cell.child_elements();
        if let (Some(title), Some(data)) = (children.next(), children.next()) {
            let key = text(title);
            if matches!(key.as_str(), "ml089" | "ml678" | "ml198")
                && fields.insert(key, text(data)).is_some()
            {
                return Err(Error::Page("ambiguous system info field"));
            }
        }
    }
    let mut field = |key| {
        fields
            .remove(key)
            .filter(|s| !s.is_empty())
            .ok_or(Error::Page("system info field"))
    };
    Ok(Device {
        model: "GS308Ev4",
        firmware: field("ml089")?,
        name: value(page.root_element(), "#sysinfoContainer input#switchName")?.into(),
        mac_address: field("ml678")?,
        serial_number: field("ml198")?,
    })
}

#[derive(Debug, Serialize)]
pub struct Port {
    pub port: u8,
    pub name: String,
    pub status: String,
    pub link_mbps: Option<u16>,
    pub duplex: Option<String>,
}

fn port_number(raw: &str) -> Result<u8> {
    raw.trim()
        .parse::<u8>()
        .ok()
        .filter(|n| (1..=8).contains(n))
        .ok_or(Error::Page("port number"))
}

fn check_ports<T>(rows: &mut [T], number: impl Fn(&T) -> u8) -> Result<()> {
    rows.sort_by_key(&number);
    if rows.len() != PORTS || rows.iter().map(number).ne(1..=8) {
        return Err(Error::Page("expected eight distinct port numbers"));
    }
    Ok(())
}

fn link_speed(raw: &str) -> Result<(Option<u16>, Option<String>)> {
    if raw.eq_ignore_ascii_case("No Speed") {
        return Ok((None, None));
    }
    let (rate, duplex) = raw
        .trim()
        .split_once(' ')
        .ok_or(Error::Page("linked speed"))?;
    let mbps = rate
        .strip_suffix('M')
        .and_then(|s| s.parse().ok())
        .filter(|v| matches!(v, 10 | 100 | 1000))
        .ok_or(Error::Page("linked speed"))?;
    let duplex = duplex.trim().to_ascii_lowercase();
    if !matches!(duplex.as_str(), "half" | "full") {
        return Err(Error::Page("duplex"));
    }
    Ok((Some(mbps), Some(duplex)))
}

pub fn ports(page: &Html) -> Result<Vec<Port>> {
    let mut rows = Vec::new();
    for item in page.select(&css("#port_details .index_li")) {
        let id = item
            .select(&css(".li_header_content[name]"))
            .next()
            .and_then(|e| e.attr("name"))
            .and_then(|n| n.strip_prefix("isShowPot"))
            .ok_or(Error::Page("port number"))?;
        let status = item
            .select(&css("span.pull-right"))
            .map(text)
            .find(|s| !s.is_empty())
            .ok_or(Error::Page("port status"))?
            .to_ascii_lowercase();
        let (link_mbps, duplex) = link_speed(value(item, "input.LinkedSpeed")?)?;
        rows.push(Port {
            port: port_number(id)?,
            name: value(item, "input.portName")?.into(),
            status,
            link_mbps,
            duplex,
        });
    }
    check_ports(&mut rows, |p| p.port)?;
    Ok(rows)
}

#[derive(Debug, Serialize)]
pub struct Counter {
    pub port: u8,
    #[serde(serialize_with = "decimal_counter")]
    pub rx_bytes: u64,
    #[serde(serialize_with = "decimal_counter")]
    pub tx_bytes: u64,
    #[serde(serialize_with = "decimal_counter")]
    pub crc_errors: u64,
}

// A decimal string preserves all 64 bits in JSON consumers that use doubles.
fn decimal_counter<S: serde::Serializer>(
    value: &u64,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

pub fn counters(page: &Html) -> Result<Vec<Counter>> {
    let list = page
        .select(&css("#settingsStatusContainer .statistic-ul"))
        .next()
        .ok_or(Error::Page("statistics list"))?;
    let items = css("li");
    let mut rows = list.select(&items);
    let columns = rows.next().ok_or(Error::Page("statistics header"))?;
    if columns
        .child_elements()
        .map(text)
        .ne(["ml578", "ml711", "ml466", "ml563"])
    {
        return Err(Error::Page("statistics columns"));
    }
    let words = list
        .select(&css("input[type=hidden]"))
        .map(|e| {
            e.attr("value")
                .and_then(|v| v.parse::<u32>().ok())
                .ok_or(Error::Page("counter word"))
        })
        .collect::<Result<Vec<_>>>()?;
    if words.len() != PORTS * 6 {
        return Err(Error::Page("counter word count"));
    }
    let rows = rows.collect::<Vec<_>>();
    if rows.len() != PORTS {
        return Err(Error::Page("statistics row count"));
    }
    let mut result = Vec::new();
    for (row, w) in rows.into_iter().zip(words.as_chunks::<6>().0) {
        let id = row
            .child_elements()
            .next()
            .ok_or(Error::Page("statistics port"))?;
        let pair = |i: usize| (u64::from(w[i]) << 32) | u64::from(w[i + 1]);
        result.push(Counter {
            port: port_number(&text(id))?,
            rx_bytes: pair(0),
            tx_bytes: pair(2),
            crc_errors: pair(4),
        });
    }
    check_ports(&mut result, |c| c.port)?;
    Ok(result)
}

#[derive(Debug, Serialize)]
pub struct Vlan {
    pub mode: &'static str,
}

pub fn vlan(page: &Html) -> Result<Vlan> {
    let root = page.root_element();
    let active = crate::admin::input(root, "input#vlanMod")?;
    let number = active
        .parse::<u8>()
        .map_err(|_| Error::Page("VLAN mode number"))?;
    let button = crate::admin::one(root, &format!("button[mod-num='{number}']"))?;
    let mode = match (active, button.attr("name")) {
        ("0", Some("noVlan")) => "disabled",
        ("1", Some("bscPotBsd")) => "basic_port_based",
        ("2", Some("advPotBsd")) => "advanced_port_based",
        ("3", Some("bsc8021Q")) => "basic_802_1q",
        ("4", Some("adv8021Q")) => "advanced_802_1q",
        _ => return Err(Error::Page("VLAN mode")),
    };
    Ok(Vlan { mode })
}
