use crate::Report;
use crate::admin::{self, Change, Plan, Submission, attr, body, css, field, input, one, token};
use crate::error::{Error, Result};
use crate::session::{Page, Session};
use clap::{Subcommand, ValueEnum};
use scraper::Html;
use serde::Serialize;
use std::{collections::BTreeMap, fmt, net::Ipv4Addr, time::Instant};

#[derive(Subcommand)]
pub enum Action {
    /// Read the switch's local clock, synchronization setting and firmware time-zone index
    Clock,
    /// Set a local date and time, or enable SNTP. Zone is the firmware's time-zone index
    SetClock {
        source: Source,
        #[arg(long, requires = "time")]
        date: Option<String>,
        #[arg(long, requires = "date")]
        time: Option<String>,
        #[arg(long,value_parser=clap::value_parser!(u8).range(1..=98))]
        zone: Option<u8>,
    },
    /// Read the SNTP client operational status
    Status,
    /// Read the effective daylight-saving dates and offset
    DstStatus,
    /// Read the configured daylight-saving rule
    Dst,
    /// Configure a rule. Custom start/end are week,weekday,month,hour,minute or YYYY-MM-DDTHH:MM
    SetDst {
        mode: DstMode,
        #[arg(long,value_parser=clap::value_parser!(u8).range(1..=98))]
        zone: Option<u8>,
        #[arg(long, requires = "end")]
        start: Option<String>,
        #[arg(long, requires = "start")]
        end: Option<String>,
    },
    /// List configured unicast SNTP servers by priority
    Servers,
    /// Add one IPv4 or DNS server with a unique priority
    ServerAdd {
        address: String,
        #[arg(long,value_parser=clap::value_parser!(u8).range(1..=3))]
        priority: u8,
        #[arg(long, default_value_t = 123)]
        port: u16,
        #[arg(long,default_value_t=4,value_parser=clap::value_parser!(u8).range(1..=4))]
        version: u8,
    },
    /// Remove the server with the given priority after checking its current address
    ServerDelete {
        #[arg(value_parser=clap::value_parser!(u8).range(1..=3))]
        priority: u8,
        #[arg(long)]
        address: String,
    },
}
#[derive(Copy, Clone, ValueEnum, PartialEq, Eq)]
pub enum Source {
    Manual,
    Sntp,
}
#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum DstMode {
    Disabled,
    Eu,
    Us,
    Recurring,
    Once,
}
impl DstMode {
    fn id(self) -> &'static str {
        match self {
            Self::Disabled => "disable",
            Self::Eu => "recurringEU",
            Self::Us => "recurringUSA",
            Self::Recurring => "recurring",
            Self::Once => "nonRecurring",
        }
    }
    fn from_id(id: &str) -> Result<Self> {
        Self::value_variants()
            .iter()
            .copied()
            .find(|v| v.id() == id)
            .ok_or(Error::Page("unrecognized daylight-saving mode"))
    }
}
#[derive(Default, Serialize, Clone, PartialEq, Eq)]
pub struct Fields(pub BTreeMap<String, String>);
impl fmt::Display for Fields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (k, v) in &self.0 {
            writeln!(f, "{k:<23} {}", crate::printable(v))?;
        }
        Ok(())
    }
}
fn status(page: &Html, dst: bool) -> Result<Fields> {
    const SNTP: &[(&str, &str)] = &[
        ("ml1003", "version"),
        ("ml1004", "server-address"),
        ("ml1005", "max-servers"),
        ("ml1006", "supported-mode"),
        ("ml1007", "address-type"),
        ("ml1008", "configured-servers"),
        ("ml1009", "last-update"),
        ("ml1010", "stratum"),
        ("ml1011", "broadcast-count"),
        ("ml1012", "last-attempt"),
        ("ml1013", "reference-clock"),
        ("ml1014", "last-attempt-status"),
        ("ml1015", "server-mode"),
    ];
    const DST: &[(&str, &str)] = &[
        ("ml1062", "mode"),
        ("ml1063", "begin"),
        ("ml1064", "end"),
        ("ml1065", "offset-minutes"),
        ("ml1066", "zone"),
        ("ml1067", "in-effect"),
    ];
    let spec = if dst { DST } else { SNTP };
    let mut values = Fields::default();
    for cell in page.select(&css(".hid_info_cell")) {
        if let Some((_, key)) = spec.iter().find(|(label, _)| {
            cell.select(&css(".hid_info_title span"))
                .any(|x| body(x) == *label)
        }) {
            let val = one(cell, ".hid_info_title + div")?;
            let text = body(val);
            let text = match text.as_str() {
                "ml1068" => "recurring",
                "ml1069" => "recurring-eu",
                "ml1070" => "recurring-us",
                "ml1071" => "once",
                "ml1016" => "unicast and broadcast",
                x => x,
            };
            if values.0.insert((*key).into(), text.into()).is_some() {
                return Err(Error::Page("duplicate time status field"));
            }
        }
    }
    if values.0.len() != spec.len() {
        return Err(Error::Page("incomplete time status panel"));
    }
    Ok(values)
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Server {
    address: String,
    port: u16,
    version: u8,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Servers {
    pub servers: BTreeMap<u8, Server>,
    #[serde(skip)]
    indices: BTreeMap<u8, u8>,
    #[serde(skip)]
    types: BTreeMap<String, String>,
}
impl fmt::Display for Servers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "PRIORITY  ADDRESS  PORT  VERSION")?;
        for (priority, s) in &self.servers {
            writeln!(
                f,
                "{priority:>8}  {}  {}  {}",
                crate::printable(&s.address),
                s.port,
                s.version
            )?;
        }
        Ok(())
    }
}
fn address_kind(v: &str) -> Result<&'static str> {
    if v.parse::<Ipv4Addr>().is_ok() {
        return Ok("IPv4");
    }
    if v.is_empty()
        || v.len() > 63
        || v.bytes().all(|c| c.is_ascii_digit() || c == b'.')
        || !v.split('.').all(|s| {
            !s.is_empty()
                && !s.starts_with('-')
                && !s.ends_with('-')
                && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
    {
        return Err(Error::Usage("invalid SNTP IPv4 address or DNS name"));
    }
    Ok("DNS")
}
fn server_table(page: &Html) -> Result<Servers> {
    let root = page.root_element();
    let table = one(root, "#sntpServerList ul.tab-init")?;
    let mut s = Servers {
        servers: BTreeMap::new(),
        indices: BTreeMap::new(),
        types: BTreeMap::new(),
    };
    for li in root.select(&css("#serverTypeMenu li")) {
        let typ = body(one(li, "a")?);
        let raw = input(li, "input.hidVal")?;
        if !matches!(typ.as_str(), "IPv4" | "DNS")
            || raw.parse::<u8>().is_err()
            || s.types.insert(typ, raw.into()).is_some()
        {
            return Err(Error::Page("server address-type menu"));
        }
    }
    for r in table.select(&css("li")) {
        let h = one(r, ".li-header")?;
        let cols = h
            .select(&css(":scope > span"))
            .map(body)
            .collect::<Vec<_>>();
        let [kind, address, port, priority, version] = cols.as_slice() else {
            return Err(Error::Page("SNTP server columns"));
        };
        if body(one(h, "span[serverPri]")?) != *priority
            || address_kind(address).ok() != Some(kind.as_str())
        {
            return Err(Error::Page("SNTP server row"));
        }
        let p: u8 = priority
            .parse()
            .ok()
            .filter(|v| (1..=3).contains(v))
            .ok_or(Error::Page("SNTP priority"))?;
        let btn = one(r, "button[onclick]")?;
        let id = attr(btn, "onclick")?
            .trim_end_matches(';')
            .strip_prefix("delSntpServer(")
            .and_then(|v| v.strip_suffix(')'))
            .and_then(|v| v.parse::<u8>().ok())
            .ok_or(Error::Page("SNTP row index"))?;
        if s.indices.values().any(|v| *v == id) || s.indices.insert(p, id).is_some() {
            return Err(Error::Page("duplicate SNTP row"));
        }
        s.servers.insert(
            p,
            Server {
                address: address.clone(),
                port: port.parse().map_err(|_| Error::Page("SNTP port"))?,
                version: version
                    .parse()
                    .ok()
                    .filter(|v| (1..=4).contains(v))
                    .ok_or(Error::Page("SNTP version"))?,
            },
        );
    }
    Ok(s)
}
#[derive(Clone, PartialEq, Eq)]
struct Dst {
    mode: DstMode,
    zone: u8,
    options: BTreeMap<String, String>,
    values: BTreeMap<String, u16>,
    choices: BTreeMap<String, Vec<u16>>,
}
fn dst_parts(mode: DstMode) -> &'static [(&'static str, &'static str)] {
    match mode {
        DstMode::Recurring => &[
            ("Week", "WEEK"),
            ("WeekDay", "WEEKDAY"),
            ("Month", "MONTH"),
            ("Hours", "HOURS"),
            ("Mins", "MINS"),
        ],
        DstMode::Once => &[
            ("Year", "YEAR"),
            ("Month", "MONTH"),
            ("Date", "DATE"),
            ("Hours", "HOURS"),
            ("Mins", "MINS"),
        ],
        _ => &[],
    }
}
fn dst_keys(mode: DstMode, end: bool) -> impl Iterator<Item = (String, String)> {
    let prefix = if mode == DstMode::Once {
        "nonRec"
    } else {
        "rec"
    };
    let side = if end { "End" } else { "Begin" };
    let post = if end { "END" } else { "START" };
    dst_parts(mode).iter().map(move |(suffix, key)| {
        (
            format!("{prefix}{side}{suffix}Hid"),
            format!("DST_{post}_{key}"),
        )
    })
}
fn dst_config(page: &Html) -> Result<Dst> {
    let root = page.root_element();
    let mode = DstMode::from_id(attr(one(root, "input[name=dstType][checked]")?, "id")?)?;
    let mut cfg = Dst {
        mode,
        zone: input(root, "#timezoneHid")?
            .parse()
            .ok()
            .filter(|v| (1..=98).contains(v))
            .ok_or(Error::Page("time zone index"))?,
        options: BTreeMap::new(),
        values: BTreeMap::new(),
        choices: BTreeMap::new(),
    };
    for el in root.select(&css("input[name=dstType]")) {
        let id = attr(el, "id")?;
        let val = attr(el, "value")?;
        DstMode::from_id(id)?;
        if val.parse::<u8>().is_err()
            || cfg.options.values().any(|v| v == val)
            || cfg.options.insert(id.into(), val.into()).is_some()
        {
            return Err(Error::Page("DST mode options"));
        }
    }
    for rule in [DstMode::Recurring, DstMode::Once] {
        for end in [false, true] {
            for (key, _) in dst_keys(rule, end) {
                if let Some(el) = root.select(&css(&format!("input#{key}"))).next() {
                    let menu = key.trim_end_matches("Hid").to_owned() + "Menu";
                    let mut choice = root
                        .select(&css(&format!("#{menu} li input.hidVal")))
                        .map(|v| {
                            attr(v, "value")?
                                .parse::<u16>()
                                .map_err(|_| Error::Page("DST menu value"))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    if choice.is_empty() {
                        // Some menus are populated from numeric bounds embedded by the device.
                        let prefix = format!("generateDropdownMenu('{menu}',");
                        for script in root.select(&css("script")) {
                            let text = body(script);
                            if let Some((_, suffix)) = text.split_once(&prefix)
                                && let Some((args, _)) = suffix.split_once(')')
                                && let Some((lo, hi)) = args.split_once(',')
                            {
                                let lo = lo
                                    .trim()
                                    .parse::<u16>()
                                    .map_err(|_| Error::Page("DST menu lower bound"))?;
                                let hi = hi
                                    .trim()
                                    .parse::<u16>()
                                    .map_err(|_| Error::Page("DST menu upper bound"))?;
                                if hi < lo || hi - lo > 200 {
                                    return Err(Error::Page("unrecognized DST menu range"));
                                }
                                choice.extend(lo..=hi);
                            }
                        }
                    }
                    cfg.choices.insert(key.clone(), choice);
                    cfg.values.insert(
                        key,
                        attr(el, "value")?
                            .parse()
                            .map_err(|_| Error::Page("DST rule field"))?,
                    );
                }
            }
        }
    }
    Ok(cfg)
}
impl Dst {
    fn displayed(&self) -> Result<Fields> {
        let mut map = Fields::default();
        map.0.insert(
            "mode".into(),
            self.mode.to_possible_value().unwrap().get_name().into(),
        );
        if self.mode != DstMode::Disabled {
            map.0.insert("zone".into(), self.zone.to_string());
        }
        for end in [false, true] {
            for (id, wire) in dst_keys(self.mode, end) {
                map.0.insert(
                    wire.to_ascii_lowercase().replace('_', "-"),
                    self.values
                        .get(&id)
                        .ok_or(Error::Page("missing active DST entry"))?
                        .to_string(),
                );
            }
        }
        Ok(map)
    }
}
fn calendar(y: u16, m: u16, d: u16) -> Result<i64> {
    let leap = |n: u16| n.is_multiple_of(4) && (!n.is_multiple_of(100) || n.is_multiple_of(400));
    let limit = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap(y) => 29,
        2 => 28,
        _ => 0,
    };
    if !(1970..=9999).contains(&y) || d == 0 || d > limit {
        return Err(Error::Usage("invalid calendar date"));
    }
    let n = i64::from(y - 1);
    let offset = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334][usize::from(m - 1)];
    Ok(n * 365 + n / 4 - n / 100 + n / 400 + offset + i64::from(m > 2 && leap(y)) + i64::from(d))
}
fn parts(s: &str, sep: char, lengths: &[usize]) -> Result<Vec<u16>> {
    let words: Vec<_> = s.split(sep).collect();
    if words.len() != lengths.len() {
        return Err(Error::Usage("date or time has the wrong format"));
    }
    words
        .iter()
        .zip(lengths)
        .map(|(w, n)| {
            if w.len() == *n && w.bytes().all(|c| c.is_ascii_digit()) {
                w.parse().map_err(|_| Error::Usage("invalid date or time"))
            } else {
                Err(Error::Usage("date or time has the wrong format"))
            }
        })
        .collect()
}
fn timestamp(date: &str, time: &str) -> Result<i64> {
    let d = parts(date, '-', &[4, 2, 2])?;
    let t = parts(time, ':', &[2, 2, 2])?;
    if t[0] > 23 || t[1] > 59 || t[2] > 59 {
        return Err(Error::Usage("invalid clock time"));
    }
    Ok(calendar(d[0], d[1], d[2])? * 86400
        + i64::from(t[0]) * 3600
        + i64::from(t[1]) * 60
        + i64::from(t[2]))
}
fn dst_values(mode: DstMode, raw: &str) -> Result<Vec<u16>> {
    if mode == DstMode::Once {
        let (date, time) = raw
            .split_once('T')
            .ok_or(Error::Usage("expected YYYY-MM-DDTHH:MM"))?;
        timestamp(date, &format!("{time}:00"))?;
        let mut v = parts(date, '-', &[4, 2, 2])?;
        v.extend(parts(time, ':', &[2, 2])?);
        return Ok(v);
    }
    let v = raw
        .split(',')
        .map(|s| {
            s.parse::<u16>()
                .map_err(|_| Error::Usage("invalid recurring DST rule"))
        })
        .collect::<Result<Vec<_>>>()?;
    if v.len() != 5
        || !(1..=5).contains(&v[0])
        || v[1] > 7
        || !(1..=12).contains(&v[2])
        || v[3] > 23
        || v[4] > 59
    {
        return Err(Error::Usage("invalid week, weekday, month, hour or minute"));
    }
    Ok(v)
}
fn clock_page(page: &Html) -> Result<Fields> {
    let root = page.root_element();
    let mut map = Fields::default();
    for (k, sel) in [
        ("date", "#systemDate"),
        ("time", "#systemTime"),
        ("zone", "#timezoneHid"),
    ] {
        map.0.insert(k.into(), input(root, sel)?.into());
    }
    let check = one(root, "input[type=checkbox]#sntpAdmin")?;
    map.0.insert(
        "source".into(),
        if check.attr("checked").is_some() {
            "sntp"
        } else {
            "manual"
        }
        .into(),
    );
    Ok(map)
}
impl Action {
    fn clock_change(
        session: &Session<'_>,
        apply: bool,
        source: Source,
        date: Option<&str>,
        time: Option<&str>,
        zone: Option<u8>,
    ) -> Result<Change> {
        let manual = source == Source::Manual;
        if manual != (date.is_some() && time.is_some()) {
            return Err(Error::Usage(
                "manual requires both --date and --time; SNTP supplies its own clock",
            ));
        }
        let read = || session.read(Page::Clock);
        let first = read()?;
        let before = clock_page(&first)?;
        token(&first)?;
        let seconds = if manual {
            Some(timestamp(date.unwrap(), time.unwrap())?)
        } else {
            None
        };
        let name = if manual { "manual" } else { "sntp" };
        let target = zone
            .map(|z| z.to_string())
            .unwrap_or_else(|| before.0["zone"].clone());
        let mut outcome = "preview";
        if apply {
            let fresh = read()?;
            let state = clock_page(&fresh)?;
            if state.0["zone"] != before.0["zone"] || state.0["source"] != before.0["source"] {
                return Err(Error::Usage("clock settings changed during preflight"));
            }
            let started = Instant::now();
            session.submit(
                "/sntpSystemTime.cgi",
                &[
                    field("sntpAdmin", !manual),
                    field("systemDate", date.unwrap_or(&state.0["date"])),
                    field("systemTime", time.unwrap_or(&state.0["time"])),
                    field("timeZone", &target),
                    field("hash", token(&fresh)?),
                ],
            )?;
            let now = read()
                .and_then(|p| clock_page(&p))
                .map_err(|_| Error::Unverified)?;
            if now.0["source"] != name || now.0["zone"] != target {
                return Err(Error::Unverified);
            }
            if let Some(start) = seconds {
                let t = timestamp(&now.0["date"], &now.0["time"]).map_err(|_| Error::Unverified)?;
                if !(0..=started.elapsed().as_secs() as i64 + 1).contains(&(t - start)) {
                    return Err(Error::Unverified);
                }
            }
            outcome = "verified";
        }
        Ok(Change {
            operation: "set-clock",
            outcome,
            details: format!(
                "source {name}, zone {target} {} {}",
                date.unwrap_or(""),
                time.unwrap_or("")
            )
            .trim()
            .into(),
        })
    }
    pub fn run(&self, session: &Session<'_>, apply: bool) -> Result<Report> {
        match self {
            Self::Clock => Ok(Report::Time(clock_page(&session.read(Page::Clock)?)?)),
            Self::SetClock {
                source,
                date,
                time,
                zone,
            } => Ok(Report::Change(Self::clock_change(
                session,
                apply,
                *source,
                date.as_deref(),
                time.as_deref(),
                *zone,
            )?)),
            Self::Status | Self::DstStatus => {
                let dst = matches!(self, Self::DstStatus);
                Ok(Report::Time(status(
                    &session.read(Page::TimeTab(u8::from(dst)))?,
                    dst,
                )?))
            }
            Self::Dst => Ok(Report::Time(
                dst_config(&session.read(Page::TimeTab(4))?)?.displayed()?,
            )),
            Self::Servers => Ok(Report::TimeServers(server_table(
                &session.read(Page::TimeTab(3))?,
            )?)),
            Self::SetDst {
                mode,
                zone,
                start,
                end,
            } => {
                let custom = matches!(mode, DstMode::Once | DstMode::Recurring);
                if custom != (start.is_some() && end.is_some())
                    || *mode == DstMode::Disabled && zone.is_some()
                {
                    return Err(Error::Usage(
                        "custom rules require start and end; disabled takes no zone",
                    ));
                }
                let change = admin::change(
                    session,
                    Page::TimeTab(4),
                    apply,
                    dst_config,
                    |s| {
                        let mut next = s.clone();
                        next.mode = *mode;
                        next.zone = zone.unwrap_or(s.zone);
                        let mut fields = vec![field(
                            "DST_TYPE",
                            s.options
                                .get(mode.id())
                                .ok_or(Error::Page("target DST mode unavailable"))?,
                        )];
                        if *mode != DstMode::Disabled {
                            fields.push(field("TIMEZONE_INDEX", next.zone));
                        }
                        if custom {
                            let start = dst_values(*mode, start.as_deref().unwrap())?;
                            let end = dst_values(*mode, end.as_deref().unwrap())?;
                            if *mode == DstMode::Once && start >= end {
                                return Err(Error::Usage("end must follow start"));
                            }
                            for (is_end, v) in [(false, start), (true, end)] {
                                for ((id, key), value) in dst_keys(*mode, is_end).zip(v) {
                                    if !s.values.contains_key(&id) {
                                        return Err(Error::Page("custom DST input unavailable"));
                                    }
                                    if s.values.get(&id) != Some(&value)
                                        && !s.choices.get(&id).is_some_and(|v| v.contains(&value))
                                    {
                                        return Err(Error::Usage(
                                            "custom DST value is unavailable in the switch menu",
                                        ));
                                    }
                                    fields.push(field(&key, value));
                                    next.values.insert(id, value);
                                }
                            }
                        }
                        let changed = next.displayed()? != s.displayed()?;
                        Ok(Plan {
                            operation: "set-dst",
                            details: format!("{} rule; zone {}", mode.id(), next.zone),
                            expected: next,
                            submission: changed.then_some(Submission {
                                path: "/sntpDstConfig.cgi",
                                fields,
                            }),
                        })
                    },
                    |a, b| a.displayed().ok() == b.displayed().ok(),
                )?;
                Ok(Report::Change(change))
            }
            Self::ServerAdd { .. } | Self::ServerDelete { .. } => {
                let change = admin::change(
                    session,
                    Page::TimeTab(3),
                    apply,
                    server_table,
                    |s| {
                        let mut next = s.clone();
                        let (op, detail, fields) = match self {
                            Self::ServerAdd {
                                address,
                                priority,
                                port,
                                version,
                            } => {
                                if *port != 123 && !(1025..=36564).contains(port) {
                                    return Err(Error::Usage("invalid SNTP server port"));
                                }
                                let kind = address_kind(address)?;
                                let code = s
                                    .types
                                    .get(kind)
                                    .ok_or(Error::Page("server address type unavailable"))?;
                                if s.servers.contains_key(priority)
                                    || s.servers
                                        .values()
                                        .any(|v| v.address.eq_ignore_ascii_case(address))
                                {
                                    return Err(Error::Usage(
                                        "duplicate server address or priority",
                                    ));
                                }
                                next.servers.insert(
                                    *priority,
                                    Server {
                                        address: address.clone(),
                                        port: *port,
                                        version: *version,
                                    },
                                );
                                (
                                    "sntp-server-add",
                                    format!("{address}, priority {priority}"),
                                    vec![
                                        field("ACTION", "ADD"),
                                        field("SERVER_ADDR_TYPE", code),
                                        field("SERVER_ADDRESS", address),
                                        field("SERVER_PORT", port),
                                        field("SERVER_PRIORITY", priority),
                                        field("SERVER_VERSION", version),
                                    ],
                                )
                            }
                            Self::ServerDelete { priority, address } => {
                                if !s.servers.get(priority).is_some_and(|server| {
                                    server.address.eq_ignore_ascii_case(address)
                                }) {
                                    return Err(Error::Usage(
                                        "address does not match the server at this priority",
                                    ));
                                }
                                next.servers.remove(priority);
                                (
                                    "sntp-server-delete",
                                    format!("{address}, priority {priority}"),
                                    vec![
                                        field("ACTION", "DELETE"),
                                        field("SERVER_INDEX", s.indices[priority]),
                                    ],
                                )
                            }
                            _ => unreachable!(),
                        };
                        Ok(Plan {
                            operation: op,
                            details: detail,
                            expected: next,
                            submission: Some(Submission {
                                path: "/sntpServerConfig.cgi",
                                fields,
                            }),
                        })
                    },
                    |a, b| a.servers == b.servers,
                )?;
                Ok(Report::Change(change))
            }
        }
    }
}
