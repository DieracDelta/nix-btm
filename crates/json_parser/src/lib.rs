//! COPY OF SNIX/TWIX LOGGING MODULE
//! Had to fork because the field parsing was rejecting non utf8 strings
//! which resulted in the entire message failing to parse
//! we need to use the entire message even if one of the fields isn't utf8.
//! Contains types Nix uses for its logging, visible in the "internal-json" log
//! messages as well as in nix-daemon communication.

use std::borrow::Cow;

use bstr::{BStr, BString};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
//use tracing::warn;

/// The different verbosity levels Nix distinguishes.
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Default,
    num_enum::TryFromPrimitive,
    num_enum::IntoPrimitive,
    Serialize,
    Deserialize,
)]
#[serde(try_from = "u64", into = "u64")]
#[repr(u64)]
pub enum VerbosityLevel {
    #[default]
    Error = 0,
    Warn = 1,
    Notice = 2,
    Info = 3,
    Talkative = 4,
    Chatty = 5,
    Debug = 6,
    Vomit = 7,
}

impl std::fmt::Display for VerbosityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                VerbosityLevel::Error => "error",
                VerbosityLevel::Warn => "warn",
                VerbosityLevel::Notice => "notice",
                VerbosityLevel::Info => "info",
                VerbosityLevel::Talkative => "talkative",
                VerbosityLevel::Chatty => "chatty",
                VerbosityLevel::Debug => "debug",
                VerbosityLevel::Vomit => "vomit",
            }
        )
    }
}

/// The different types of log messages Nix' `internal-json` format can
/// represent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase" /*, deny_unknown_fields */)]
// TODO: deny_unknown_fields doesn't seem to work in the testcases below
pub enum LogMessage<'a> {
    Start {
        #[serde(skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<Field<'a>>>,
        id: u64,
        level: VerbosityLevel,
        parent: u64,
        text: std::borrow::Cow<'a, str>,
        r#type: ActivityType,
    },

    Stop {
        id: u64,
    },

    Result {
        fields: Vec<Field<'a>>,
        id: u64,
        r#type: ResultType,
    },

    // FUTUREWORK: there sometimes seems to be column/file/line fields set to
    // null, and a raw_msg field, see msg_with_raw_msg testcase. These
    // should be represented.
    Msg {
        level: VerbosityLevel,
        msg: std::borrow::Cow<'a, str>,
    },

    // Log lines like these are sent by nixpkgs stdenv, present in `nix log`
    // outputs of individual builds. They are also interpreted by Nix to
    // re-emit [Self::Result]-style messages.
    SetPhase {
        phase: &'a str,
    },
}
use bstr::ByteSlice;
pub fn serialize<S>(cow: &BStr, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    // Encode as bytes (JSON: string if UTF-8, otherwise array) via serde_bytes
    serde_bytes::serialize(cow.as_bytes(), s)
}

pub fn deserialize<'de, S>(d: S) -> Result<Cow<'de, BStr>, S::Error>
where
    S: Deserializer<'de>,
{
    // First deserialize as bytes (borrow when possible)
    let cow_bytes: Cow<'de, [u8]> = serde_bytes::deserialize(d)?;

    // Map Cow<[u8]> → Cow<BStr> without copying when borrowed
    Ok(match cow_bytes {
        Cow::Borrowed(b) => Cow::Borrowed(BStr::new(b)),
        Cow::Owned(v) => Cow::Owned(BString::from(v)),
    })
}
mod serde_cow_bstr {

    use super::*;

    pub fn serialize<S>(cow: &BStr, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Either import ByteSlice (as above) and use cow.as_bytes(),
        // or do: serde_bytes::serialize(cow.as_ref().as_ref(), s)
        serde_bytes::serialize(cow.as_bytes(), s)
    }

    pub fn deserialize<'de, S>(d: S) -> Result<Cow<'de, BStr>, S::Error>
    where
        S: Deserializer<'de>,
    {
        // Borrow when possible
        let cow_bytes: Cow<'de, [u8]> = serde_bytes::deserialize(d)?;
        Ok(match cow_bytes {
            Cow::Borrowed(b) => Cow::Borrowed(BStr::new(b)),
            Cow::Owned(v) => Cow::Owned(BString::from(v)),
        })
    }
}

/// Fields in a log message can be either ints or strings.
/// Sometimes, Nix also uses invalid UTF-8 in here, so we use BStr.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Field<'a> {
    Int(u64),
    String(
        #[serde(with = "serde_cow_bstr", borrow)] std::borrow::Cow<'a, BStr>,
    ),
}

#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    num_enum::TryFromPrimitive,
    num_enum::IntoPrimitive,
    Serialize,
    Deserialize,
)]
#[serde(try_from = "u8", into = "u8")]
#[repr(u8)]
pub enum ActivityType {
    Unknown = 0,
    CopyPath = 100,
    FileTransfer = 101,
    Realise = 102,
    CopyPaths = 103,
    Builds = 104,
    Build = 105,
    OptimiseStore = 106,
    VerifyPaths = 107,
    Substitute = 108,
    QueryPathInfo = 109,
    PostBuildHook = 110,
    BuildWaiting = 111,
    FetchTree = 112,
}

impl std::fmt::Display for ActivityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ActivityType::Unknown => "unknown",
                ActivityType::CopyPath => "copy-path",
                ActivityType::FileTransfer => "file-transfer",
                ActivityType::Realise => "realise",
                ActivityType::CopyPaths => "copy-paths",
                ActivityType::Builds => "builds",
                ActivityType::Build => "build",
                ActivityType::OptimiseStore => "optimise-store",
                ActivityType::VerifyPaths => "verify-paths",
                ActivityType::Substitute => "substitute",
                ActivityType::QueryPathInfo => "query-path-info",
                ActivityType::PostBuildHook => "post-build-hook",
                ActivityType::BuildWaiting => "build-waiting",
                ActivityType::FetchTree => "fetch-tree",
            }
        )
    }
}

#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    num_enum::TryFromPrimitive,
    num_enum::IntoPrimitive,
    Serialize,
    Deserialize,
)]
#[serde(try_from = "u8", into = "u8")]
#[repr(u8)]
pub enum ResultType {
    FileLinked = 100,
    BuildLogLine = 101,
    UntrustedPath = 102,
    CorruptedPath = 103,
    SetPhase = 104,
    Progress = 105,
    SetExpected = 106,
    PostBuildLogLine = 107,
    FetchStatus = 108,
}

impl<'a> LogMessage<'a> {
    pub fn from_json_str(s: &'a str) -> Result<Self, Error> {
        Ok(serde_json::from_str(s)?)
    }
}

impl std::fmt::Display for LogMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(self)
                .expect("Failed to serialize LogMessage")
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to deserialize: {0}")]
    FailedDeserialize(#[from] serde_json::Error),
}
