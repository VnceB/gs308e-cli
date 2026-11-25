//! In-memory test model. The only source of connectivity and port roles is the test.
//! Configuration values are inert data. There is no client, storage or firmware encoder.
use std::collections::{BTreeMap, BTreeSet};

pub type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Unknown,
    IsolatedFixture,
    Uplink,
    ManagementComputer,
    OtherDevice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Up,
    Down,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Connection {
    pub role: Role,
    pub link: Link,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Auto,
    Disabled,
    Full10,
    Full100,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortSettings {
    pub label: String,
    pub speed: Speed,
    pub flow: bool,
    pub ingress_kbps: Option<u32>,
    pub egress_kbps: Option<u32>,
    pub priority: Priority,
}

impl Default for PortSettings {
    fn default() -> Self {
        Self {
            label: "Fixture".into(),
            speed: Speed::Auto,
            flow: true,
            ingress_kbps: None,
            egress_kbps: None,
            priority: Priority::High,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Membership {
    Tagged,
    Untagged,
    Excluded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vlan {
    pub name: String,
    pub members: [Membership; 8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mirror {
    pub sources: Vec<u8>,
    pub destination: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Toggle {
    Leds,
    EnergySaving,
    LoopPrevention,
    BroadcastFiltering,
    IgmpSnooping,
    BlockUnknownMulticast,
    ValidateIgmpHeader,
    Discovery,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Disabled,
    BasicPort,
    AdvancedPort,
    BasicQ,
    AdvancedQ,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voice {
    pub vlan: u16,
    pub cos: u8,
    pub ouis: BTreeSet<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticIpv4 {
    pub address: std::net::Ipv4Addr,
    pub prefix: u8,
    pub gateway: std::net::Ipv4Addr,
}

impl StaticIpv4 {
    fn validate(self) -> Result<()> {
        // This fixture exercises a conventional subnet with two distinct host addresses.
        if !(1..=30).contains(&self.prefix) {
            return Err("unsupported fixture prefix");
        }
        let mask = u32::MAX << (32 - self.prefix);
        let address = u32::from(self.address);
        let gateway = u32::from(self.gateway);
        let network = address & mask;
        let broadcast = network | !mask;
        if address == gateway
            || (gateway & mask) != network
            || [address, gateway]
                .iter()
                .any(|&v| v == network || v == broadcast)
        {
            return Err("invalid fixture address or gateway");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SntpClient {
    pub local_port: u16,
    pub interval_exponent: u8,
    pub timeout_seconds: u8,
    pub retries: u8,
}

impl SntpClient {
    fn validate(self) -> Result<()> {
        if !matches!(self.local_port, 123 | 1025..=65535)
            || !(6..=10).contains(&self.interval_exponent)
            || !(1..=30).contains(&self.timeout_seconds)
            || self.retries > 10
        {
            return Err("invalid SNTP client settings");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shared {
    pub name: String,
    pub dscp: bool,
    pub toggles: BTreeSet<Toggle>,
    pub mode: Mode,
    pub voice: Option<Voice>,
    pub igmp_vlan: Option<u16>,
    pub router_port: Option<u8>,
    pub static_ipv4: Option<StaticIpv4>,
    pub sntp: Option<SntpClient>,
    pub vlans: BTreeMap<u16, Vlan>,
    pub pvid: [u16; 8],
    pub lags: [Vec<u8>; 2],
    pub mirror: Option<Mirror>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub ports: [PortSettings; 8],
    pub shared: Shared,
}

impl Default for Config {
    fn default() -> Self {
        let default_vlan = Vlan {
            name: "Default".into(),
            members: [Membership::Untagged; 8],
        };
        Self {
            ports: std::array::from_fn(|_| PortSettings::default()),
            shared: Shared {
                name: "Example switch".into(),
                dscp: false,
                toggles: BTreeSet::new(),
                mode: Mode::Disabled,
                voice: None,
                igmp_vlan: None,
                router_port: None,
                static_ipv4: None,
                sntp: None,
                vlans: BTreeMap::from([(1, default_vlan)]),
                pvid: [1; 8],
                lags: Default::default(),
                mirror: None,
            },
        }
    }
}

pub fn index(port: u8) -> Result<usize> {
    match port {
        1..=8 => Ok(usize::from(port - 1)),
        _ => Err("invalid physical port"),
    }
}

fn distinct_ports(numbers: &[u8]) -> Result<BTreeSet<u8>> {
    let mut seen = BTreeSet::new();
    for &p in numbers {
        index(p)?;
        if !seen.insert(p) {
            return Err("duplicate port");
        }
    }
    Ok(seen)
}

fn printable_name(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.len()) && s.bytes().all(|b| (32..=126).contains(&b))
}

impl Config {
    fn validate(&self, before: &Self) -> Result<()> {
        for (old, new) in before.ports.iter().zip(&self.ports) {
            if !printable_name(&new.label, 16) {
                return Err("invalid port label");
            }
            // Two illustrative choices from the synthetic editor. Firmware rate codes are unknown.
            for rate in [new.ingress_kbps, new.egress_kbps].into_iter().flatten() {
                if !matches!(rate, 512 | 1024) {
                    return Err("rate absent from fixture menu");
                }
            }
            if self.shared.dscp && old.priority != new.priority {
                return Err("priority requires port-based QoS");
            }
        }
        if !printable_name(&self.shared.name, 20) {
            return Err("invalid device name");
        }
        if let Some(address) = self.shared.static_ipv4 {
            address.validate()?;
        }
        if let Some(sntp) = self.shared.sntp {
            sntp.validate()?;
        }
        self.validate_vlans(before)?;
        self.validate_lags()?;
        if let Some(mirror) = &self.shared.mirror {
            index(mirror.destination)?;
            let sources = distinct_ports(&mirror.sources)?;
            if sources.is_empty() || sources.contains(&mirror.destination) {
                return Err("invalid mirror endpoints");
            }
        }
        Ok(())
    }

    fn validate_vlans(&self, before: &Self) -> Result<()> {
        let shared = &self.shared;
        let capacity = if shared.mode == Mode::AdvancedQ {
            64
        } else {
            8
        };
        if !shared.vlans.contains_key(&1) || shared.vlans.len() > capacity {
            return Err("invalid VLAN table");
        }
        let membership_changed =
            shared.vlans != before.shared.vlans || shared.pvid != before.shared.pvid;
        if membership_changed && shared.mode == Mode::Disabled {
            return Err("VLAN mode required");
        }
        if before.shared.mode != shared.mode
            && (before.shared.vlans != Self::default().shared.vlans
                || before.shared.pvid != [1; 8]
                || before.shared.voice.is_some())
        {
            return Err("mode change would discard configuration");
        }
        let max_id = if matches!(shared.mode, Mode::BasicPort | Mode::AdvancedPort) {
            8
        } else {
            4093
        };
        for (id, vlan) in &shared.vlans {
            if !(1..=max_id).contains(id) || !printable_name(&vlan.name, 14) {
                return Err("invalid VLAN ID or name");
            }
        }
        for (port, id) in shared.pvid.iter().enumerate() {
            if shared.mode == Mode::AdvancedQ {
                let vlan = shared.vlans.get(id).ok_or("PVID references missing VLAN")?;
                if vlan.members[port] == Membership::Excluded {
                    return Err("PVID requires membership");
                }
            } else {
                if *id != 1 {
                    return Err("PVID editing requires advanced 802.1Q");
                }
                let mut tagged = 0;
                let mut untagged = 0;
                for vlan in shared.vlans.values() {
                    match vlan.members[port] {
                        Membership::Tagged => tagged += 1,
                        Membership::Untagged => untagged += 1,
                        Membership::Excluded => {}
                    }
                }
                let valid = match shared.mode {
                    Mode::BasicPort => tagged == 0 && untagged == 1,
                    Mode::AdvancedPort => tagged == 0,
                    Mode::BasicQ => {
                        (untagged == 1 && tagged == 0)
                            || (untagged == 0 && tagged == shared.vlans.len())
                    }
                    _ => true,
                };
                if !valid {
                    return Err("unsupported membership for VLAN mode");
                }
            }
        }
        if let Some(v) = &shared.voice
            && (shared.mode != Mode::AdvancedQ
                || !shared.vlans.contains_key(&v.vlan)
                || v.cos > 7
                || v.ouis.len() > 15)
        {
            return Err("invalid voice VLAN");
        }
        if let Some(id) = shared.igmp_vlan
            && (shared.mode == Mode::Disabled || !shared.vlans.contains_key(&id))
        {
            return Err("IGMP VLAN must exist in an active mode");
        }
        if let Some(router) = shared.router_port {
            index(router)?;
        }
        Ok(())
    }

    fn validate_lags(&self) -> Result<()> {
        let mut assigned = BTreeSet::new();
        for group in &self.shared.lags {
            if group.is_empty() {
                continue;
            }
            if !(2..=4).contains(&group.len()) {
                return Err("invalid static LAG size");
            }
            let ports = distinct_ports(group)?;
            let first = index(group[0])?;
            for p in ports {
                let i = index(p)?;
                if !assigned.insert(p) {
                    return Err("overlapping LAGs");
                }
                let member = &self.ports[i];
                let peer = &self.ports[first];
                if member.speed != peer.speed || member.flow != peer.flow {
                    return Err("LAG port settings differ");
                }
                if self
                    .shared
                    .vlans
                    .values()
                    .any(|v| v.members[i] != v.members[first])
                {
                    return Err("LAG VLAN membership differs");
                }
            }
        }
        Ok(())
    }

    fn shared_port(&self, i: usize) -> bool {
        let number = (i + 1) as u8;
        self.shared.router_port == Some(number)
            || self.shared.lags.iter().any(|g| g.contains(&number))
            || self
                .shared
                .mirror
                .as_ref()
                .is_some_and(|m| m.destination == number || m.sources.contains(&number))
            || self.shared.vlans.iter().any(|(&id, v)| {
                v.members[i] != Membership::Excluded
                    && (id != 1
                        || Some(id) == self.shared.igmp_vlan
                        || self
                            .shared
                            .voice
                            .as_ref()
                            .is_some_and(|voice| voice.vlan == id))
            })
    }
}

#[derive(Debug)]
pub struct Plan {
    before: Config,
    after: Config,
    affected: Vec<usize>,
}

impl Plan {
    pub fn before(&self) -> &Config {
        &self.before
    }
    pub fn after(&self) -> &Config {
        &self.after
    }
    fn new(before: &Config, after: Config) -> Result<Self> {
        after.validate(before)?;
        let changed: Vec<_> = (0..8)
            .filter(|&i| before.ports[i] != after.ports[i])
            .collect();
        let shared =
            before.shared != after.shared || changed.iter().any(|&i| before.shared_port(i));
        Ok(Self {
            before: before.clone(),
            after,
            affected: if shared { (0..8).collect() } else { changed },
        })
    }
}

#[derive(Clone, Copy)]
pub enum Delivery {
    Ack,
    Rejected,
    LostBeforeApply,
    LostAfterApply,
    AckWithoutApply,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Readback {
    Applied,
    Original,
    Diverged,
    Unavailable,
}

pub struct Lab {
    pub current: Config,
    pub connections: [Connection; 8],
    pub submissions: usize,
}

impl Lab {
    pub fn isolated() -> Self {
        Self {
            current: Config::default(),
            connections: [Connection {
                role: Role::IsolatedFixture,
                link: Link::Down,
            }; 8],
            submissions: 0,
        }
    }

    fn allowed(&self, plan: &Plan) -> Result<()> {
        for &i in &plan.affected {
            if self.connections[i]
                != (Connection {
                    role: Role::IsolatedFixture,
                    link: Link::Down,
                })
            {
                return Err("protected or unverified scope");
            }
        }
        Ok(())
    }

    pub fn preview(&self, edit: impl FnOnce(&mut Config)) -> Result<Plan> {
        let mut next = self.current.clone();
        edit(&mut next);
        let plan = Plan::new(&self.current, next)?;
        self.allowed(&plan)?;
        Ok(plan)
    }

    pub fn submit(&mut self, plan: &Plan, delivery: Delivery) -> Result<()> {
        self.allowed(plan)?;
        if self.current != plan.before {
            return Err("stale preimage");
        }
        if plan.affected.is_empty() {
            return Ok(());
        }
        self.submissions += 1;
        match delivery {
            Delivery::Rejected => Err("fixture rejection"),
            Delivery::LostBeforeApply => Err("acknowledgement unavailable"),
            Delivery::LostAfterApply => {
                self.current = plan.after.clone();
                Err("acknowledgement unavailable")
            }
            Delivery::AckWithoutApply => Ok(()),
            Delivery::Ack => {
                self.current = plan.after.clone();
                Ok(())
            }
        }
    }

    pub fn verify(&self, plan: &Plan, readable: bool) -> Readback {
        if !readable {
            Readback::Unavailable
        } else if self.current == plan.after {
            Readback::Applied
        } else if self.current == plan.before {
            Readback::Original
        } else {
            Readback::Diverged
        }
    }

    pub fn restore(&mut self, plan: &Plan, readable: bool) -> Result<()> {
        self.restore_with(plan, readable, Delivery::Ack)
    }

    pub fn restore_with(&mut self, plan: &Plan, readable: bool, delivery: Delivery) -> Result<()> {
        match self.verify(plan, readable) {
            Readback::Original => Ok(()),
            Readback::Applied => {
                let inverse = Plan::new(&plan.after, plan.before.clone())?;
                let response = self.submit(&inverse, delivery);
                if self.verify(&inverse, readable) == Readback::Applied {
                    Ok(())
                } else {
                    response.and(Err("restoration mismatch"))
                }
            }
            Readback::Unavailable | Readback::Diverged => {
                Err("restoration requires confirmed state")
            }
        }
    }
}
