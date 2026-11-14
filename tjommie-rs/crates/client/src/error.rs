use std::fmt;

use protocol::ErrorCode;

/// Why a connection ended: a WebSocket close code and reason, and whether the connection had opened (`opened`: the daemon
/// had welcomed us). `1006` is a connection that dropped without a close frame, `1000` a close the client asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseReason {
    pub code: u16,
    pub reason: String,
    pub opened: bool,
}

impl CloseReason {
    pub(crate) fn new(code: u16, reason: impl Into<String>, opened: bool) -> Self {
        Self {
            code,
            reason: reason.into(),
            opened,
        }
    }
}

impl fmt::Display for CloseReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = if self.reason.is_empty() {
            "none"
        } else {
            &self.reason
        };
        write!(
            formatter,
            "WebSocket closed (code {}, reason: {reason})",
            self.code
        )
    }
}

/// Why [`crate::Client::connect`] failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// The daemon closed with 4401: the token is wrong (or the hello never came).
    Unauthorized,
    /// The daemon closed with 4003: it speaks another protocol version.
    ProtocolMismatch,
    /// The WebSocket upgrade was refused with this HTTP status.
    Refused(u16),
    /// Nothing answered at the address.
    Unreachable(String),
    /// The daemon sent something that isn't the welcome.
    BadFrame(String),
    /// No welcome in time (4408 in the TypeScript client).
    TimedOut,
    /// The daemon closed the connection before welcoming, for another reason.
    Closed(CloseReason),
}

impl fmt::Display for ConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized => formatter.write_str("The daemon refused the token."),
            Self::ProtocolMismatch => {
                formatter.write_str("The daemon speaks another protocol version.")
            }
            Self::Refused(status) => {
                write!(formatter, "The daemon refused the connection ({status}).")
            }
            Self::Unreachable(why) => write!(formatter, "Couldn't reach the daemon: {why}"),
            Self::BadFrame(why) => write!(formatter, "Invalid server frame: {why}"),
            Self::TimedOut => formatter.write_str("The daemon didn't welcome the client in time."),
            Self::Closed(reason) => reason.fmt(formatter),
        }
    }
}

impl std::error::Error for ConnectError {}

/// Why a [`crate::Client::call`] failed.
#[derive(Debug, Clone, PartialEq)]
pub enum RpcError {
    /// The daemon answered with an error.
    Daemon { code: ErrorCode, message: String },
    /// The connection ended before the answer came (or had ended already).
    Closed(CloseReason),
    /// The params couldn't be written as JSON.
    BadParams(String),
    /// The answer passed the contract but isn't the method's result type.
    BadResult(String),
}

impl fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Daemon { code, message } => write!(formatter, "{code:?}: {message}"),
            Self::Closed(reason) => reason.fmt(formatter),
            Self::BadParams(why) => write!(formatter, "Couldn't write the params: {why}"),
            Self::BadResult(why) => write!(formatter, "Couldn't read the result: {why}"),
        }
    }
}

impl std::error::Error for RpcError {}
