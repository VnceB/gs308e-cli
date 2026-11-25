use crate::admin::{self, Fields, Plan, Submission, attr, body, css, field, input, one};
use crate::error::{Error, Result};
use crate::session::{Page, Session};
use clap::{Subcommand, ValueEnum};
use scraper::{ElementRef, Html};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Disabled,
    BasicPort,
    AdvancedPort,
    BasicDot1q,
    AdvancedDot1q,
}
impl Mode {
    fn names(self) -> (&'static str, &'static str, u8) {
        match self {
            Self::Disabled => ("noVlan", "disabled", 0),
            Self::BasicPort => ("bscPotBsd", "basic_port_based", 1),
            Self::AdvancedPort => ("advPotBsd", "advanced_port_based", 2),
            Self::BasicDot1q => ("bsc8021Q", "basic_802_1q", 3),
            Self::AdvancedDot1q => ("adv8021Q", "advanced_802_1q", 4),
        }
    }
    fn limit(self) -> u16 {
        match self {
            Self::BasicPort | Self::AdvancedPort => 8,
            Self::BasicDot1q => 4093,
            Self::AdvancedDot1q => 4094,
            Self::Disabled => 0,
        }
    }
    fn dot1q(self) -> bool {
        matches!(self, Self::BasicDot1q | Self::AdvancedDot1q)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Member {
    Included,
    Tagged,
    Untagged,
}
impl Member {
    fn label(self) -> &'static str {
        match self {
            Self::Included => "included",
            Self::Tagged => "tagged",
            Self::Untagged => "untagged",
        }
    }
}
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize)]
struct Row {
    name: String,
    members: BTreeMap<u8, Member>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Table {
    mode: Mode,
    vlans: BTreeMap<u16, Row>,
    assignments: BTreeMap<u8, String>,
    pvids: BTreeMap<u8, u16>,
    management_vlan: Option<u16>,
    voice_vlan: Option<u16>,
    voice_cos: Option<u8>,
    ouis: BTreeMap<String, String>,
    #[serde(skip)]
    oui_controls: BTreeMap<String, String>,
}
impl fmt::Display for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "VLAN mode   {}", self.mode.names().1)?;
        for (id, row) in &self.vlans {
            let mut ports = String::new();
            for (p, m) in &row.members {
                write!(ports, " {}:{}", p, m.label())?;
            }
            writeln!(f, "VLAN {id:>4}  {}{ports}", crate::printable(&row.name))?;
        }
        for (p, id) in &self.assignments {
            writeln!(f, "Port {p}  VLAN {id}")?;
        }
        for (p, id) in &self.pvids {
            writeln!(f, "Port {p}  PVID {id}")?;
        }
        if let Some(id) = self.management_vlan {
            writeln!(f, "Management VLAN  {id}")?;
        }
        if let Some(id) = self.voice_vlan {
            writeln!(
                f,
                "Voice VLAN  {id}  CoS {}",
                self.voice_cos.unwrap_or_default()
            )?;
        }
        for (prefix, desc) in &self.ouis {
            writeln!(f, "OUI {prefix}  {}", crate::printable(desc))?;
        }
        Ok(())
    }
}
fn vid(s: &str, max: u16) -> Result<u16> {
    s.trim()
        .parse::<u16>()
        .ok()
        .filter(|n| (1..=max).contains(n))
        .ok_or(Error::Page("VLAN ID"))
}
fn unique<K: Ord, V>(m: &mut BTreeMap<K, V>, k: K, v: V) -> Result<()> {
    if m.insert(k, v).is_some() {
        return Err(Error::Page("duplicate VLAN table entry"));
    }
    Ok(())
}
fn eight<T>(m: &BTreeMap<u8, T>) -> Result<()> {
    if m.keys().copied().ne(1..=8) {
        return Err(Error::Page("expected eight physical port entries"));
    }
    Ok(())
}
fn oui_key(v: &str) -> Result<String> {
    let n = v.replace('-', ":").to_ascii_uppercase();
    if n.len() != 8
        || !n
            .split(':')
            .all(|b| b.len() == 2 && b.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(Error::Usage("use a three-byte OUI such as 02:00:00"));
    }
    Ok(n)
}
impl Table {
    pub fn parse(page: &Html) -> Result<Self> {
        let mode_name = crate::model::vlan(page)?.mode;
        let mode = Mode::value_variants()
            .iter()
            .copied()
            .find(|m| m.names().1 == mode_name)
            .ok_or(Error::Page("VLAN mode"))?;
        let mut s = Self {
            mode,
            vlans: BTreeMap::new(),
            assignments: BTreeMap::new(),
            pvids: BTreeMap::new(),
            management_vlan: None,
            voice_vlan: None,
            voice_cos: None,
            ouis: BTreeMap::new(),
            oui_controls: BTreeMap::new(),
        };
        if mode == Mode::Disabled {
            return Ok(s);
        }
        let area = one(
            page.root_element(),
            &format!("section[vlan-page='{}']", mode.names().0),
        )?;
        if mode != Mode::BasicPort {
            s.read_rows(area)?;
        }
        if matches!(mode, Mode::BasicPort | Mode::BasicDot1q) {
            s.read_assignments(area)?;
        }
        if mode.dot1q() {
            if area.select(&css("input[name=mgmtVlan]")).next().is_some() {
                let id = vid(input(area, "input[name=mgmtVlan]")?, mode.limit())?;
                if !s.vlans.contains_key(&id) {
                    return Err(Error::Page("unknown management VLAN"));
                }
                s.management_vlan = Some(id);
            }
            let count = input(area, "input#vlanNum")?
                .parse::<usize>()
                .map_err(|_| Error::Page("VLAN count"))?;
            if count != s.vlans.len() {
                return Err(Error::Page("inconsistent VLAN count"));
            }
        }
        if mode == Mode::AdvancedDot1q {
            s.read_voice_and_pvids(area)?;
        }
        Ok(s)
    }
    fn read_rows(&mut self, area: ElementRef<'_>) -> Result<()> {
        let (n, bind) = match self.mode {
            Mode::AdvancedPort => (2, "btns-2"),
            Mode::BasicDot1q => (3, "btns-3"),
            Mode::AdvancedDot1q => (4, "btns-4"),
            _ => return Err(Error::Page("VLAN group table")),
        };
        let list = one(area, &format!("ul[bind='{bind}']"))?;
        for el in list.select(&css("li")) {
            let id = vid(&body(one(el, &format!("[vid-{n}]"))?), self.mode.limit())?;
            let name = body(one(el, &format!("[vnm-{n}]"))?);
            let mut row = Row {
                name,
                members: BTreeMap::new(),
            };
            if self.mode != Mode::BasicDot1q {
                let mask = input(el, "input[hidden-mem]")?;
                if mask.len() != 8 {
                    return Err(Error::Page("VLAN member bitmap length"));
                }
                for (p, d) in (1..=8).zip(mask.bytes()) {
                    let member = match (self.mode, d) {
                        (Mode::AdvancedPort, b'1') => Member::Included,
                        (Mode::AdvancedDot1q, b'1') => Member::Tagged,
                        (Mode::AdvancedDot1q, b'2') => Member::Untagged,
                        (Mode::AdvancedPort, b'0') | (Mode::AdvancedDot1q, b'3') => continue,
                        _ => return Err(Error::Page("VLAN member bitmap")),
                    };
                    row.members.insert(p, member);
                }
            }
            unique(&mut self.vlans, id, row)?;
        }
        if !self.vlans.contains_key(&1) {
            return Err(Error::Page("default VLAN missing"));
        }
        Ok(())
    }
    fn read_assignments(&mut self, area: ElementRef<'_>) -> Result<()> {
        let basic = self.mode == Mode::BasicPort;
        let key = if basic { "name-1" } else { "name-3" };
        for el in area.select(&css(&format!("li[{key}]"))) {
            let p = admin::numbers(
                attr(el, key)?
                    .strip_prefix("port")
                    .ok_or(Error::Page("VLAN port key"))?,
            )?;
            let mut v = if basic {
                body(one(el, "[vid-1]")?)
            } else {
                match attr(el, "pot-mod")? {
                    "Access" => body(one(el, "[vid-wrap=Access] [vid-str-3]")?)
                        .split_once(" - ")
                        .map(|v| v.0.to_owned())
                        .ok_or(Error::Page("access VLAN"))?,
                    "Trunk" => attr(one(el, "[vid-wrap=Trunk] [vid-str-3]")?, "real-val")?.into(),
                    _ => return Err(Error::Page("802.1Q port mode")),
                }
            };
            if v.eq_ignore_ascii_case("all") {
                v = "ALL".into()
            } else {
                let id = vid(&v, self.mode.limit())?;
                v = id.to_string();
                if !basic && !self.vlans.contains_key(&id) {
                    return Err(Error::Page("unknown access VLAN"));
                }
            }
            unique(&mut self.assignments, p, v)?;
        }
        eight(&self.assignments)
    }
    fn read_voice_and_pvids(&mut self, area: ElementRef<'_>) -> Result<()> {
        let voice = input(area, "input#voVid")?;
        if voice != "0" {
            let id = vid(voice, 4094)?;
            if !self.vlans.contains_key(&id) {
                return Err(Error::Page("unknown voice VLAN"));
            }
            self.voice_vlan = Some(id);
        }
        self.voice_cos = Some(
            body(one(area, "#cos")?)
                .parse::<u8>()
                .ok()
                .filter(|v| *v < 8)
                .ok_or(Error::Page("voice class of service"))?,
        );
        let list = one(area, "ul[bind=pvid-tab]")?;
        for row in list.select(&css("li")) {
            let p = admin::numbers(&body(one(row, ".pot-num")?))?;
            let text = body(one(row, "[pvid-str]")?);
            let mut current = None;
            for frag in text.split(',').map(str::trim).filter(|x| !x.is_empty()) {
                if let Some(raw) = frag.strip_suffix('*') {
                    if current.is_some() {
                        return Err(Error::Page("ambiguous PVID"));
                    }
                    current = Some(vid(raw, 4094)?);
                }
            }
            let id = current.unwrap_or(1);
            if !self
                .vlans
                .get(&id)
                .is_some_and(|r| r.members.contains_key(&p))
            {
                return Err(Error::Page("PVID outside membership"));
            }
            unique(&mut self.pvids, p, id)?;
        }
        eight(&self.pvids)?;
        let list = one(area, "ul[bind=btns-oui-4]")?;
        for el in list.select(&css("li[oui-num]")) {
            let prefix =
                oui_key(&body(one(el, "[tele]")?)).map_err(|_| Error::Page("voice OUI prefix"))?;
            let index = attr(el, "oui-num")?;
            if !index
                .strip_prefix("INDEX_")
                .is_some_and(|n| n.parse::<u8>().is_ok())
                || self.oui_controls.values().any(|v| v == index)
            {
                return Err(Error::Page("invalid voice OUI row index"));
            }
            self.oui_controls.insert(prefix.clone(), index.into());
            unique(&mut self.ouis, prefix, body(one(el, "[desc]")?))?;
        }
        Ok(())
    }
}

#[derive(Subcommand)]
pub enum Action {
    /// Show VLANs, membership and the applicable port, PVID and voice tables
    List,
    /// Switch mode. Applying this operation discards existing VLAN configuration
    SetMode {
        mode: Mode,
        #[arg(long)]
        from: Mode,
        #[arg(long, required = true)]
        accept_vlan_reset: bool,
    },
    /// Create a VLAN in the current mode. Port lists are comma-separated
    Create {
        id: u16,
        #[arg(long, default_value = "")]
        name: String,
        #[command(flatten)]
        ports: MemberSelection,
    },
    /// Rename a VLAN without changing its membership or voice designation
    Rename { id: u16, name: String },
    /// Move the management interface to an existing 802.1Q VLAN
    Management {
        id: u16,
        #[arg(long, required = true)]
        accept_reconnect: bool,
    },
    /// Delete an unused VLAN in the current mode
    Delete { id: u16 },
    /// Replace a VLAN's membership, preserving its name and voice settings
    Members {
        id: u16,
        #[command(flatten)]
        ports: MemberSelection,
    },
    /// Assign a basic-mode port to a VLAN or to all VLANs
    Assign {
        #[arg(value_parser=clap::value_parser!(u8).range(1..=8))]
        port: u8,
        vlan: String,
    },
    /// Set the default incoming VLAN on an advanced 802.1Q member port
    Pvid {
        #[arg(value_parser=clap::value_parser!(u8).range(1..=8))]
        port: u8,
        id: u16,
    },
    /// Designate one advanced 802.1Q voice VLAN, or use "off"
    Voice {
        vlan: String,
        #[arg(long,value_parser=clap::value_parser!(u8).range(0..=7))]
        cos: Option<u8>,
    },
    /// Add a voice manufacturer prefix (three hexadecimal octets)
    OuiAdd { prefix: String, description: String },
    /// Change the description of one existing voice manufacturer prefix
    OuiEdit { prefix: String, description: String },
    /// Remove one existing voice manufacturer prefix
    OuiDelete { prefix: String },
}
#[derive(clap::Args)]
pub struct MemberSelection {
    /// Advanced port-based VLAN members
    #[arg(long,value_delimiter=',',value_parser=clap::value_parser!(u8).range(1..=8))]
    ports: Vec<u8>,
    /// Tagged members of an advanced 802.1Q VLAN
    #[arg(long,value_delimiter=',',value_parser=clap::value_parser!(u8).range(1..=8))]
    tagged: Vec<u8>,
    /// Untagged members of an advanced 802.1Q VLAN
    #[arg(long,value_delimiter=',',value_parser=clap::value_parser!(u8).range(1..=8))]
    untagged: Vec<u8>,
}
impl MemberSelection {
    fn map(&self, mode: Mode) -> Result<BTreeMap<u8, Member>> {
        let mut result = BTreeMap::new();
        for (ports, kind, allowed) in [
            (&self.ports, Member::Included, Mode::AdvancedPort),
            (&self.tagged, Member::Tagged, Mode::AdvancedDot1q),
            (&self.untagged, Member::Untagged, Mode::AdvancedDot1q),
        ] {
            for p in ports {
                if mode != allowed {
                    return Err(Error::Usage(
                        "membership option is incompatible with active VLAN mode",
                    ));
                }
                if result.insert(*p, kind).is_some() {
                    return Err(Error::Usage("each port may appear only once"));
                }
            }
        }
        if result.is_empty() && mode != Mode::BasicDot1q {
            return Err(Error::Usage("supply at least one VLAN member"));
        }
        Ok(result)
    }
}

impl Action {
    pub fn check_protected(&self, protected: &[u8]) -> Result<()> {
        let touches_protected = match self {
            Self::List
            | Self::Rename { .. }
            | Self::Delete { .. }
            | Self::Voice { .. }
            | Self::OuiAdd { .. }
            | Self::OuiEdit { .. }
            | Self::OuiDelete { .. } => false,
            Self::Assign { port, .. } | Self::Pvid { port, .. } => protected.contains(port),
            Self::Create { ports, .. } => ports
                .ports
                .iter()
                .chain(&ports.tagged)
                .chain(&ports.untagged)
                .any(|p| protected.contains(p)),
            Self::Members { .. } => false,
            Self::SetMode { .. } | Self::Management { .. } => true,
        };
        if touches_protected {
            return Err(Error::Usage(
                "VLAN changes can affect protected forwarding paths",
            ));
        }
        Ok(())
    }
    pub fn run(
        &self,
        session: &Session<'_>,
        apply: bool,
        protected: &[u8],
    ) -> Result<admin::Change> {
        admin::change(
            session,
            Page::Vlan,
            apply,
            |page| {
                if let Self::SetMode { mode, .. } = self {
                    let (name, _, number) = mode.names();
                    let button = one(page.root_element(), &format!("button[mod-num='{number}']"))?;
                    if button.attr("name") != Some(name)
                        || button.attr("disabled").is_some()
                            && crate::model::vlan(page)?.mode != mode.names().1
                    {
                        return Err(Error::Page("target VLAN mode control unavailable"));
                    }
                }
                Table::parse(page)
            },
            |s| {
                let plan = self.plan(s)?;
                if apply
                    && !protected.is_empty()
                    && let Self::Members { id, .. } = self
                    && let (Some(old), Some(new)) = (s.vlans.get(id), plan.expected.vlans.get(id))
                    && protected
                        .iter()
                        .any(|p| old.members.get(p) != new.members.get(p))
                {
                    return Err(Error::Usage(
                        "VLAN changes can affect protected forwarding paths",
                    ));
                }
                if plan.submission.is_some() {
                    if matches!(
                        self,
                        Self::Assign { .. }
                            | Self::Members { .. }
                            | Self::Create { .. }
                            | Self::Pvid { .. }
                    ) {
                        for group in crate::settings::active_lags(session)? {
                            plan.expected.check_lag(&group)?;
                        }
                    }
                    if let Self::Delete { id } = self
                        && s.mode.dot1q()
                    {
                        let feature = crate::settings::Feature::Multicast;
                        let cfg = feature.read(&session.read(feature.page())?, None)?;
                        if cfg.values.get("vlan") == Some(&id.to_string()) {
                            return Err(Error::Usage("VLAN is referenced by multicast settings"));
                        }
                    }
                }
                Ok(plan)
            },
            |expected, current| {
                if matches!(self, Self::SetMode { .. }) {
                    expected.mode == current.mode
                } else {
                    let mut expected = expected.clone();
                    expected.oui_controls.clone_from(&current.oui_controls);
                    expected == *current
                }
            },
        )
    }
    fn plan(&self, s: &Table) -> Result<Plan<Table>> {
        let mut next = s.clone();
        let mode = s.mode;
        let mut fields = Fields::new();
        let (op, path, details) = match self {
            Self::List => unreachable!("read-only list"),
            Self::SetMode {
                mode: target, from, ..
            } => {
                if mode != *from {
                    return Err(Error::Usage("active VLAN mode differs from --from"));
                }
                next.mode = *target;
                fields.push(field("VLAN_MOD", target.names().2));
                (
                    "vlan-set-mode",
                    "/vlanMod.cgi",
                    format!(
                        "{} -> {} (device-wide VLAN reset)",
                        from.names().1,
                        target.names().1
                    ),
                )
            }
            Self::Assign { port, vlan } => {
                if !matches!(mode, Mode::BasicPort | Mode::BasicDot1q) {
                    return Err(Error::Usage(
                        "assign requires basic port or basic 802.1Q mode",
                    ));
                }
                let v = if vlan.eq_ignore_ascii_case("all") {
                    "ALL".into()
                } else {
                    let id = vid(vlan, mode.limit())
                        .map_err(|_| Error::Usage("invalid VLAN assignment"))?;
                    if mode.dot1q() && !s.vlans.contains_key(&id) {
                        return Err(Error::Usage("VLAN does not exist"));
                    }
                    id.to_string()
                };
                next.assignments.insert(*port, v.clone());
                if mode.dot1q() {
                    fields.push(field("page", "portCfg"));
                }
                for (p, id) in &next.assignments {
                    fields.push(field(&format!("port{p}"), id));
                }
                (
                    "vlan-assign",
                    if mode.dot1q() {
                        "/8021qBasic.cgi"
                    } else {
                        "/portBasedBasic.cgi"
                    },
                    format!("port {port} -> {v}"),
                )
            }
            Self::Pvid { port, id } => {
                if mode != Mode::AdvancedDot1q
                    || !s
                        .vlans
                        .get(id)
                        .is_some_and(|r| r.members.contains_key(port))
                {
                    return Err(Error::Usage(
                        "PVID requires an existing advanced 802.1Q membership",
                    ));
                }
                next.pvids.insert(*port, *id);
                fields.extend([field("PORT", port), field("PVID", id)]);
                ("vlan-pvid", "/portPVID.cgi", format!("port {port} -> {id}"))
            }
            Self::Rename { id, name } => {
                admin::label(name, 14)?;
                let row = next
                    .vlans
                    .get_mut(id)
                    .ok_or(Error::Usage("VLAN does not exist in this mode"))?;
                row.name.clone_from(name);
                let (path, payload) = next.row_form(*id, s.vlans.len(), false)?;
                fields = payload;
                ("vlan-rename", path, format!("VLAN {id} -> {name}"))
            }
            Self::Management { id, .. } => {
                if s.management_vlan.is_none() || !s.vlans.contains_key(id) {
                    return Err(Error::Usage(
                        "management VLAN requires an available control and an existing VLAN",
                    ));
                }
                let path = match mode {
                    Mode::BasicDot1q => "/8021qBasic.cgi",
                    Mode::AdvancedDot1q => "/8021qAdvanced.cgi",
                    _ => return Err(Error::Usage("management VLAN requires 802.1Q")),
                };
                next.management_vlan = Some(*id);
                fields.extend([field("ACTION", "vlanMgmt"), field("mgmtVlanId", id)]);
                (
                    "vlan-management",
                    path,
                    format!("management VLAN -> {id}; the current management path may disconnect"),
                )
            }
            Self::Delete { id } => {
                if *id == 1
                    || !s.vlans.contains_key(id)
                    || s.management_vlan == Some(*id)
                    || s.voice_vlan == Some(*id)
                    || s.pvids.values().any(|v| v == id)
                    || s.assignments.values().any(|v| v == &id.to_string())
                {
                    return Err(Error::Usage(
                        "VLAN is absent, reserved, or referenced by an assignment, PVID, voice or management",
                    ));
                }
                let path = match mode {
                    Mode::AdvancedPort => {
                        fields.push(field("hiddenMem", "00000000"));
                        "/portBasedAdvanced.cgi"
                    }
                    Mode::BasicDot1q => {
                        fields.extend([field("Action", "Delete"), field("page", "vlanCfg")]);
                        "/8021qBasic.cgi"
                    }
                    Mode::AdvancedDot1q => {
                        fields.push(field("ACTION", "Delete"));
                        "/8021qAdvanced.cgi"
                    }
                    _ => return Err(Error::Usage("current mode has no deletable VLAN table")),
                };
                fields.push(field("VLAN_ID", id));
                next.vlans.remove(id);
                ("vlan-delete", path, format!("VLAN {id}"))
            }
            Self::Create { id, .. } | Self::Members { id, .. } => {
                let creating = matches!(self, Self::Create { .. });
                if *id == 0
                    || *id > mode.limit()
                    || !matches!(
                        mode,
                        Mode::AdvancedPort | Mode::AdvancedDot1q | Mode::BasicDot1q
                    )
                {
                    return Err(Error::Usage(
                        "VLAN ID or operation is invalid for the active mode",
                    ));
                }
                let (name, members) = match self {
                    Self::Create { name, ports, .. } => {
                        if s.vlans.contains_key(id) {
                            return Err(Error::Usage("VLAN already exists"));
                        }
                        let capacity = if mode == Mode::AdvancedDot1q { 64 } else { 8 };
                        if s.vlans.len() >= capacity {
                            return Err(Error::Usage("VLAN table is full"));
                        }
                        admin::label(name, 14)?;
                        (name.clone(), ports.map(mode)?)
                    }
                    Self::Members { ports, .. } => {
                        if mode == Mode::BasicDot1q {
                            return Err(Error::Usage("use vlan assign in basic 802.1Q mode"));
                        }
                        let row = s.vlans.get(id).ok_or(Error::Usage("VLAN does not exist"))?;
                        (row.name.clone(), ports.map(mode)?)
                    }
                    _ => unreachable!(),
                };
                if s.pvids
                    .iter()
                    .any(|(p, v)| v == id && !members.contains_key(p))
                {
                    return Err(Error::Usage(
                        "reassign the port PVID before removing its membership",
                    ));
                }
                if !creating && s.management_vlan == Some(*id) && members.is_empty() {
                    return Err(Error::Usage(
                        "management VLAN requires at least one member port",
                    ));
                }
                next.vlans.insert(*id, Row { name, members });
                let (path, payload) = next.row_form(*id, s.vlans.len(), creating)?;
                fields = payload;
                (
                    if creating {
                        "vlan-create"
                    } else {
                        "vlan-members"
                    },
                    path,
                    format!("VLAN {id}"),
                )
            }
            Self::Voice { vlan, cos } => {
                if mode != Mode::AdvancedDot1q {
                    return Err(Error::Usage("voice VLAN requires advanced 802.1Q"));
                }
                next.voice_vlan = if vlan == "off" {
                    None
                } else {
                    Some(
                        vid(vlan, 4094)
                            .map_err(|_| Error::Usage("specify an existing VLAN or off"))?,
                    )
                };
                if next.voice_vlan.is_none() && cos.is_some() {
                    return Err(Error::Usage("--cos requires an enabled voice VLAN"));
                }
                if let Some(v) = cos {
                    next.voice_cos = Some(*v)
                }
                if let Some(id) = next.voice_vlan.or(s.voice_vlan) {
                    let (_, f) = next.row_form(id, s.vlans.len(), false)?;
                    fields = f;
                }
                (
                    "vlan-voice",
                    "/8021qAdvanced.cgi",
                    format!("voice VLAN {vlan}"),
                )
            }
            Self::OuiAdd { prefix, .. }
            | Self::OuiEdit { prefix, .. }
            | Self::OuiDelete { prefix } => {
                if mode != Mode::AdvancedDot1q {
                    return Err(Error::Usage("voice OUI requires advanced 802.1Q"));
                }
                let key = oui_key(prefix)?;
                let (action, desc) = match self {
                    Self::OuiAdd { description, .. } => {
                        if s.ouis.contains_key(&key) {
                            return Err(Error::Usage("OUI already exists"));
                        }
                        if s.ouis.len() >= 15 {
                            return Err(Error::Usage("voice OUI table is full"));
                        }
                        admin::label(description, 32)?;
                        next.ouis.insert(key.clone(), description.clone());
                        ("Add", description.clone())
                    }
                    Self::OuiEdit { description, .. } => {
                        let index = s
                            .oui_controls
                            .get(&key)
                            .ok_or(Error::Usage("OUI does not exist"))?;
                        admin::label(description, 32)?;
                        next.ouis.insert(key.clone(), description.clone());
                        fields.push(field(index, "checked"));
                        ("Apply", description.clone())
                    }
                    _ => (
                        "Delete",
                        next.ouis
                            .remove(&key)
                            .ok_or(Error::Usage("OUI does not exist"))?,
                    ),
                };
                fields.extend([
                    field("ACTION", action),
                    field("TELEPHONY", &key),
                    field("DESCRIPTION", desc),
                ]);
                ("vlan-oui", "/OUITable.cgi", format!("{action} {key}"))
            }
        };
        Ok(Plan {
            operation: op,
            details,
            submission: (next != *s).then_some(Submission { path, fields }),
            expected: next,
        })
    }
}
impl Table {
    pub fn contains_id(&self, id: u16) -> bool {
        self.vlans.contains_key(&id)
    }
    pub fn check_lag(&self, ports: &std::collections::BTreeSet<u8>) -> Result<()> {
        if let Some(first) = ports.first() {
            for p in ports {
                if self.pvids.get(p) != self.pvids.get(first)
                    || self.assignments.get(p) != self.assignments.get(first)
                    || self
                        .vlans
                        .values()
                        .any(|r| r.members.get(p) != r.members.get(first))
                {
                    return Err(Error::Usage(
                        "LAG members must have matching VLAN membership, tagging and PVIDs",
                    ));
                }
            }
        }
        Ok(())
    }
    fn row_form(&self, id: u16, count: usize, create: bool) -> Result<(&'static str, Fields)> {
        let r = self
            .vlans
            .get(&id)
            .ok_or(Error::Usage("VLAN does not exist"))?;
        let mut f = vec![field("VLAN_ID", id), field("VLAN_NAME", &r.name)];
        let path = match self.mode {
            Mode::BasicDot1q => {
                f.extend([
                    field("page", "vlanCfg"),
                    field("Action", "Apply"),
                    field("vlanNum", count),
                ]);
                "/8021qBasic.cgi"
            }
            Mode::AdvancedPort | Mode::AdvancedDot1q => {
                let mask: String = (1..=8)
                    .map(|p| match r.members.get(&p) {
                        Some(Member::Tagged | Member::Included) => '1',
                        Some(Member::Untagged) => '2',
                        None if self.mode.dot1q() => '3',
                        None => '0',
                    })
                    .collect();
                f.push(field("hiddenMem", mask));
                if self.mode.dot1q() {
                    f.extend([
                        field("ACTION", if create { "Add" } else { "Apply" }),
                        field("vlanNum", count),
                        field("voiceVLANID", u8::from(self.voice_vlan == Some(id))),
                        field(
                            "voiceVlanCos",
                            self.voice_cos.ok_or(Error::Page("missing voice class"))?,
                        ),
                    ]);
                    "/8021qAdvanced.cgi"
                } else {
                    "/portBasedAdvanced.cgi"
                }
            }
            _ => return Err(Error::Usage("no VLAN editor in this mode")),
        };
        Ok((path, f))
    }
}
