use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Error)]
pub enum Position {
    #[error("session command {0}")]
    Command(usize),
    #[error("command file line {0}")]
    FileLine(usize),
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Usage(&'static str),
    #[error("{0}")]
    CommandLine(String),
    #[error("session interrupted")]
    Interrupted,
    #[error("{position}: {cause}")]
    SessionCommand {
        position: Position,
        cause: Box<Error>,
    },
    #[error("sign-in failed or the session expired")]
    Auth,
    #[error("the lifetime issued for this session cookie has ended")]
    ExpiredCookie,
    #[error("{method} {path}: the switch denied the request (HTTP {status})")]
    Denied {
        method: &'static str,
        path: &'static str,
        status: u16,
    },
    #[error(
        "POST /login.cgi: {reply} (HTTP {status}) without a usable SID session cookie ({cookie}). Check the reported cookie category in the sign-in guide"
    )]
    NoSession {
        reply: &'static str,
        status: u16,
        cookie: &'static str,
    },
    #[error(
        "GET {path}: the switch returned its login page after issuing a session cookie. End this session and sign in again when a session slot is available"
    )]
    LoginPage { path: &'static str },
    #[error("port editor field {field}: {reason}")]
    PortField {
        field: &'static str,
        reason: &'static str,
    },
    #[error("{method} /login.cgi: {reason}")]
    LoginNotice {
        method: &'static str,
        reason: &'static str,
    },
    #[error("{0}")]
    Network(&'static str),
    #[error("{method} {path}: {reason}")]
    Request {
        method: &'static str,
        path: &'static str,
        reason: &'static str,
    },
    #[error("{method} {path}: unexpected HTTP status {status}")]
    Status {
        method: &'static str,
        path: &'static str,
        status: u16,
    },
    #[error("the switch returned an HTTP redirect; specify its final management origin directly")]
    Redirect,
    #[error("unsupported switch model; this release supports the GS308Ev4")]
    Model,
    #[error("{}", firmware_error(.0.as_deref()))]
    Firmware(Option<String>),
    #[error("unrecognized switch response ({0}); check the model and firmware")]
    Page(&'static str),
    #[error("could not verify the submitted change; inspect current state before retrying")]
    Unverified,
    #[error("logout verification: {0}")]
    Logout(&'static str),
    #[error("diagnostics incomplete; see the individual checks in the report")]
    PartialDiagnostics,
    #[error("output failed")]
    Output,
}

impl Error {
    pub fn at_command(self, number: usize) -> Self {
        self.at(Position::Command(number))
    }
    pub fn at(self, position: Position) -> Self {
        Self::SessionCommand {
            position,
            cause: Box::new(self),
        }
    }
    pub fn code(&self) -> u8 {
        match self {
            Self::Usage(_) | Self::CommandLine(_) => 2,
            Self::Interrupted => 130,
            Self::SessionCommand { cause, .. } => cause.code(),
            Self::Auth
            | Self::ExpiredCookie
            | Self::LoginNotice { .. }
            | Self::Denied { .. }
            | Self::NoSession { .. }
            | Self::LoginPage { .. } => 3,
            Self::Network(_)
            | Self::Request { .. }
            | Self::Status { .. }
            | Self::Redirect
            | Self::Unverified
            | Self::Logout(_) => 4,
            Self::PartialDiagnostics => 6,
            Self::Model | Self::Firmware(_) | Self::Page(_) | Self::PortField { .. } => 5,
            Self::Output => 1,
        }
    }
}

fn firmware_error(detected: Option<&str>) -> String {
    match detected {
        Some(version) => format!(
            "unsupported switch firmware ({version}); this release requires GS308Ev4 firmware V1.0.1.13 or newer"
        ),
        None => {
            "unsupported switch firmware; this release requires GS308Ev4 firmware V1.0.1.13 or newer"
                .to_owned()
        }
    }
}
