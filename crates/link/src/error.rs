//! What can go wrong on the link.

use crate::noise::WrongKey;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    /// Noise failed: a damaged or tampered message, or a bad key.
    Noise(snow::Error),
    /// The responder answered with a key other than the one expected.
    WrongKey(WrongKey),
    /// A message that doesn't follow the protocol.
    Malformed(String),
    /// A message over the size allowed.
    TooLarge { limit: usize },
    /// A damaged stored key.
    BadKey(String),
    /// Pairing couldn't go on.
    Pairing(String),
    /// No randomness from the system.
    Random(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Noise(e) => write!(f, "noise: {e}"),
            Error::WrongKey(w) => w.fmt(f),
            Error::Malformed(why) => write!(f, "malformed message: {why}"),
            Error::TooLarge { limit } => write!(f, "message over {limit} bytes"),
            Error::BadKey(why) => write!(f, "bad key: {why}"),
            Error::Pairing(why) => write!(f, "pairing: {why}"),
            Error::Random(why) => write!(f, "randomness: {why}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Noise(e) => Some(e),
            Error::WrongKey(w) => Some(w),
            _ => None,
        }
    }
}

impl From<snow::Error> for Error {
    fn from(e: snow::Error) -> Self {
        Error::Noise(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Malformed(e.to_string())
    }
}
