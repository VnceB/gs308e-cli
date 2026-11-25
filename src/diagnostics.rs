use crate::error::{Error, Result};
use crate::maintenance::{self, CableResult};
use crate::model::{self, Port, Vlan};
use crate::session::{Page, Session};
use crate::vlan::Table;
use serde::Serialize;
use std::io::{self, Write};

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum Check<T> {
    Ok { data: T },
    Error { exit_code: u8, message: String },
}

impl<T> Check<T> {
    fn error(error: &Error) -> Self {
        Self::Error {
            exit_code: error.code(),
            message: error.to_string(),
        }
    }

    fn passed(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    fn render<W: Write>(
        &self,
        title: &str,
        out: &mut W,
        show: impl FnOnce(&T, &mut W) -> io::Result<()>,
    ) -> io::Result<()> {
        match self {
            Self::Ok { data } => {
                writeln!(out, "{title}")?;
                show(data, out)
            }
            Self::Error { message, .. } => writeln!(out, "{title} failed: {message}"),
        }
    }
}

impl<T> From<Result<T>> for Check<T> {
    fn from(result: Result<T>) -> Self {
        match result {
            Ok(data) => Self::Ok { data },
            Err(error) => Self::error(&error),
        }
    }
}

#[derive(Serialize)]
pub struct Diagnostics {
    ports: Check<Vec<Port>>,
    vlan_mode: Check<Vlan>,
    vlan_list: Check<Table>,
    cable_results: Check<Vec<CableResult>>,
    #[serde(flatten)]
    logout: Option<LogoutReport>,
}

#[derive(Serialize)]
pub struct LogoutReport {
    logout_request: Check<u16>,
    logout_verification: Check<&'static str>,
}

impl Diagnostics {
    pub fn read(session: &Session<'_>, dashboard: &scraper::Html) -> Self {
        let ports = model::ports(dashboard).into();
        let (vlan_mode, vlan_list) = match session.read(Page::Vlan) {
            Ok(page) => (model::vlan(&page).into(), Table::parse(&page).into()),
            Err(error) => (Check::error(&error), Check::error(&error)),
        };
        let cable_results = session
            .read(Page::Cable)
            .and_then(|page| maintenance::cable_results(&page, session.binding()))
            .into();
        Self {
            ports,
            vlan_mode,
            vlan_list,
            cable_results,
            logout: None,
        }
    }

    pub fn run(session: Session<'_>) -> Result<Self> {
        let reads = session
            .dashboard()
            .map(|dashboard| Self::read(&session, &dashboard));
        let logout = LogoutReport::finish(session);
        if reads.is_err()
            && let Check::Error { message, .. } = &logout.logout_verification
        {
            eprintln!("warning: could not confirm logout ({message})");
        }
        let mut reads = reads?;
        reads.logout = Some(logout);
        Ok(reads)
    }

    pub fn complete(&self) -> bool {
        self.ports.passed()
            && self.vlan_mode.passed()
            && self.vlan_list.passed()
            && self.cable_results.passed()
            && self.logout.as_ref().is_none_or(LogoutReport::verified)
    }

    pub fn render(&self, out: &mut impl Write) -> io::Result<()> {
        self.ports
            .render("Ports", out, |rows, out| crate::render_ports(rows, out))?;
        self.vlan_mode
            .render("VLAN mode", out, crate::render_vlan)?;
        self.vlan_list
            .render("VLAN list", out, |table, out| write!(out, "{table}"))?;
        self.cable_results
            .render("Stored cable results", out, |rows, out| {
                crate::render_cable_results(rows, out)
            })?;
        if let Some(logout) = &self.logout
            && !logout.verified()
        {
            logout.render(out)?;
        }
        Ok(())
    }
}

impl LogoutReport {
    pub fn finish(session: Session<'_>) -> Self {
        let (post, check) = session.logout_and_verify();
        Self {
            logout_request: post.into(),
            logout_verification: check.into(),
        }
    }

    pub fn verified(&self) -> bool {
        self.logout_verification.passed()
    }

    pub fn render(&self, out: &mut impl Write) -> io::Result<()> {
        if !self.verified() {
            self.logout_verification
                .render("Logout", out, |evidence, out| writeln!(out, "{evidence}"))?;
        }
        Ok(())
    }
}
