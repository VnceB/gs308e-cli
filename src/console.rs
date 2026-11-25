use crate::diagnostics::LogoutReport;
use crate::error::{Error, Position, Result};
use crate::session::{Session, Switch};
use crate::{Command, Report, collect, render};
use clap::{Parser, error::ErrorKind};
use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(
    name = "gs308e",
    about = "Commands for the open session. Use exit to log out."
)]
pub(crate) struct Invocation {
    #[command(subcommand)]
    command: Command,
    /// Apply only this command. Changes otherwise produce previews
    #[arg(long, global = true)]
    apply: bool,
}

pub(crate) enum Entry {
    Command(Invocation),
    Help(String),
    Exit,
    Empty,
}

fn parse(line: &str, read_only: bool) -> Result<Entry> {
    let words = shlex::split(line).ok_or(Error::Usage(
        "unterminated quote or escape in session command",
    ))?;
    if words.is_empty() {
        return Ok(Entry::Empty);
    }
    if words.len() == 1 && matches!(words[0].as_str(), "exit" | "quit" | "logout") {
        return Ok(Entry::Exit);
    }
    let invocation =
        match Invocation::try_parse_from(std::iter::once("gs308e".to_owned()).chain(words)) {
            Ok(invocation) => invocation,
            Err(error) if error.kind() == ErrorKind::DisplayHelp => {
                return Ok(Entry::Help(error.to_string()));
            }
            Err(error) => return Err(Error::CommandLine(error.to_string().trim_end().into())),
        };
    if read_only
        && (invocation.apply
            || !matches!(
                &invocation.command,
                Command::Info
                    | Command::Firmware
                    | Command::Ports { .. }
                    | Command::Counters { action: None, .. }
                    | Command::Snapshot
                    | Command::Diagnostics
                    | Command::Vlan {
                        action: None | Some(crate::vlan::Action::List)
                    }
                    | Command::Settings {
                        action: crate::settings::Action::Show { .. }
                    }
                    | Command::Time {
                        action: crate::time::Action::Clock
                            | crate::time::Action::Status
                            | crate::time::Action::DstStatus
                            | crate::time::Action::Dst
                            | crate::time::Action::Servers
                    }
                    | Command::Maintenance {
                        action: crate::maintenance::Action::CableResults
                    }
            ))
    {
        return Err(Error::Usage(
            "read-only sessions accept passive reports only",
        ));
    }
    match invocation.command {
        Command::Session { .. } | Command::Completions { .. } => Err(Error::Usage(
            "run session and completions outside an open session",
        )),
        Command::Maintenance {
            action: crate::maintenance::Action::Backup { .. },
        } => Err(Error::Usage(
            "run the binary configuration export as its own invocation",
        )),
        Command::Diagnostics if invocation.apply => Err(Error::Usage(
            "diagnostics is read-only and does not accept --apply",
        )),
        _ => Ok(Entry::Command(invocation)),
    }
}

pub(crate) fn prepare(
    commands: &[String],
    file: Option<&Path>,
    read_only: bool,
) -> Result<Vec<(Position, Entry)>> {
    if let Some(path) = file {
        return prepare_file(path, read_only);
    }
    commands
        .iter()
        .enumerate()
        .map(
            |(i, command)| match parse(command, read_only).map_err(|e| e.at_command(i + 1))? {
                Entry::Empty => Err(Error::Usage("empty -c command").at_command(i + 1)),
                Entry::Exit if i + 1 < commands.len() => {
                    Err(Error::Usage("logout must be the last -c command").at_command(i + 1))
                }
                entry => Ok((Position::Command(i + 1), entry)),
            },
        )
        .collect()
}

fn prepare_file(path: &Path, read_only: bool) -> Result<Vec<(Position, Entry)>> {
    let metadata = path
        .metadata()
        .map_err(|e| Error::CommandLine(format!("could not open command file: {e}")))?;
    if !metadata.is_file() {
        return Err(Error::Usage("expected a regular command file"));
    }
    let file = File::open(path)
        .map_err(|e| Error::CommandLine(format!("could not open command file: {e}")))?;
    let mut reader = BufReader::new(file);
    let mut entries = Vec::new();
    for number in 1.. {
        let position = Position::FileLine(number);
        let Some(line) = next_line(&mut reader).map_err(|e| e.at(position))? else {
            break;
        };
        let line = line.trim_end_matches(['\r', '\n']);
        let line = if number == 1 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        let entry = parse(line, read_only).map_err(|e| e.at(position))?;
        if matches!(entry, Entry::Empty) {
            continue;
        }
        if let Some((previous, Entry::Exit)) = entries.last() {
            return Err(Error::Usage("logout must be the last command in the file").at(*previous));
        }
        entries.push((position, entry));
    }
    if entries.is_empty() {
        return Err(Error::Usage("command file contains no commands"));
    }
    Ok(entries)
}

fn emit(report: &Report, json: bool) -> Result<()> {
    let mut out = io::stdout().lock();
    render(report, json, &mut out)
        .and_then(|_| out.flush())
        .map_err(|_| Error::Output)
}

const COMMAND_GAP: Duration = Duration::from_secs(2);

fn wait_for_next(previous: Option<Instant>, interrupted: &AtomicBool) -> Result<()> {
    loop {
        if interrupted.load(Ordering::SeqCst) {
            return Err(Error::Interrupted);
        }
        let remaining = previous.map_or(Duration::ZERO, |end| {
            COMMAND_GAP.saturating_sub(end.elapsed())
        });
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::park_timeout(remaining);
    }
}

fn execute(
    previous: &mut Option<Instant>,
    interrupted: &AtomicBool,
    session: &Session<'_>,
    entry: Entry,
    json: bool,
    protected: &[u8],
) -> Result<bool> {
    match entry {
        Entry::Exit => return Ok(false),
        Entry::Empty => (),
        Entry::Help(text) => eprint!("{text}"),
        Entry::Command(invocation) => {
            wait_for_next(*previous, interrupted)?;
            let report = collect(session, &invocation.command, invocation.apply, protected)?;
            emit(&report, json)?;
            *previous = Some(Instant::now());
            if matches!(report, Report::Diagnostics(ref checks) if !checks.complete()) {
                return Err(Error::PartialDiagnostics);
            }
        }
    }
    Ok(true)
}

type Input = Result<Option<String>>;

fn next_line(input: &mut impl BufRead) -> Input {
    let mut line = String::new();
    match input.take(65537).read_line(&mut line) {
        Ok(0) => Ok(None),
        Ok(n) if n <= 65536 => Ok(Some(line)),
        Ok(_) => Err(Error::Usage("session command exceeds 64 KiB")),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            Err(Error::Usage("session commands must be UTF-8"))
        }
        Err(_) => Err(Error::Usage("could not read a session command")),
    }
}

fn read_lines(sender: mpsc::SyncSender<Input>) {
    let mut input = io::stdin().lock();
    loop {
        let message = next_line(&mut input);
        let end = !matches!(message, Ok(Some(_)));
        if sender.send(message).is_err() || end {
            break;
        }
    }
}

pub(crate) fn run(
    switch: &Switch,
    secret: Zeroizing<String>,
    commands: Vec<(Position, Entry)>,
    read_only: bool,
    json: bool,
    protected: &[u8],
) -> Result<()> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let interrupted = Arc::new(AtomicBool::new(false));
    let stop = interrupted.clone();
    let wake = sender.clone();
    let runner = std::thread::current();
    ctrlc::set_handler(move || {
        stop.store(true, Ordering::SeqCst);
        runner.unpark();
        let _ = wake.try_send(Err(Error::Interrupted));
    })
    .map_err(|_| Error::Usage("could not install session interrupt handler"))?;
    let session = switch.login(&secret)?;
    drop(secret);
    let check_interrupt = || {
        if interrupted.load(Ordering::SeqCst) {
            Err(Error::Interrupted)
        } else {
            Ok(())
        }
    };
    let mut previous = None;
    let result = (|| {
        if !commands.is_empty() {
            for (position, entry) in commands {
                check_interrupt()?;
                if !execute(
                    &mut previous,
                    &interrupted,
                    &session,
                    entry,
                    json,
                    protected,
                )
                .map_err(|e| e.at(position))?
                {
                    break;
                }
            }
            return check_interrupt();
        }
        let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
        if interactive {
            eprintln!("Session open. Type help for commands or exit to log out.");
        }
        std::thread::Builder::new()
            .spawn(move || read_lines(sender))
            .map_err(|_| Error::Usage("could not start session input"))?;
        for number in 1.. {
            check_interrupt()?;
            if interactive {
                eprint!("gs308e> ");
                io::stderr().flush().map_err(|_| Error::Output)?;
            }
            let Some(line) = receiver
                .recv()
                .map_err(|_| Error::Usage("session input closed unexpectedly"))??
            else {
                break;
            };
            check_interrupt()?;
            let entry = match parse(&line, read_only) {
                Ok(entry) => entry,
                Err(e) if interactive => {
                    eprintln!("error: {e}");
                    continue;
                }
                Err(e) => return Err(e.at_command(number)),
            };
            match execute(
                &mut previous,
                &interrupted,
                &session,
                entry,
                json,
                protected,
            ) {
                Ok(true) => (),
                Ok(false) => break,
                Err(e @ Error::Usage(_)) if interactive => {
                    eprintln!("error: {e}");
                    previous = Some(Instant::now());
                }
                Err(e) => return Err(e.at_command(number)),
            }
        }
        check_interrupt()
    })();
    // Cleanup precedes reporting any command failure. It uses only the original SID.
    let logout = LogoutReport::finish(session);
    let verified = logout.verified();
    let output = if json {
        emit(&Report::SessionEnd(logout), true)
    } else {
        Ok(())
    };
    result?;
    output?;
    if !verified {
        return Err(Error::Logout(
            "previous dashboard access could not be confirmed closed; allow the session to expire",
        ));
    }
    Ok(())
}
