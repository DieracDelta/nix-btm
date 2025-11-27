//! Parser for Nix internal-json log format.
//!
//! This crate provides types and parsing logic for the internal-json log format
//! emitted by Nix when using `--log-format internal-json`.
//!
//! Each line has the format: `@nix <json>`
//!
//! ## Implementation Notes
//!
//! This parser was implemented by analyzing the Nix source code directly at
//! commit [`751011168ab314ffc3dc45632bbbad71f66f1354`](https://github.com/NixOS/nix/tree/751011168ab314ffc3dc45632bbbad71f66f1354):
//!
//! - `ActivityType` enum: [logging.hh:15-30](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/include/nix/util/logging.hh#L15-L30)
//! - `ResultType` enum: [logging.hh:32-42](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/include/nix/util/logging.hh#L32-L42)
//! - `Verbosity` levels: [error.hh](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/include/nix/util/error.hh)
//!   (lvlError=0 through lvlVomit=7)
//! - `Logger::Field` type (int/string union): [logging.hh:78-103](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/include/nix/util/logging.hh#L78-L103)
//! - JSON serialization in `JSONLogger`: [logging.cc:209-338](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/logging.cc#L209-L338)
//!   - `log()` -> "msg" action: [lines 267-274](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/logging.cc#L267-L274)
//!   - `startActivity()` -> "start" action: [lines 303-319](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/logging.cc#L303-L319)
//!   - `stopActivity()` -> "stop" action: [lines 322-328](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/logging.cc#L322-L328)
//!   - `result()` -> "result" action: [lines 330-338](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/logging.cc#L330-L338)
//!   - `addFields()` helper: [lines 225-237](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/logging.cc#L225-L237)
//! - `handleJSONLogMessage()` for "setPhase": [logging.cc:441-443](https://github.com/NixOS/nix/blob/751011168ab314ffc3dc45632bbbad71f66f1354/src/libutil/logging.cc#L441-L443)
//!
//! ## Example Messages
//!
//! Real examples from `nix build --log-format internal-json`:
//!
//! ```text
//! // Start activity (type 0 = Unknown, used for evaluation)
//! @nix {"action":"start","id":5012802360049664,"level":4,"parent":0,"text":"evaluating derivation '...'","type":0}
//!
//! // Stop activity
//! @nix {"action":"stop","id":5012802360049665}
//!
//! // Start with fields (type 109 = QueryPathInfo)
//! @nix {"action":"start","fields":["/nix/store/...-bat-0.26.0","https://cache.nixos.org"],"id":5012879669460993,"level":4,"parent":0,"text":"querying info about '...'","type":109}
//!
//! // Result with progress (type 105 = Progress) - [done, expected, running, failed]
//! @nix {"action":"result","fields":[3,3,0,0],"id":5012879669460994,"type":105}
//!
//! // Log message
//! @nix {"action":"msg","level":3,"msg":"these 150 derivations will be built:"}
//! ```

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// Activity types correspond to different Nix operations.
/// These numeric values match the Nix C++ ActivityType enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum ActivityType {
    /// Unknown activity type
    Unknown = 0,
    /// Copying a path
    CopyPath = 100,
    /// Downloading a file
    FileTransfer = 101,
    /// Realising store paths
    Realise = 102,
    /// Copying multiple paths
    CopyPaths = 103,
    /// Building multiple derivations
    Builds = 104,
    /// Building a single derivation
    Build = 105,
    /// Optimising the Nix store
    OptimiseStore = 106,
    /// Verifying store paths
    VerifyPaths = 107,
    /// Substituting a path
    Substitute = 108,
    /// Querying path info from a cache
    QueryPathInfo = 109,
    /// Running post-build hook
    PostBuildHook = 110,
    /// Build waiting for a lock
    BuildWaiting = 111,
    /// Fetching a flake tree
    FetchTree = 112,
}

impl From<u32> for ActivityType {
    fn from(value: u32) -> Self {
        match value {
            0 => ActivityType::Unknown,
            100 => ActivityType::CopyPath,
            101 => ActivityType::FileTransfer,
            102 => ActivityType::Realise,
            103 => ActivityType::CopyPaths,
            104 => ActivityType::Builds,
            105 => ActivityType::Build,
            106 => ActivityType::OptimiseStore,
            107 => ActivityType::VerifyPaths,
            108 => ActivityType::Substitute,
            109 => ActivityType::QueryPathInfo,
            110 => ActivityType::PostBuildHook,
            111 => ActivityType::BuildWaiting,
            112 => ActivityType::FetchTree,
            _ => ActivityType::Unknown,
        }
    }
}

/// Result types for activity progress updates.
/// These numeric values match the Nix C++ ResultType enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum ResultType {
    /// A file was linked
    FileLinked = 100,
    /// A build log line
    BuildLogLine = 101,
    /// An untrusted path
    UntrustedPath = 102,
    /// A corrupted path
    CorruptedPath = 103,
    /// Build phase changed
    SetPhase = 104,
    /// Progress update (done, expected, running, failed)
    Progress = 105,
    /// Set expected count for an activity type
    SetExpected = 106,
    /// Post-build hook log line
    PostBuildLogLine = 107,
    /// Fetch status update
    FetchStatus = 108,
}

impl From<u32> for ResultType {
    fn from(value: u32) -> Self {
        match value {
            100 => ResultType::FileLinked,
            101 => ResultType::BuildLogLine,
            102 => ResultType::UntrustedPath,
            103 => ResultType::CorruptedPath,
            104 => ResultType::SetPhase,
            105 => ResultType::Progress,
            106 => ResultType::SetExpected,
            107 => ResultType::PostBuildLogLine,
            108 => ResultType::FetchStatus,
            _ => ResultType::Progress, // Default fallback
        }
    }
}

/// Verbosity levels for log messages.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[repr(u32)]
pub enum VerbosityLevel {
    Error = 0,
    Warn = 1,
    Notice = 2,
    Info = 3,
    Talkative = 4,
    Chatty = 5,
    Debug = 6,
    Vomit = 7,
}

/// Alias for backwards compatibility
pub type Verbosity = VerbosityLevel;

impl From<u32> for VerbosityLevel {
    fn from(value: u32) -> Self {
        match value {
            0 => VerbosityLevel::Error,
            1 => VerbosityLevel::Warn,
            2 => VerbosityLevel::Notice,
            3 => VerbosityLevel::Info,
            4 => VerbosityLevel::Talkative,
            5 => VerbosityLevel::Chatty,
            6 => VerbosityLevel::Debug,
            _ => VerbosityLevel::Vomit,
        }
    }
}

/// A field value that can be either an integer or a string (bytes).
/// This matches the format expected by the existing codebase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field<'a> {
    Int(u64),
    String(Cow<'a, [u8]>),
}

impl<'a> Field<'a> {
    /// Get this field as bytes, or None if it's an integer.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Field::String(cow) => Some(cow.as_ref()),
            Field::Int(_) => None,
        }
    }

    /// Get this field as an integer, or None if it's a string.
    pub fn as_int(&self) -> Option<u64> {
        match self {
            Field::Int(i) => Some(*i),
            Field::String(_) => None,
        }
    }

    /// Convert to owned version
    pub fn into_owned(self) -> Field<'static> {
        match self {
            Field::Int(i) => Field::Int(i),
            Field::String(cow) => Field::String(Cow::Owned(cow.into_owned())),
        }
    }
}

/// A field value that can be either an integer or a string.
/// Used for serde deserialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    Int(u64),
    String(String),
}

impl FieldValue {
    /// Get this field as a string, or None if it's an integer.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            FieldValue::String(s) => Some(s),
            FieldValue::Int(_) => None,
        }
    }

    /// Get this field as an integer, or None if it's a string.
    pub fn as_int(&self) -> Option<u64> {
        match self {
            FieldValue::Int(i) => Some(*i),
            FieldValue::String(_) => None,
        }
    }

    /// Convert to Field type
    pub fn to_field(&self) -> Field<'static> {
        match self {
            FieldValue::Int(i) => Field::Int(*i),
            FieldValue::String(s) => {
                Field::String(Cow::Owned(s.as_bytes().to_vec()))
            }
        }
    }
}

/// Activity ID - a unique identifier for a running activity.
pub type ActivityId = u64;

/// A parsed Nix internal-json log message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum NixLogMessage {
    /// An activity has started.
    #[serde(rename = "start")]
    Start {
        /// Unique ID for this activity
        id: ActivityId,
        /// Verbosity level
        level: u32,
        /// Type of activity
        #[serde(rename = "type")]
        activity_type: u32,
        /// Human-readable description
        text: String,
        /// Parent activity ID (0 if root)
        parent: ActivityId,
        /// Additional fields depending on activity type
        #[serde(default)]
        fields: Vec<FieldValue>,
    },
    /// An activity has stopped.
    #[serde(rename = "stop")]
    Stop {
        /// ID of the activity that stopped
        id: ActivityId,
    },
    /// A result/progress update for an activity.
    #[serde(rename = "result")]
    Result {
        /// ID of the activity
        id: ActivityId,
        /// Type of result
        #[serde(rename = "type")]
        result_type: u32,
        /// Result fields
        #[serde(default)]
        fields: Vec<FieldValue>,
    },
    /// A log message.
    #[serde(rename = "msg")]
    Msg {
        /// Verbosity level
        level: u32,
        /// The message text
        msg: String,
        /// Raw message (for error info)
        #[serde(default)]
        raw_msg: Option<String>,
        /// Source file line
        #[serde(default)]
        line: Option<u32>,
        /// Source file column
        #[serde(default)]
        column: Option<u32>,
        /// Source file path
        #[serde(default)]
        file: Option<String>,
        /// Stack trace for errors
        #[serde(default)]
        trace: Option<Vec<TraceFrame>>,
    },
    /// Build phase change (emitted by builders).
    #[serde(rename = "setPhase")]
    SetPhase {
        /// The new phase name
        phase: String,
    },
}

/// A stack trace frame for error messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceFrame {
    /// Raw message for this frame
    pub raw_msg: String,
    /// Source line
    pub line: Option<u32>,
    /// Source column
    pub column: Option<u32>,
    /// Source file
    pub file: Option<String>,
}

impl NixLogMessage {
    /// Parse a line from Nix internal-json output.
    /// Lines should have the format `@nix <json>`.
    pub fn parse(line: &str) -> Result<Self, ParseError> {
        let json_str = line
            .strip_prefix("@nix ")
            .ok_or(ParseError::MissingPrefix)?;

        serde_json::from_str(json_str).map_err(ParseError::Json)
    }

    /// Parse JSON directly (without the `@nix` prefix).
    pub fn parse_json(json: &str) -> Result<Self, ParseError> {
        serde_json::from_str(json).map_err(ParseError::Json)
    }

    /// Get the activity ID if this is an activity-related message.
    pub fn activity_id(&self) -> Option<ActivityId> {
        match self {
            NixLogMessage::Start { id, .. } => Some(*id),
            NixLogMessage::Stop { id } => Some(*id),
            NixLogMessage::Result { id, .. } => Some(*id),
            _ => None,
        }
    }

    /// Get the activity type if this is a Start message.
    pub fn activity_type(&self) -> Option<ActivityType> {
        match self {
            NixLogMessage::Start { activity_type, .. } => {
                Some(ActivityType::from(*activity_type))
            }
            _ => None,
        }
    }

    /// Get the result type if this is a Result message.
    pub fn result_type(&self) -> Option<ResultType> {
        match self {
            NixLogMessage::Result { result_type, .. } => {
                Some(ResultType::from(*result_type))
            }
            _ => None,
        }
    }
}

/// Errors that can occur when parsing Nix log messages.
#[derive(Debug)]
pub enum ParseError {
    /// Line doesn't start with `@nix `
    MissingPrefix,
    /// JSON parsing failed
    Json(serde_json::Error),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::MissingPrefix => {
                write!(f, "line does not start with '@nix '")
            }
            ParseError::Json(e) => write!(f, "JSON parse error: {}", e),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ParseError::Json(e) => Some(e),
            _ => None,
        }
    }
}

/// Helper to extract progress info from a Result message with Progress type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub expected: u64,
    pub running: u64,
    pub failed: u64,
}

impl Progress {
    /// Try to extract progress from result fields.
    /// Progress results have 4 integer fields: [done, expected, running,
    /// failed]
    pub fn from_fields(fields: &[FieldValue]) -> Option<Self> {
        if fields.len() >= 4 {
            Some(Progress {
                done: fields[0].as_int()?,
                expected: fields[1].as_int()?,
                running: fields[2].as_int()?,
                failed: fields[3].as_int()?,
            })
        } else {
            None
        }
    }
}

/// Helper to extract SetExpected info from a Result message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetExpected {
    pub activity_type: ActivityType,
    pub expected: u64,
}

impl SetExpected {
    /// Try to extract from result fields.
    /// SetExpected results have 2 fields: [activity_type, expected]
    pub fn from_fields(fields: &[FieldValue]) -> Option<Self> {
        if fields.len() >= 2 {
            Some(SetExpected {
                activity_type: ActivityType::from(fields[0].as_int()? as u32),
                expected: fields[1].as_int()?,
            })
        } else {
            None
        }
    }
}

/// Compatibility enum matching the API expected by the existing codebase.
/// This provides the `from_json_str` method and uses `Field<'a>` for fields.
#[derive(Debug, Clone)]
pub enum LogMessage<'a> {
    Start {
        id: u64,
        level: VerbosityLevel,
        r#type: ActivityType,
        text: Cow<'a, str>,
        parent: u64,
        fields: Option<Vec<Field<'a>>>,
    },
    Stop {
        id: u64,
    },
    Result {
        id: u64,
        r#type: ResultType,
        fields: Vec<Field<'a>>,
    },
    Msg {
        level: VerbosityLevel,
        msg: Cow<'a, str>,
    },
    SetPhase {
        phase: Cow<'a, str>,
    },
}

impl<'a> LogMessage<'a> {
    /// Parse a JSON string (without the `@nix` prefix) into a LogMessage.
    pub fn from_json_str(
        json: &str,
    ) -> Result<LogMessage<'static>, ParseError> {
        let nix_msg: NixLogMessage =
            serde_json::from_str(json).map_err(ParseError::Json)?;

        Ok(match nix_msg {
            NixLogMessage::Start {
                id,
                level,
                activity_type,
                text,
                parent,
                fields,
            } => LogMessage::Start {
                id,
                level: VerbosityLevel::from(level),
                r#type: ActivityType::from(activity_type),
                text: Cow::Owned(text),
                parent,
                fields: if fields.is_empty() {
                    None
                } else {
                    Some(fields.into_iter().map(|f| f.to_field()).collect())
                },
            },
            NixLogMessage::Stop { id } => LogMessage::Stop { id },
            NixLogMessage::Result {
                id,
                result_type,
                fields,
            } => LogMessage::Result {
                id,
                r#type: ResultType::from(result_type),
                fields: fields.into_iter().map(|f| f.to_field()).collect(),
            },
            NixLogMessage::Msg { level, msg, .. } => LogMessage::Msg {
                level: VerbosityLevel::from(level),
                msg: Cow::Owned(msg),
            },
            NixLogMessage::SetPhase { phase } => LogMessage::SetPhase {
                phase: Cow::Owned(phase),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_start_message() {
        let line = r#"@nix {"action":"start","id":5012802360049664,"level":4,"parent":0,"text":"evaluating derivation 'github:nixos/nixpkgs/master#legacyPackages.x86_64-linux.pkgsMusl.bat'","type":0}"#;
        let msg = NixLogMessage::parse(line).unwrap();

        match msg {
            NixLogMessage::Start {
                id,
                level,
                activity_type,
                text,
                parent,
                ..
            } => {
                assert_eq!(id, 5012802360049664);
                assert_eq!(level, 4);
                assert_eq!(activity_type, 0);
                assert_eq!(parent, 0);
                assert!(text.contains("evaluating derivation"));
            }
            _ => panic!("Expected Start message"),
        }
    }

    #[test]
    fn test_parse_start_with_fields() {
        let line = r#"@nix {"action":"start","fields":["/nix/store/hhidwc251z9hh1cnyhnqkp3868vk7p6g-bat-0.26.0","https://cache.nixos.org"],"id":5012879669460993,"level":4,"parent":0,"text":"querying info about '/nix/store/hhidwc251z9hh1cnyhnqkp3868vk7p6g-bat-0.26.0' on 'https://cache.nixos.org'","type":109}"#;
        let msg = NixLogMessage::parse(line).unwrap();

        match msg {
            NixLogMessage::Start {
                id,
                activity_type,
                fields,
                ..
            } => {
                assert_eq!(id, 5012879669460993);
                assert_eq!(activity_type, 109);
                assert_eq!(fields.len(), 2);
                assert_eq!(
                    fields[0].as_str(),
                    Some(
                        "/nix/store/hhidwc251z9hh1cnyhnqkp3868vk7p6g-bat-0.26.\
                         0"
                    )
                );
                assert_eq!(fields[1].as_str(), Some("https://cache.nixos.org"));
            }
            _ => panic!("Expected Start message"),
        }
    }

    #[test]
    fn test_parse_stop_message() {
        let line = r#"@nix {"action":"stop","id":5012802360049665}"#;
        let msg = NixLogMessage::parse(line).unwrap();

        match msg {
            NixLogMessage::Stop { id } => {
                assert_eq!(id, 5012802360049665);
            }
            _ => panic!("Expected Stop message"),
        }
    }

    #[test]
    fn test_parse_result_message() {
        let line = r#"@nix {"action":"result","fields":[3,3,0,0],"id":5012879669460994,"type":105}"#;
        let msg = NixLogMessage::parse(line).unwrap();

        match msg {
            NixLogMessage::Result {
                id,
                result_type,
                fields,
            } => {
                assert_eq!(id, 5012879669460994);
                assert_eq!(result_type, 105);
                let progress = Progress::from_fields(&fields).unwrap();
                assert_eq!(progress.done, 3);
                assert_eq!(progress.expected, 3);
                assert_eq!(progress.running, 0);
                assert_eq!(progress.failed, 0);
            }
            _ => panic!("Expected Result message"),
        }
    }

    #[test]
    fn test_parse_msg_message() {
        let line = r#"@nix {"action":"msg","level":3,"msg":"these 150 derivations will be built:"}"#;
        let msg = NixLogMessage::parse(line).unwrap();

        match msg {
            NixLogMessage::Msg { level, msg, .. } => {
                assert_eq!(level, 3);
                assert!(msg.contains("150 derivations"));
            }
            _ => panic!("Expected Msg message"),
        }
    }

    #[test]
    fn test_activity_type_conversion() {
        assert_eq!(ActivityType::from(105), ActivityType::Build);
        assert_eq!(ActivityType::from(109), ActivityType::QueryPathInfo);
        assert_eq!(ActivityType::from(101), ActivityType::FileTransfer);
        assert_eq!(ActivityType::from(9999), ActivityType::Unknown);
    }

    #[test]
    fn test_result_type_conversion() {
        assert_eq!(ResultType::from(105), ResultType::Progress);
        assert_eq!(ResultType::from(101), ResultType::BuildLogLine);
        assert_eq!(ResultType::from(104), ResultType::SetPhase);
    }

    #[test]
    fn test_verbosity_ordering() {
        assert!(Verbosity::Error < Verbosity::Warn);
        assert!(Verbosity::Warn < Verbosity::Info);
        assert!(Verbosity::Info < Verbosity::Debug);
    }

    #[test]
    fn test_missing_prefix_error() {
        let line = r#"{"action":"stop","id":123}"#;
        match NixLogMessage::parse(line) {
            Err(ParseError::MissingPrefix) => {}
            _ => panic!("Expected MissingPrefix error"),
        }
    }

    #[test]
    fn test_parse_json_directly() {
        let json = r#"{"action":"stop","id":123}"#;
        let msg = NixLogMessage::parse_json(json).unwrap();
        match msg {
            NixLogMessage::Stop { id } => assert_eq!(id, 123),
            _ => panic!("Expected Stop message"),
        }
    }
}
