mod admin;
mod console;
mod diagnostics;
mod error;
mod firmware;
mod maintenance;
mod model;
mod profile;
mod session;
mod settings;
mod signin;
mod time;
mod vlan;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use error::{Error, Result};
use model::{Counter, Device, Port, Vlan};
use serde::Serialize;
use session::{Page, Session, Switch};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::process::ExitCode;
use zeroize::Zeroizing;

/// Inspect and configure a NETGEAR GS308Ev4 switch.
#[derive(Parser)]
#[command(
    version,
    after_help = "Examples:\n  gs308e --host 192.0.2.1 info\n  gs308e --host switch.example ports\n  gs308e --json counters --port 3 --password-stdin < /run/secrets/switch\n  gs308e vlan list\n  gs308e settings show port --port 2\n  gs308e vlan create 20 --name Studio --untagged 2,4\n  gs308e completions zsh\n\nSet GS308E_HOST to reuse a management address."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Switch management address or origin (defaults to http:// if no scheme is given)
    #[arg(long, env = "GS308E_HOST", global = true, value_name = "HOST")]
    host: Option<String>,
    /// Read the first password line from a pipe or redirected stdin
    #[arg(long, global = true)]
    password_stdin: bool,
    /// Print fixed sign-in response and cookie classifications to stderr for this login attempt
    #[arg(long, global = true)]
    explain_login: bool,
    /// Emit machine-readable JSON with schema_version 1
    #[arg(long, global = true)]
    json: bool,
    /// Request submission only on firmware with an approved write profile
    #[arg(long, global = true)]
    apply: bool,
    /// Ports to protect from configuration changes, comma-separated
    #[arg(long,global=true,env="GS308E_PROTECT_PORTS",value_delimiter=',',value_parser=clap::value_parser!(u8).range(1..=8))]
    protect_port: Vec<u8>,
    /// Trust an additional PEM CA certificate for HTTPS
    #[arg(long, global = true, value_name = "FILE")]
    cacert: Option<std::path::PathBuf>,
    /// Maximum seconds per HTTP request
    #[arg(long,global=true,default_value_t=8,value_name="SECONDS",value_parser=clap::value_parser!(u64).range(1..=120))]
    timeout: u64,
}

#[derive(Subcommand)]
enum Command {
    /// Show model, firmware and device identifiers
    Info,
    /// Show the detected firmware version without device identifiers (JSON adds the write policy)
    Firmware,
    /// List ports with link state, negotiated speed, duplex and name
    Ports {
        #[command(flatten)]
        filter: PortFilter,
        /// Count connected and idle physical ports
        #[arg(long, conflicts_with = "port")]
        summary: bool,
    },
    /// Show per-port traffic and CRC error counters
    Counters {
        #[command(flatten)]
        filter: PortFilter,
        #[command(subcommand)]
        action: Option<CounterAction>,
    },
    /// Inspect or configure the clock, SNTP servers and daylight-saving rules
    Time {
        #[command(subcommand)]
        action: time::Action,
    },
    /// Inspect diagnostics, export configuration or preview disruptive actions
    Maintenance {
        #[command(subcommand)]
        action: maintenance::Action,
    },
    /// Inspect or edit a documented settings page
    Settings {
        #[command(subcommand)]
        action: settings::Action,
    },
    /// Collect device, ports, counters and VLAN mode in one session
    Snapshot,
    /// Read ports, VLAN mode and table, and stored cable results in one session and verify logout
    Diagnostics,
    /// Open a command prompt, or run a batch with one login
    #[command(
        visible_alias = "interactive",
        after_help = "Examples:\n  gs308e session\n  gs308e session --read-only -f reports.txt\n  gs308e session -c 'ports --summary' -c 'vlan list'"
    )]
    Session {
        /// Allow only passive reports and reject all --apply flags
        #[arg(long)]
        read_only: bool,
        /// Command with shell-style quoting. Repeat to run a sequence
        #[arg(
            short = 'c',
            long = "command",
            value_name = "COMMAND",
            allow_hyphen_values = true
        )]
        commands: Vec<String>,
        /// Read a UTF-8 file with one command per line. Blank lines and # comments are ignored
        #[arg(short = 'f', long, value_name = "FILE", conflicts_with = "commands")]
        file: Option<std::path::PathBuf>,
    },
    /// Inspect VLAN configuration or preview and apply a VLAN change
    Vlan {
        #[command(subcommand)]
        action: Option<vlan::Action>,
    },
    /// Write a completion script for your shell
    Completions { shell: Shell },
}

#[derive(Subcommand)]
enum CounterAction {
    /// Clear the statistics on all eight ports, irreversibly
    Clear {
        #[arg(long, required = true)]
        all: bool,
    },
}
#[derive(clap::Args)]
struct PortFilter {
    /// Return only this physical port
    #[arg(short,long,value_name="1-8",value_parser=clap::value_parser!(u8).range(1..=8))]
    port: Option<u8>,
}

#[derive(Serialize)]
#[serde(tag = "command", content = "data", rename_all = "lowercase")]
enum Report {
    Info(Device),
    Firmware(profile::FirmwareReport),
    Diagnostics(Box<diagnostics::Diagnostics>),
    #[serde(rename = "session-end")]
    SessionEnd(diagnostics::LogoutReport),
    Ports(Vec<Port>),
    #[serde(rename = "ports-summary")]
    PortSummary {
        total: usize,
        connected: usize,
        idle: usize,
    },
    Counters(Vec<Counter>),
    Vlan(Vlan),
    #[serde(rename = "vlan-list")]
    VlanList(vlan::Table),
    Change(admin::Change),
    Time(time::Fields),
    #[serde(rename = "sntp-servers")]
    TimeServers(time::Servers),
    #[serde(rename = "cable-results")]
    CableResults(Vec<maintenance::CableResult>),
    #[serde(skip)]
    Backup(Zeroizing<Vec<u8>>),
    Settings(settings::Values),
    Snapshot {
        device: Device,
        ports: Vec<Port>,
        counters: Vec<Counter>,
        vlan: Vlan,
    },
}

fn collect(
    session: &Session<'_>,
    command: &Command,
    apply: bool,
    protected: &[u8],
) -> Result<Report> {
    let needs_dashboard = matches!(
        command,
        Command::Info
            | Command::Firmware
            | Command::Ports { .. }
            | Command::Snapshot
            | Command::Diagnostics
            | Command::Maintenance {
                action: maintenance::Action::Upgrade { .. }
            }
            | Command::Settings {
                action: settings::Action::Show {
                    feature: settings::Feature::Identity | settings::Feature::Port,
                    ..
                }
            }
            | Command::Settings {
                action: settings::Action::Set {
                    feature: settings::Feature::Identity,
                    ..
                }
            }
    );
    let dashboard = if needs_dashboard {
        Some(session.dashboard()?)
    } else {
        session.ensure_bound()?;
        None
    };
    if apply {
        session.require_write(matches!(
            command,
            Command::Settings {
                action: settings::Action::Set {
                    feature: settings::Feature::Identity,
                    ..
                }
            }
        ))?;
    }
    match command {
        Command::Time { action } => action.run(session, apply),
        Command::Maintenance { action } => {
            action.run(session, apply, protected, dashboard.as_ref())
        }
        Command::Firmware => Ok(Report::Firmware(session.binding().report())),
        Command::Info => Ok(Report::Info(model::device(dashboard.as_ref().unwrap())?)),
        Command::Ports { filter, summary } => {
            let mut rows = model::ports(dashboard.as_ref().unwrap())?;
            if *summary {
                let total = rows.len();
                let connected = rows.iter().filter(|p| p.link_mbps.is_some()).count();
                return Ok(Report::PortSummary {
                    total,
                    connected,
                    idle: total - connected,
                });
            }
            rows.retain(|p| filter.port.is_none_or(|n| p.port == n));
            Ok(Report::Ports(rows))
        }
        Command::Settings {
            action: settings::Action::Show { feature, port },
        } => {
            let fetched;
            let page = match feature.page() {
                Page::Dashboard => dashboard.as_ref().unwrap(),
                other => {
                    fetched = session.read(other)?;
                    &fetched
                }
            };
            Ok(Report::Settings(feature.observe(
                page,
                *port,
                session.binding(),
            )?))
        }
        Command::Settings {
            action:
                settings::Action::Set {
                    feature,
                    port,
                    values,
                },
        } => {
            if apply && !protected.is_empty() {
                match feature {
                    settings::Feature::Identity | settings::Feature::Leds => {}
                    settings::Feature::Port | settings::Feature::StormRate => {
                        if port.is_some_and(|p| protected.contains(&p)) {
                            return Err(Error::Usage("cannot modify a protected port"));
                        }
                    }
                    _ => {
                        return Err(Error::Usage(
                            "this settings change may affect protected ports",
                        ));
                    }
                }
            }
            Ok(Report::Change(settings::change(
                session,
                *feature,
                *port,
                values,
                apply,
                *feature == settings::Feature::Discovery && session.binding().reviewed_ui(),
                protected,
            )?))
        }
        Command::Counters {
            filter,
            action: Some(CounterAction::Clear { .. }),
        } => {
            if filter.port.is_some() || (apply && !protected.is_empty()) {
                return Err(Error::Usage("clear applies to all ports"));
            }
            Ok(Report::Change(admin::clear(session, apply)?))
        }
        Command::Counters {
            filter,
            action: None,
        } => {
            let mut rows = model::counters(&session.read(Page::Statistics)?)?;
            rows.retain(|p| filter.port.is_none_or(|n| p.port == n));
            Ok(Report::Counters(rows))
        }
        Command::Vlan {
            action: Some(vlan::Action::List),
        } => Ok(Report::VlanList(vlan::Table::parse(
            &session.read(Page::Vlan)?,
        )?)),
        Command::Vlan {
            action: Some(action),
        } => {
            if apply && !protected.is_empty() {
                action.check_protected(protected)?;
            }
            Ok(Report::Change(action.run(session, apply, protected)?))
        }
        Command::Vlan { action: None } => {
            Ok(Report::Vlan(model::vlan(&session.read(Page::Vlan)?)?))
        }
        Command::Snapshot => {
            let dash = dashboard.as_ref().unwrap();
            Ok(Report::Snapshot {
                device: model::device(dash)?,
                ports: model::ports(dash)?,
                counters: model::counters(&session.read(Page::Statistics)?)?,
                vlan: model::vlan(&session.read(Page::Vlan)?)?,
            })
        }
        Command::Diagnostics => {
            if apply {
                return Err(Error::Usage(
                    "diagnostics is read-only and does not accept --apply",
                ));
            }
            Ok(Report::Diagnostics(Box::new(
                diagnostics::Diagnostics::read(session, dashboard.as_ref().unwrap()),
            )))
        }
        Command::Completions { .. } | Command::Session { .. } => {
            unreachable!("handled before collect")
        }
    }
}

fn password(from_stdin: bool) -> Result<Zeroizing<String>> {
    let mut password = if from_stdin {
        if io::stdin().is_terminal() {
            return Err(Error::Usage(
                "pipe the password to --password-stdin or omit it for a hidden prompt",
            ));
        }
        let mut input = Zeroizing::new(String::new());
        io::stdin()
            .lock()
            .take(514)
            .read_line(&mut input)
            .map_err(|_| Error::Usage("could not read password from stdin"))?;
        if input.ends_with('\n') {
            input.pop();
            if input.ends_with('\r') {
                input.pop();
            }
        }
        input
    } else {
        if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            return Err(Error::Usage(
                "noninteractive use requires --password-stdin and a password on stdin",
            ));
        }
        Zeroizing::new(
            rpassword::prompt_password("Switch password: ")
                .map_err(|_| Error::Usage("could not open terminal for password input"))?,
        )
    };
    if password.is_empty()
        || password.len() > 512
        || !password.bytes().all(|b| (32..=126).contains(&b))
    {
        password.clear();
        return Err(Error::Usage(
            "expected one nonempty printable ASCII password (up to 512 bytes)",
        ));
    }
    Ok(password)
}

fn printable(raw: &str) -> String {
    raw.chars().map(|c| if c.is_control() || matches!(c,'\u{061c}'|'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}') {'�'} else {c}).collect()
}

fn render(report: &Report, json: bool, out: &mut impl Write) -> io::Result<()> {
    if json {
        #[derive(Serialize)]
        struct Envelope<'a> {
            schema_version: u8,
            #[serde(flatten)]
            report: &'a Report,
        }
        let line = serde_json::to_string(&Envelope {
            schema_version: 1,
            report,
        })
        .map_err(io::Error::other)?;
        return writeln!(out, "{line}");
    }
    match report {
        Report::Firmware(f) => writeln!(
            out,
            "Firmware    {}",
            f.version.as_deref().unwrap_or("unidentified")
        ),
        Report::Info(d) => render_info(d, out),
        Report::Ports(rows) => render_ports(rows, out),
        Report::PortSummary {
            connected,
            idle,
            total,
        } => writeln!(out, "{connected} connected, {idle} idle ({total} ports)"),
        Report::Counters(rows) => render_counters(rows, out),
        Report::Snapshot {
            device,
            ports,
            counters,
            vlan,
        } => {
            render_info(device, out)?;
            writeln!(out)?;
            render_ports(ports, out)?;
            writeln!(out)?;
            render_counters(counters, out)?;
            writeln!(out)?;
            render_vlan(vlan, out)
        }
        Report::Time(v) => write!(out, "{v}"),
        Report::TimeServers(v) => write!(out, "{v}"),
        Report::CableResults(rows) => render_cable_results(rows, out),
        Report::Diagnostics(checks) => checks.render(out),
        Report::SessionEnd(logout) => logout.render(out),
        Report::Backup(bytes) => out.write_all(bytes),
        Report::Vlan(v) => render_vlan(v, out),
        Report::VlanList(table) => write!(out, "{table}"),
        Report::Settings(values) => write!(out, "{values}"),
        Report::Change(change) => writeln!(out, "{change}"),
    }
}

fn render_cable_results(rows: &[maintenance::CableResult], out: &mut impl Write) -> io::Result<()> {
    let show_dist = rows.iter().any(|r| r.distance.is_some());
    if show_dist {
        writeln!(out, "PORT  LAST RESULT  DISTANCE")?;
    } else {
        writeln!(out, "PORT  LAST RESULT")?;
    }
    for row in rows {
        if show_dist {
            writeln!(
                out,
                "{:>4}  {:<11}  {}",
                row.port,
                printable(&row.result),
                printable(row.distance.as_deref().unwrap_or("-"))
            )?;
        } else {
            writeln!(out, "{:>4}  {}", row.port, printable(&row.result))?;
        }
    }
    Ok(())
}

fn render_info(d: &Device, out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "Model       {}", d.model)?;
    writeln!(out, "Firmware    {}", printable(&d.firmware))?;
    writeln!(out, "Name        {}", printable(&d.name))?;
    writeln!(out, "MAC         {}", printable(&d.mac_address))?;
    writeln!(out, "Serial      {}", printable(&d.serial_number))
}
fn render_ports(rows: &[Port], out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "PORT  STATUS      LINK       DUPLEX  NAME")?;
    for p in rows {
        let speed = p
            .link_mbps
            .map(|n| format!("{n} Mbps"))
            .unwrap_or_else(|| "-".into());
        writeln!(
            out,
            "{:>4}  {:<10}  {:<9}  {:<6}  {}",
            p.port,
            printable(&p.status),
            speed,
            p.duplex.as_deref().unwrap_or("-"),
            printable(&p.name)
        )?;
    }
    Ok(())
}
fn render_counters(rows: &[Counter], out: &mut impl Write) -> io::Result<()> {
    let widths = rows.iter().fold([8, 8, 10], |w, c| {
        [
            w[0].max(c.rx_bytes.to_string().len()),
            w[1].max(c.tx_bytes.to_string().len()),
            w[2].max(c.crc_errors.to_string().len()),
        ]
    });
    let [rx, tx, crc] = widths;
    writeln!(
        out,
        "PORT  {:>rx$}  {:>tx$}  {:>crc$}",
        "RX BYTES", "TX BYTES", "CRC ERRORS"
    )?;
    for c in rows {
        writeln!(
            out,
            "{:>4}  {:>rx$}  {:>tx$}  {:>crc$}",
            c.port, c.rx_bytes, c.tx_bytes, c.crc_errors
        )?;
    }
    Ok(())
}
fn render_vlan(v: &Vlan, out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "VLAN mode   {}", v.mode.replace('_', " "))
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    if let Command::Completions { shell } = cli.command {
        let mut buf = Vec::new();
        clap_complete::generate(shell, &mut Cli::command(), "gs308e", &mut buf);
        return output(io::stdout().lock().write_all(&buf));
    }
    if matches!(
        cli.command,
        Command::Maintenance {
            action: maintenance::Action::Backup { .. }
        }
    ) && (cli.json || io::stdout().is_terminal())
    {
        return Err(Error::Usage(
            "backup requires redirected binary stdout and cannot be combined with JSON",
        ));
    }
    if matches!(cli.command, Command::Diagnostics) && cli.apply {
        return Err(Error::Usage(
            "diagnostics is read-only and does not accept --apply",
        ));
    }
    let commands = match &cli.command {
        Command::Session { .. } if cli.apply => {
            return Err(Error::Usage("put --apply on an individual session command"));
        }
        Command::Session {
            commands,
            read_only,
            file,
        } => Some((
            console::prepare(commands, file.as_deref(), *read_only)?,
            *read_only,
        )),
        _ => None,
    };
    let host = cli.host.as_deref().ok_or(Error::Usage(
        "provide --host <HOST> or set GS308E_HOST; see --help for examples",
    ))?;
    let switch = Switch::new(host, cli.timeout, cli.explain_login, cli.cacert.as_deref())?;
    let secret = password(cli.password_stdin)?;
    if let Some((commands, read_only)) = commands {
        return console::run(
            &switch,
            secret,
            commands,
            read_only,
            cli.json,
            &cli.protect_port,
        );
    }
    let session = switch.login(&secret)?;
    drop(secret);
    let report = if matches!(cli.command, Command::Diagnostics) {
        Report::Diagnostics(Box::new(diagnostics::Diagnostics::run(session)?))
    } else {
        let report = collect(&session, &cli.command, cli.apply, &cli.protect_port);
        if let Err(err) = session.logout() {
            eprintln!("warning: could not confirm logout ({err})");
        }
        report?
    };
    output(render(&report, cli.json, &mut io::stdout().lock()))?;
    if matches!(report, Report::Diagnostics(ref checks) if !checks.complete()) {
        return Err(Error::PartialDiagnostics);
    }
    Ok(())
}

fn output(result: io::Result<()>) -> Result<()> {
    match result {
        Err(err) if err.kind() != io::ErrorKind::BrokenPipe => Err(Error::Output),
        _ => Ok(()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(e.code())
        }
    }
}
