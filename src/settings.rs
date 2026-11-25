use crate::admin::{self, Plan, Submission, attr, body, css, field, input, one};
use crate::error::{Error, Result};
use crate::session::{Page, Session};
use clap::{Subcommand, ValueEnum};
use scraper::{ElementRef, Html};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Feature {
    /// Switch name and UI language
    Identity,
    /// Port label, configured speed, flow, priority and rates (requires --port)
    Port,
    /// Device-wide port LED control
    Leds,
    PowerSaving,
    LoopPrevention,
    QosMode,
    BroadcastFilter,
    /// Per-port broadcast storm limit (requires filtering and --port)
    StormRate,
    Discovery,
    Multicast,
    Mirror,
    Lag,
    /// HTTP idle timeout and maximum simultaneous sessions
    HttpSession,
    /// Management IPv4, DHCP, gateway and DNS. Applying may disconnect the session
    Address,
    /// SNTP client enable, mode, port and polling controls
    SntpClient,
    DosPrevention,
}
#[derive(Subcommand)]
pub enum Action {
    /// Show values and available choices for one settings page
    Show {
        feature: Feature,
        #[arg(long,value_parser=clap::value_parser!(u8).range(1..=8))]
        port: Option<u8>,
    },
    /// Preview a change. Pass --apply to submit. Use show to list the accepted keys
    Set {
        feature: Feature,
        #[arg(required = true, value_name = "KEY=VALUE")]
        values: Vec<String>,
        #[arg(long,value_parser=clap::value_parser!(u8).range(1..=8))]
        port: Option<u8>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Default)]
pub struct Values {
    pub values: BTreeMap<String, String>,
    #[serde(skip)]
    controls: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub choices: BTreeMap<String, Vec<String>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<&'static str, &'static str>,
}
impl fmt::Display for Values {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (k, v) in &self.values {
            write!(f, "{k:<18} {}", crate::printable(v))?;
            if let Some(opts) = self.choices.get(k) {
                write!(f, "  [{}]", crate::printable(&opts.join(", ")))?
            }
            writeln!(f)?;
        }
        for (field, reason) in &self.unavailable {
            writeln!(f, "{field:<18} unavailable ({reason})")?;
        }
        Ok(())
    }
}
impl Values {
    fn missing(&mut self, field: &'static str, reason: &'static str, view: PortView) -> Result<()> {
        if matches!(view, PortView::Complete) {
            return Err(Error::PortField { field, reason });
        }
        self.unavailable.insert(field, reason);
        Ok(())
    }
    fn port_input<'a>(
        &mut self,
        row: ElementRef<'a>,
        key: &'static str,
        selector: &str,
        view: PortView,
    ) -> Result<Option<&'a str>> {
        let selector = css(selector);
        let mut controls = row.select(&selector);
        let Some(control) = controls.next() else {
            self.missing(key, "control is absent", view)?;
            return Ok(None);
        };
        if controls.next().is_some() {
            return Err(Error::PortField {
                field: key,
                reason: "duplicate controls",
            });
        }
        control.attr("value").map(Some).ok_or(Error::PortField {
            field: key,
            reason: "control has no value",
        })
    }
    fn put(&mut self, key: &str, value: impl ToString) {
        self.values.insert(key.into(), value.to_string());
    }
    fn control(&mut self, key: &str, value: impl ToString) {
        self.controls.insert(key.into(), value.to_string());
    }
    fn choice(&mut self, key: &str, value: &str, choices: Vec<String>) -> Result<()> {
        if !choices.iter().any(|v| v == value) {
            return Err(Error::Page("selected option absent from UI options"));
        }
        self.put(key, value);
        self.choices.insert(key.into(), choices);
        Ok(())
    }
    fn toggle(&mut self, key: &str, on: bool) {
        self.put(key, if on { "on" } else { "off" });
        self.choices
            .insert(key.into(), vec!["on".into(), "off".into()]);
    }
    fn flag(&self, key: &str) -> bool {
        self.values[key] == "on"
    }
    fn fixed(&mut self, key: &str, index: &str, options: &[(&str, &str)]) -> Result<()> {
        let (_, val) = options
            .iter()
            .find(|(wire, _)| *wire == index)
            .ok_or(Error::Page("unknown option encoding"))?;
        self.choice(
            key,
            val,
            options.iter().map(|(_, v)| v.to_string()).collect(),
        )
    }
}
const SPEED: &[(&str, &str)] = &[
    ("1", "auto"),
    ("2", "disabled"),
    ("4", "10-full"),
    ("6", "100-full"),
];
const PRIORITY: &[(&str, &str)] = &[
    ("0", "low"),
    ("1", "medium"),
    ("2", "high"),
    ("3", "critical"),
];
const QOS: &[(&str, &str)] = &[("1", "port-based"), ("0", "dscp")];
fn wire(value: &str, choices: &[(&str, &str)]) -> Result<String> {
    choices
        .iter()
        .find(|(_, v)| *v == value)
        .map(|(w, _)| w.to_string())
        .ok_or(Error::Usage("unsupported option"))
}
fn checkbox(root: ElementRef<'_>, selector: &str) -> Result<bool> {
    let node = one(root, selector)?;
    if node.attr("type") != Some("checkbox") {
        return Err(Error::Page("expected checkbox"));
    }
    Ok(node.attr("checked").is_some())
}
fn bounded(v: &str, min: u16, max: u16) -> Result<u16> {
    v.parse::<u16>()
        .ok()
        .filter(|n| (min..=max).contains(n))
        .ok_or(Error::Usage("number outside allowed range"))
}
fn validate_static_ipv4(values: &BTreeMap<String, String>) -> Result<()> {
    use std::net::Ipv4Addr;
    let ip = |key: &str| {
        values[key]
            .parse::<Ipv4Addr>()
            .map_err(|_| Error::Usage("invalid IPv4 address, mask, gateway or DNS"))
    };
    let address = ip("address")?;
    let gateway = ip("gateway")?;
    let mask = u32::from(ip("mask")?);
    let usable = |a: Ipv4Addr| {
        !a.is_unspecified() && !a.is_multicast() && !a.is_broadcast() && !a.is_loopback()
    };
    if !usable(address) || mask == 0 || (!mask & (!mask).wrapping_add(1)) != 0 {
        return Err(Error::Usage(
            "a unicast address and contiguous subnet mask are required",
        ));
    }
    let host = u32::from(address) & !mask;
    if mask.count_ones() < 31 && (host == 0 || host == !mask) {
        return Err(Error::Usage("address is the network or broadcast address"));
    }
    if !gateway.is_unspecified()
        && (!usable(gateway) || (u32::from(address) ^ u32::from(gateway)) & mask != 0)
    {
        return Err(Error::Usage(
            "gateway must be on the configured management subnet",
        ));
    }
    for key in ["primary-dns", "secondary-dns"] {
        if !values[key].is_empty() {
            ip(key)?;
        }
    }
    Ok(())
}
fn csv_ports(s: &str) -> Result<BTreeSet<u8>> {
    let mut ports = BTreeSet::new();
    if !s.is_empty() {
        for p in s.split(',') {
            let id = p
                .parse::<u8>()
                .ok()
                .filter(|n| (1..=8).contains(n))
                .ok_or(Error::Usage("use physical port numbers 1-8"))?;
            if !ports.insert(id) {
                return Err(Error::Usage("duplicate port"));
            }
        }
    }
    Ok(ports)
}
fn join_ports(ports: impl IntoIterator<Item = u8>) -> String {
    ports
        .into_iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",")
}
fn bitmap(ports: &BTreeSet<u8>, range: std::ops::RangeInclusive<u8>) -> String {
    range
        .map(|p| if ports.contains(&p) { '1' } else { '0' })
        .collect()
}

#[derive(Clone, Copy)]
enum PortView {
    Report,
    Complete,
}

fn port_settings(page: &Html, port: u8, view: PortView) -> Result<Values> {
    let root = page.root_element();
    let ports = crate::model::ports(page)?;
    let port_info = ports
        .iter()
        .find(|r| r.port == port)
        .ok_or(Error::Page("physical port"))?;
    let mut settings = Values::default();
    settings.put("name", &port_info.name);
    let mut selected = None;
    for row in root.select(&css("#port_details .index_li")) {
        let visible = attr(one(row, ".li_header_content[name]")?, "name")?;
        let id = visible
            .strip_prefix("isShowPot")
            .ok_or(Error::Page("port number"))
            .and_then(admin::numbers)?;
        if (id == port || matches!(view, PortView::Complete))
            && let Some(editor_id) = settings.port_input(row, "editor-port", "input.port", view)?
            && admin::numbers(editor_id)? != id
        {
            return Err(Error::Page("port setting identifiers disagree"));
        }
        if id == port {
            selected = Some(row);
        }
    }
    let row = selected.ok_or(Error::Page("selected settings port"))?;
    for (key, selector, options) in [
        ("speed", "input.Speed", SPEED),
        ("priority", "input.priority", PRIORITY),
        ("flow", "input.flowCtr", &[("1", "on"), ("2", "off")][..]),
    ] {
        if let Some(value) = settings.port_input(row, key, selector, view)? {
            settings
                .fixed(key, value, options)
                .map_err(|_| Error::PortField {
                    field: key,
                    reason: "unknown option encoding",
                })?;
        }
    }
    for (key, selector, menu) in [
        ("ingress", "input.ingressRate", ".IngressList li a"),
        ("egress", "input.egressRate", ".EgressList li a"),
    ] {
        let Some(value) = settings.port_input(row, key, selector, view)? else {
            continue;
        };
        let opts = root.select(&css(menu)).map(body).collect::<Vec<_>>();
        if opts.is_empty() {
            settings.missing(key, "rate menu is absent", view)?;
            continue;
        }
        let index = value.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
        let selected = index
            .and_then(|i| opts.get(i))
            .ok_or(Error::PortField {
                field: key,
                reason: "selection does not match the rate menu",
            })?
            .clone();
        settings.choice(key, &selected, opts)?;
    }
    Ok(settings)
}

impl Feature {
    pub fn observe(
        self,
        page: &Html,
        port: Option<u8>,
        binding: crate::profile::Binding,
    ) -> Result<Values> {
        if self == Self::Port {
            return port_settings(
                page,
                port.ok_or(Error::Usage("port required"))?,
                PortView::Report,
            );
        }
        let mut values = self.read(page, port)?;
        if self == Self::Discovery && binding.reviewed_ui() && !values.values.contains_key("nsdp") {
            values
                .unavailable
                .insert("nsdp", "retired in this firmware");
        }
        Ok(values)
    }
    pub fn page(self) -> Page {
        match self {
            Self::Identity | Self::Port => Page::Dashboard,
            _ => Page::Settings(self),
        }
    }
    pub fn get_path(self) -> &'static str {
        match self {
            Self::Identity | Self::Port => "/dashboard.cgi",
            Self::Leds => "/leds.cgi",
            Self::PowerSaving => "/green_ethernet.cgi",
            Self::LoopPrevention => "/loopDetection.cgi",
            Self::QosMode | Self::BroadcastFilter | Self::StormRate => "/qos.cgi",
            Self::Discovery => "/switchDiscovery.cgi",
            Self::Multicast => "/multicast.cgi",
            Self::Mirror => "/mirroring.cgi",
            Self::Lag => "/lag.cgi",
            Self::HttpSession => "/httpConfiguration.cgi",
            Self::Address => "/ip_dhcp.cgi",
            Self::SntpClient => "/sntpSub.cgi",
            Self::DosPrevention => "/dos.cgi",
        }
    }
    fn toggle_spec(
        self,
    ) -> Option<(
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    )> {
        match self {
            Self::Leds => Some(("#ledSetting", "/port_led.cgi", "portled", "0", "2")),
            Self::PowerSaving => Some((
                "#powStateModSet, #powStateMod",
                "/green_ethernet.cgi",
                "powerSavingMode",
                "1",
                "0",
            )),
            Self::LoopPrevention => Some((
                "#loopPrevMod",
                "/loopDetection.cgi",
                "loopDetection",
                "1",
                "0",
            )),
            Self::BroadcastFilter => Some((
                "#stormControlCheck",
                "/broadCastFilter.cgi",
                "status",
                "Enable",
                "Disable",
            )),
            Self::DosPrevention => Some(("#dosState", "/dos.cgi", "DOS", "1", "0")),
            _ => None,
        }
    }
    pub fn read(self, page: &Html, port: Option<u8>) -> Result<Values> {
        let root = page.root_element();
        let mut s = Values::default();
        if matches!(self, Self::Port | Self::StormRate) != port.is_some() {
            return Err(Error::Usage(
                "--port is required only for port or storm-rate settings",
            ));
        }
        if let Some((selector, ..)) = self.toggle_spec() {
            let enabled = checkbox(root, selector).map_err(|err| match self {
                Self::PowerSaving => {
                    Error::Page("Power Saving page: expected one power-saving checkbox")
                }
                Self::Leds => Error::Page("LED page: expected one LED checkbox"),
                _ => err,
            })?;
            s.toggle("enabled", enabled);
            return Ok(s);
        }
        match self {
            Self::Identity => {
                s.put("name", input(root, "#sysinfoContainer #switchName")?);
                let opts = root
                    .select(&css("#langList li[value]"))
                    .filter_map(|e| e.attr("value").map(str::to_owned))
                    .collect();
                s.choice("language", input(root, "#selLang")?, opts)?;
            }
            Self::Port => {
                return port_settings(
                    page,
                    port.ok_or(Error::Usage("port required"))?,
                    PortView::Complete,
                );
            }
            Self::QosMode => {
                s.fixed("mode", input(root, "#qosMode")?, QOS)?;
            }
            Self::StormRate => {
                if !checkbox(root, "#stormControlCheck")? {
                    return Err(Error::Usage(
                        "storm-rate requires active broadcast filtering",
                    ));
                }
                let rows = root
                    .select(&css("#stormcontrol_edit .selectItem"))
                    .collect::<Vec<_>>();
                if rows.len() != 8 {
                    return Err(Error::Page("storm-rate port rows"));
                }
                for (p, el) in (1..=8).zip(rows) {
                    let dropdown = el
                        .ancestors()
                        .filter_map(ElementRef::wrap)
                        .find(|a| a.value().classes().any(|c| c == "dropdown"))
                        .ok_or(Error::Page("storm-rate menu"))?;
                    let options = dropdown
                        .select(&css("li[value]"))
                        .map(|x| Ok((body(one(x, "a")?), attr(x, "value")?.to_owned())))
                        .collect::<Result<BTreeMap<_, _>>>()?;
                    let wire = options
                        .get(&body(el))
                        .ok_or(Error::Page("storm-rate selection"))?;
                    s.control(&format!("wire-{p}"), wire);
                    if Some(p) == port {
                        s.choice("rate", &body(el), options.keys().cloned().collect())?;
                        for (name, code) in options {
                            s.control(&format!("option-{name}"), code);
                        }
                    }
                }
            }
            Self::Discovery => {
                s.toggle("upnp", checkbox(root, "#upnp_status")?);
                if root.select(&css("#nsdp_status")).next().is_some() {
                    s.toggle("nsdp", checkbox(root, "#nsdp_status")?);
                }
            }
            Self::HttpSession => {
                s.put("timeout-minutes", input(root, "#httpSessTimeout")?);
                s.put("max-sessions", input(root, "#httpMaxSess")?);
            }
            Self::Multicast => {
                for (k, id) in [
                    ("snooping", "#igmp_status"),
                    ("block-unknown", "#mul_addr"),
                    ("validate-header", "#ip_header"),
                ] {
                    s.toggle(k, checkbox(root, id)?);
                }
                let vlan = one(root, "#igmpSnoopingVlanId")?;
                if vlan.attr("disabled").is_none() {
                    s.put("vlan", attr(vlan, "value")?);
                }
                let el = one(root, "[seld-val=rtpt]")?;
                let menu = el
                    .ancestors()
                    .filter_map(ElementRef::wrap)
                    .find(|e| e.value().classes().any(|c| c == "dropdown"))
                    .ok_or(Error::Page("router-port options"))?;
                let options = menu
                    .select(&css("li[value]"))
                    .map(|x| Ok((body(one(x, "a")?), attr(x, "value")?.to_string())))
                    .collect::<Result<BTreeMap<_, _>>>()?;
                s.choice("router-port", &body(el), options.keys().cloned().collect())?;
                for (name, code) in options {
                    s.control(&format!("option-{name}"), code);
                }
            }
            Self::Mirror => {
                s.toggle("enabled", checkbox(root, "#portMirrorMod")?);
                for (k, selector) in [
                    ("sources", "#sourcePortlist"),
                    ("destination", "#destiPortlist"),
                ] {
                    let list = one(root, selector)?;
                    let mut ids = Vec::new();
                    let mut seen = BTreeSet::new();
                    for el in list.select(&css("li.port_circle")) {
                        let p = admin::numbers(&body(one(el, ".port_circle_num")?))?;
                        if !seen.insert(p) {
                            return Err(Error::Page("duplicate mirror port"));
                        }
                        if el.value().classes().any(|c| c == "port_circle_selected") {
                            ids.push(p);
                        }
                    }
                    if seen.len() != 8 {
                        return Err(Error::Page("incomplete mirror table"));
                    }
                    s.put(k, join_ports(ids));
                }
            }
            Self::Lag => {
                let mask = input(root, "#hiddenMem")?;
                if mask.len() != 8 || !mask.bytes().all(|v| matches!(v, b'0' | b'1')) {
                    return Err(Error::Page("LAG member bitmap"));
                }
                for (n, start) in [(1, 1), (2, 5)] {
                    s.toggle(
                        &format!("enabled-{n}"),
                        checkbox(root, &format!("#lagStatus{n}"))?,
                    );
                    let list = one(root, &format!("#lag{n}"))?;
                    let mut members = Vec::new();
                    let all = list
                        .select(&css(".port_circle_num"))
                        .map(|e| admin::numbers(&body(e)))
                        .collect::<Result<Vec<_>>>()?;
                    if all.iter().copied().ne(start..start + 4) {
                        return Err(Error::Page("unsupported LAG layout"));
                    }
                    for p in all {
                        if mask.as_bytes()[usize::from(p - 1)] == b'1' {
                            members.push(p)
                        }
                    }
                    s.put(&format!("members-{n}"), join_ports(members));
                }
            }
            Self::Address => {
                s.toggle("dhcp", checkbox(root, "#dhcpMod")?);
                for (k, sel) in [
                    ("address", "#ipAddr"),
                    ("mask", "#subMsk"),
                    ("gateway", "#gatWay"),
                    ("primary-dns", "#dnsPri"),
                    ("secondary-dns", "#dnsSec"),
                ] {
                    s.put(k, input(root, sel)?);
                }
            }
            Self::SntpClient => {
                s.toggle("enabled", checkbox(root, "#sntpAdmin")?);
                s.choice(
                    "mode",
                    if checkbox(root, "#sntpMode")? {
                        "broadcast"
                    } else {
                        "unicast"
                    },
                    vec!["unicast".into(), "broadcast".into()],
                )?;
                for (k, sel) in [
                    ("port", "#sntpPort"),
                    ("interval", "#ucastPollInterval"),
                    ("timeout", "#ucastPollTimeout"),
                    ("retry", "#ucastPollRetry"),
                ] {
                    s.put(k, input(root, sel)?);
                }
            }
            _ => unreachable!("handled by toggle_spec"),
        }
        Ok(s)
    }
    fn plan(
        self,
        before: &Values,
        updates: &[String],
        port: Option<u8>,
        nsdp_retired: bool,
    ) -> Result<Plan<Values>> {
        let mut next = before.clone();
        let mut keys = BTreeSet::new();
        let mut changes = Vec::new();
        for arg in updates {
            let (key, v) = arg
                .split_once('=')
                .ok_or(Error::Usage("expected KEY=VALUE"))?;
            let old = before
                .values
                .get(key)
                .ok_or(Error::Usage("unknown field; inspect settings show"))?;
            if key.starts_with("option-") || key.starts_with("wire-") || !keys.insert(key) {
                return Err(Error::Usage("duplicate or internal settings key"));
            }
            if let Some(opts) = before.choices.get(key)
                && !opts.iter().any(|x| x == v)
            {
                return Err(Error::Usage("value is not in the available choices"));
            }
            if old != v {
                changes.push(format!("{key} {old:?} -> {v:?}"));
            }
            next.put(key, v);
        }
        let (path, fields) = self.encode(&mut next, &keys, port, nsdp_retired)?;
        let pv = self.to_possible_value().unwrap();
        let feature_name = pv.get_name();
        let summary = if changes.is_empty() {
            "no changes".to_owned()
        } else {
            changes.join(", ")
        };
        Ok(Plan {
            operation: "settings-set",
            details: format!(
                "{feature_name}{} {summary}",
                port.map(|p| format!(" port {p}")).unwrap_or_default(),
            ),
            submission: (next != *before).then_some(Submission { path, fields }),
            expected: next,
        })
    }
    fn encode(
        self,
        s: &mut Values,
        keys: &BTreeSet<&str>,
        port: Option<u8>,
        nsdp_retired: bool,
    ) -> Result<(&'static str, admin::Fields)> {
        let mut fields = Vec::new();
        if let Some((_, path, key, on, off)) = self.toggle_spec() {
            return Ok((
                path,
                vec![field(key, if s.flag("enabled") { on } else { off })],
            ));
        }
        let v = &s.values;
        let path = match self {
            Self::Identity => {
                admin::label(&v["name"], 20)?;
                fields.extend([
                    field("switch_name", &v["name"]),
                    field("SET_LANG", &v["language"]),
                ]);
                "/switch_name.cgi"
            }
            Self::Port => {
                admin::label(&v["name"], 16)?;
                fields.extend([
                    field(&format!("port{}", port.unwrap()), "checked"),
                    field("DESCRIPTION", &v["name"]),
                    field("SPEED", wire(&v["speed"], SPEED)?),
                    field("priority", wire(&v["priority"], PRIORITY)?),
                    field("FLOW_CONTROL", if s.flag("flow") { 1 } else { 2 }),
                ]);
                for (key, post) in [("ingress", "IngressRate"), ("egress", "EgressRate")] {
                    let i = s.choices[key]
                        .iter()
                        .position(|x| x == &v[key])
                        .ok_or(Error::Page("rate choice"))?;
                    fields.push(field(post, i + 1));
                }
                "/port_status.cgi"
            }
            Self::QosMode => {
                fields.push(field(
                    "status",
                    if v["mode"] == "port-based" {
                        "Disable"
                    } else {
                        "Enable"
                    },
                ));
                "/qos.cgi"
            }
            Self::Discovery => {
                for key in ["upnp", "nsdp"] {
                    if key == "nsdp" && nsdp_retired && !v.contains_key(key) {
                        // The V1.0.1.13 UI still submits this constant after removing the protocol.
                        fields.push(field("nsdp", "Enable"));
                        continue;
                    }
                    if !v.contains_key(key) {
                        return Err(Error::Usage(
                            "discovery form omits a co-submitted protocol; write unavailable",
                        ));
                    }
                    fields.push(field(key, if s.flag(key) { "Enable" } else { "Disable" }));
                }
                "/switchDiscovery.cgi"
            }
            Self::StormRate => {
                let p = port.unwrap();
                let wire = s.controls[&format!("option-{}", v["rate"])].clone();
                s.control(&format!("wire-{p}"), wire);
                for p in 1..=8 {
                    fields.push(field(
                        &format!("port{p}"),
                        &s.controls[&format!("wire-{p}")],
                    ));
                }
                "/stromControl.cgi"
            }
            Self::HttpSession => {
                fields.extend([
                    field(
                        "HTTP_SESSION_TIMEOUT",
                        bounded(&v["timeout-minutes"], 0, 60)?,
                    ),
                    field("HTTP_SESSION_MAXNUM", bounded(&v["max-sessions"], 1, 4)?),
                ]);
                "/httpConfiguration.cgi"
            }
            Self::Multicast => {
                for (key, post) in [
                    ("snooping", "status"),
                    ("block-unknown", "BLOCK_UN_MUL_ADDR"),
                    ("validate-header", "IP_HEADER"),
                ] {
                    fields.push(field(post, u8::from(s.flag(key))));
                }
                if let Some(vlan) = v.get("vlan") {
                    fields.push(field("VLAN_ID_ENABLED", bounded(vlan, 1, 4094)?));
                }
                fields.push(field(
                    "ROUTER_PORT",
                    &s.controls[&format!("option-{}", v["router-port"])],
                ));
                "/multicast.cgi"
            }
            Self::Mirror => {
                let source = csv_ports(&v["sources"])?;
                let dest = csv_ports(&v["destination"])?;
                if s.flag("enabled") {
                    if source.is_empty() || dest.len() != 1 || !dest.is_disjoint(&source) {
                        return Err(Error::Usage(
                            "mirroring requires sources and one distinct destination",
                        ));
                    }
                    fields.extend([
                        field("mirrorStatus", 1),
                        field("sourcePort", bitmap(&source, 1..=8)),
                        field("destiPort", dest.first().unwrap()),
                    ]);
                } else {
                    if keys.contains("sources") || keys.contains("destination") {
                        return Err(Error::Usage("enable mirroring to change its ports"));
                    }
                    fields.push(field("mirrorStatus", 0));
                }
                "/mirroring.cgi"
            }
            Self::Lag => {
                for (n, start) in [(1, 1), (2, 5)] {
                    let ports = csv_ports(&v[&format!("members-{n}")])?;
                    if ports.iter().any(|p| !(start..start + 4).contains(p))
                        || (s.flag(&format!("enabled-{n}")) && ports.len() < 2)
                    {
                        return Err(Error::Usage(
                            "LAG needs supported member ports and at least two when enabled",
                        ));
                    }
                    fields.extend([
                        field(
                            &format!("adminMode{n}"),
                            u8::from(s.flag(&format!("enabled-{n}"))),
                        ),
                        field(&format!("portSet_{n}"), bitmap(&ports, 1..=8)),
                    ]);
                }
                "/lag.cgi"
            }
            Self::Address => {
                if s.flag("dhcp") && keys.iter().any(|k| *k != "dhcp") {
                    return Err(Error::Usage(
                        "set dhcp=off before editing static IPv4 fields",
                    ));
                }
                if !s.flag("dhcp") {
                    validate_static_ipv4(v)?;
                }
                fields.push(field("dhcpMode", u8::from(s.flag("dhcp"))));
                for (key, form) in [
                    ("address", "ip_address"),
                    ("mask", "subnet_mask"),
                    ("gateway", "gateway_address"),
                    ("primary-dns", "dns_primary"),
                    ("secondary-dns", "dns_secondary"),
                ] {
                    fields.push(field(form, &v[key]));
                }
                "/ip_dhcp.cgi"
            }
            Self::SntpClient => {
                let p = bounded(&v["port"], 1, 65535)?;
                if p != 123 && p < 1025 {
                    return Err(Error::Usage("invalid SNTP client port"));
                }
                fields.extend([
                    field("SNTP_ADMIN", u8::from(s.flag("enabled"))),
                    field(
                        "SNTP_CLIENT_MODE",
                        if v["mode"] == "broadcast" { 2 } else { 1 },
                    ),
                    field("SNTP_CLIENT_PORT", p),
                    field("UCAST_POLL_INTERVAL", bounded(&v["interval"], 6, 10)?),
                    field("UCAST_POLL_TIMEOUT", bounded(&v["timeout"], 1, 30)?),
                    field("UCAST_POLL_RETRY", bounded(&v["retry"], 0, 10)?),
                ]);
                "/sntpClientConfig.cgi"
            }
            _ => unreachable!("a read-only page or single toggle"),
        };
        Ok((path, fields))
    }
}
pub fn active_lags(session: &Session<'_>) -> Result<Vec<BTreeSet<u8>>> {
    let lag = Feature::Lag.read(&session.read(Feature::Lag.page())?, None)?;
    enabled_groups(&lag)
}
fn enabled_groups(lag: &Values) -> Result<Vec<BTreeSet<u8>>> {
    (1..=2)
        .filter(|n| lag.flag(&format!("enabled-{n}")))
        .map(|n| csv_ports(&lag.values[&format!("members-{n}")]))
        .collect()
}
fn mirror_endpoints(mirror: &Values) -> Result<BTreeSet<u8>> {
    if !mirror.flag("enabled") {
        return Ok(BTreeSet::new());
    }
    let mut ports = csv_ports(&mirror.values["sources"])?;
    ports.extend(csv_ports(&mirror.values["destination"])?);
    Ok(ports)
}
impl Feature {
    fn dependencies(
        self,
        session: &Session<'_>,
        old: &Values,
        next: &Values,
        port: Option<u8>,
    ) -> Result<()> {
        let changed = |k: &str| old.values.get(k) != next.values.get(k);
        match self {
            Self::Port => {
                if changed("priority") {
                    let qos = Self::QosMode.read(&session.read(Self::QosMode.page())?, None)?;
                    if qos.values["mode"] != "port-based" {
                        return Err(Error::Usage("priority requires port-based QoS"));
                    }
                }
                if (changed("speed") || changed("flow"))
                    && active_lags(session)?
                        .iter()
                        .any(|g| port.is_some_and(|p| g.contains(&p)))
                {
                    return Err(Error::Usage(
                        "port speed and flow must remain consistent across an active LAG",
                    ));
                }
            }
            Self::Mirror if next.flag("enabled") => {
                let ports = mirror_endpoints(next)?;
                if active_lags(session)?.iter().any(|g| !g.is_disjoint(&ports)) {
                    return Err(Error::Usage(
                        "a mirror endpoint is a member of an active LAG",
                    ));
                }
            }
            Self::Lag => {
                let groups = enabled_groups(next)?;
                if groups.is_empty() {
                    return Ok(());
                }
                let mirror = Self::Mirror.read(&session.read(Self::Mirror.page())?, None)?;
                let mirrored = mirror_endpoints(&mirror)?;
                let dashboard = session.read(Page::Dashboard)?;
                let vlan = crate::vlan::Table::parse(&session.read(Page::Vlan)?)?;
                for group in groups {
                    if !group.is_disjoint(&mirrored) {
                        return Err(Error::Usage("a LAG member is a mirror endpoint"));
                    }
                    vlan.check_lag(&group)?;
                    let mut first = None;
                    for p in group {
                        let config = Self::Port.read(&dashboard, Some(p))?;
                        let pair = (
                            config.values["speed"].clone(),
                            config.values["flow"].clone(),
                        );
                        if first.as_ref().is_some_and(|v| v != &pair) {
                            return Err(Error::Usage(
                                "LAG members require matching speed and flow",
                            ));
                        }
                        first = Some(pair);
                    }
                }
            }
            Self::Multicast
                if next.flag("snooping") && (changed("vlan") || changed("snooping")) =>
            {
                if let Some(id) = next.values.get("vlan") {
                    let vlan = crate::vlan::Table::parse(&session.read(Page::Vlan)?)?;
                    if !vlan.contains_id(bounded(id, 1, 4094)?) {
                        return Err(Error::Usage("multicast VLAN does not exist"));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
pub fn change(
    session: &Session<'_>,
    feature: Feature,
    port: Option<u8>,
    updates: &[String],
    apply: bool,
    nsdp_retired: bool,
    protected: &[u8],
) -> Result<admin::Change> {
    // The quick Power Saving panel supports reads. Its Settings editor supplies
    // the hidden token used by the native Apply handler on the reviewed firmware.
    let page = if feature == Feature::PowerSaving && session.binding().reviewed_ui() {
        Page::PowerSavingEditor
    } else {
        feature.page()
    };
    admin::change(
        session,
        page,
        apply,
        |p| {
            if feature == Feature::Port && apply && !protected.is_empty() {
                let id = port.ok_or(Error::Usage("port required"))?;
                if protected.contains(&id) {
                    return Err(Error::Usage("cannot modify a protected port"));
                }
            }
            feature.read(p, port)
        },
        |v| {
            let plan = feature.plan(v, updates, port, nsdp_retired)?;
            if plan.submission.is_some() {
                feature.dependencies(session, v, &plan.expected, port)?;
            }
            Ok(plan)
        },
        |a, b| a == b,
    )
}
