//! The record: one item of the stream, whether a log line, a cluster event
//! or a notice from Gossip itself.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The value of a structured field. Nested objects and arrays of a JSON line
/// are stored as JSON text, so in practice a field is a string, a number, a
/// boolean or null.
pub use serde_json::Value;

/// One item of the stream.
///
/// Every record knows where it came from, when, what kind of thing it is, a
/// level when one is known, a message, the structured fields when there are
/// any, and the raw line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The cluster, named after its kubeconfig context.
    pub cluster: String,
    pub namespace: String,
    /// The pod, or an empty string for a notice that is about no single pod.
    pub pod: String,
    /// The container; none for events and for notices about a whole pod.
    pub container: Option<String>,
    /// The kubelet's time for lines, the event's time for events, the local
    /// clock for notices.
    pub time: DateTime<Utc>,
    pub kind: Kind,
    /// Which recogniser accepted the line; `Text` for events and notices.
    pub format: Format,
    pub level: Option<Level>,
    pub message: String,
    pub fields: BTreeMap<String, Value>,
    /// The line as it came, for a raw output.
    pub raw: String,
    /// The line belongs to the run of the container before the current one.
    pub previous: bool,
    /// The line was longer than the limit and has been cut.
    pub truncated: bool,
}

impl Record {
    /// A notice: something the core wants to tell the user.
    pub fn notice(
        cluster: impl Into<String>,
        namespace: impl Into<String>,
        pod: impl Into<String>,
        container: Option<String>,
        time: DateTime<Utc>,
        message: impl Into<String>,
    ) -> Self {
        let message = message.into();
        Record {
            cluster: cluster.into(),
            namespace: namespace.into(),
            pod: pod.into(),
            container,
            time,
            kind: Kind::Notice,
            format: Format::Text,
            level: None,
            raw: message.clone(),
            message,
            fields: BTreeMap::new(),
            previous: false,
            truncated: false,
        }
    }
}

/// The keys of the fields a record of kind `event` carries.
pub mod event_field {
    /// Why it happened, in one word: `Scheduled`, `Killing`, `BackOff`, ...
    pub const REASON: &str = "reason";
    /// `Normal` or `Warning`.
    pub const TYPE: &str = "type";
    /// How often it has happened.
    pub const COUNT: &str = "count";
    /// The object it is about, as kind and name: `Deployment/flowcpt-api`.
    pub const REGARDING: &str = "regarding";
}

/// The text of a field value, as filters compare it and as it is shown:
/// a string without its quotes, anything else as JSON writes it.
pub fn value_text(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(text) => Cow::Borrowed(text),
        other => Cow::Owned(other.to_string()),
    }
}

/// What kind of thing a record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A log line of a container.
    Line,
    /// A cluster event about a followed pod or the workload above it.
    Event,
    /// A message from Gossip itself.
    Notice,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Line => "line",
            Kind::Event => "event",
            Kind::Notice => "notice",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A word that names no record kind.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("`{0}` is not a record kind; the kinds are line, event and notice")]
pub struct UnknownKind(pub String);

impl FromStr for Kind {
    type Err = UnknownKind;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "line" => Ok(Kind::Line),
            "event" => Ok(Kind::Event),
            "notice" => Ok(Kind::Notice),
            _ => Err(UnknownKind(s.to_string())),
        }
    }
}

/// Which recogniser accepted a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Json,
    Logfmt,
    Text,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Logfmt => "logfmt",
            Format::Text => "text",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The level of a record, ordered from `Trace` up to `Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "trace",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    /// The level a word stands for, in any letter case. Besides the five
    /// names this knows the usual spellings of other loggers and maps
    /// everything above an error (`fatal`, `critical`, `panic`, ...) to
    /// `Error`.
    pub fn from_word(word: &str) -> Option<Level> {
        let level = match word.to_ascii_lowercase().as_str() {
            "trace" => Level::Trace,
            "debug" => Level::Debug,
            "info" | "information" | "informational" | "notice" => Level::Info,
            "warn" | "warning" => Level::Warn,
            "error" | "err" | "fatal" | "critical" | "crit" | "panic" | "alert" | "emergency"
            | "emerg" => Level::Error,
            _ => return None,
        };
        Some(level)
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A word that names no level.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("`{0}` is not a level; the levels are trace, debug, info, warn and error")]
pub struct UnknownLevel(pub String);

impl FromStr for Level {
    type Err = UnknownLevel;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Level::from_word(s).ok_or_else(|| UnknownLevel(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn sample() -> Record {
        let mut fields = BTreeMap::new();
        fields.insert("tenant_id".to_string(), json!("t_123"));
        fields.insert("status".to_string(), json!(200));
        Record {
            cluster: "aks-vulcan-01".into(),
            namespace: "flowcpt".into(),
            pod: "flowcpt-api-7d9f-xk2p1".into(),
            container: Some("api".into()),
            time: Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap(),
            kind: Kind::Line,
            format: Format::Json,
            level: Some(Level::Warn),
            message: "payment intent expired".into(),
            fields,
            raw: r#"{"level":"WARN"}"#.into(),
            previous: false,
            truncated: false,
        }
    }

    #[test]
    fn levels_are_ordered_from_trace_to_error() {
        assert!(Level::Trace < Level::Debug);
        assert!(Level::Debug < Level::Info);
        assert!(Level::Info < Level::Warn);
        assert!(Level::Warn < Level::Error);
    }

    #[test]
    fn levels_parse_in_any_case_and_spelling() {
        assert_eq!("WARN".parse::<Level>(), Ok(Level::Warn));
        assert_eq!("warning".parse::<Level>(), Ok(Level::Warn));
        assert_eq!("Info".parse::<Level>(), Ok(Level::Info));
        assert_eq!("FATAL".parse::<Level>(), Ok(Level::Error));
        assert_eq!("err".parse::<Level>(), Ok(Level::Error));
        assert!("loud".parse::<Level>().is_err());
        assert!("".parse::<Level>().is_err());
    }

    #[test]
    fn kinds_parse_in_any_case() {
        assert_eq!("event".parse::<Kind>(), Ok(Kind::Event));
        assert_eq!("Notice".parse::<Kind>(), Ok(Kind::Notice));
        assert!("text".parse::<Kind>().is_err());
    }

    #[test]
    fn a_record_serialises_every_field() {
        let value = serde_json::to_value(sample()).unwrap();
        let object = value.as_object().unwrap();
        let keys: Vec<&str> = object.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "cluster",
                "container",
                "fields",
                "format",
                "kind",
                "level",
                "message",
                "namespace",
                "pod",
                "previous",
                "raw",
                "time",
                "truncated",
            ]
        );
        assert_eq!(object["kind"], "line");
        assert_eq!(object["format"], "json");
        assert_eq!(object["level"], "warn");
        assert_eq!(object["time"], "2026-11-18T14:02:11Z");
        assert_eq!(object["fields"]["status"], 200);
    }

    #[test]
    fn a_record_survives_a_round_trip_through_json() {
        let record = sample();
        let text = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<Record>(&text).unwrap(), record);
    }

    #[test]
    fn a_notice_has_no_level_and_no_fields() {
        let time = Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap();
        let notice = Record::notice("c", "ns", "", None, time, "target matches nothing");
        assert_eq!(notice.kind, Kind::Notice);
        assert_eq!(notice.format, Format::Text);
        assert_eq!(notice.level, None);
        assert!(notice.fields.is_empty());
        assert_eq!(notice.raw, "target matches nothing");
    }

    #[test]
    fn value_text_drops_the_quotes_of_a_string_only() {
        assert_eq!(value_text(&json!("t_123")), "t_123");
        assert_eq!(value_text(&json!(200)), "200");
        assert_eq!(value_text(&json!(true)), "true");
        assert_eq!(value_text(&json!(null)), "null");
    }
}
