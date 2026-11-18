//! The line parser: turns the lines of one container into parsed lines.
//!
//! The format is detected per line. Three recognisers run in order, JSON,
//! logfmt, then text, and the first that accepts the line wins. Before them
//! comes the continuation rule: a line that starts with whitespace, with
//! `at ` or with `Caused by:` belongs to the line before it.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::record::{Format, Level, Value, value_text};

/// Lines longer than this are cut, and a record never grows beyond it
/// through continuation lines.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// A continuation line joins the line before it only when it is no more than
/// this much younger than that line.
const CONTINUATION_WINDOW: TimeDelta = TimeDelta::seconds(2);

/// The key under which the application's own timestamp is kept in the fields
/// of a line, whichever of `timestamp`, `ts` or `time` the line used.
pub const TIMESTAMP_FIELD: &str = "timestamp";

const LEVEL_KEYS: [&str; 3] = ["level", "severity", "lvl"];
const MESSAGE_KEYS: [&str; 2] = ["message", "msg"];
const OTHER_TIMESTAMP_KEYS: [&str; 2] = ["ts", "time"];

/// The `format` option: which recogniser to use for a stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormatChoice {
    /// Detect the format of every line.
    #[default]
    Auto,
    /// Only the JSON recogniser; a line it refuses is text.
    Json,
    /// Only the logfmt recogniser; a line it refuses is text.
    Logfmt,
    /// Every line is text, and the continuation rule is off.
    Text,
}

impl FormatChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            FormatChoice::Auto => "auto",
            FormatChoice::Json => "json",
            FormatChoice::Logfmt => "logfmt",
            FormatChoice::Text => "text",
        }
    }
}

impl fmt::Display for FormatChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A word that names no format.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("`{0}` is not a format; the formats are auto, json, logfmt and text")]
pub struct UnknownFormat(pub String);

impl FromStr for FormatChoice {
    type Err = UnknownFormat;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(FormatChoice::Auto),
            "json" => Ok(FormatChoice::Json),
            "logfmt" => Ok(FormatChoice::Logfmt),
            "text" => Ok(FormatChoice::Text),
            _ => Err(UnknownFormat(s.to_string())),
        }
    }
}

/// A line after parsing: everything of a record that the line itself decides.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedLine {
    /// The time of the line, or of the first line when continuation lines
    /// were joined to it.
    pub time: DateTime<Utc>,
    pub format: Format,
    pub level: Option<Level>,
    pub message: String,
    pub fields: BTreeMap<String, Value>,
    /// The line as it came; continuation lines are joined with newlines.
    pub raw: String,
    /// The line, or one of its continuation lines, was cut.
    pub truncated: bool,
}

/// Parses the lines of one container, in the order they were written.
///
/// Because a later line may still belong to an earlier one, the parser keeps
/// the newest line back. [`push`](LineParser::push) hands out the line
/// before it once it is complete, and [`flush`](LineParser::flush) hands out
/// the one kept back, for when the stream ends or goes quiet.
#[derive(Debug)]
pub struct LineParser {
    choice: FormatChoice,
    pending: Option<ParsedLine>,
}

impl LineParser {
    pub fn new(choice: FormatChoice) -> Self {
        LineParser {
            choice,
            pending: None,
        }
    }

    /// Takes the next line of the container, without its line ending, and
    /// the time the kubelet gave it. Returns the line before it when that
    /// one is now complete.
    pub fn push(&mut self, time: DateTime<Utc>, line: &str) -> Option<ParsedLine> {
        let (line, truncated) = cut(line);

        if self.choice != FormatChoice::Text && is_continuation(line) {
            if let Some(pending) = &mut self.pending {
                let young = time.signed_duration_since(pending.time) <= CONTINUATION_WINDOW;
                let fits = pending.raw.len() + 1 + line.len() <= MAX_LINE_BYTES;
                if young && fits {
                    pending.message.push('\n');
                    pending.message.push_str(line);
                    pending.raw.push('\n');
                    pending.raw.push_str(line);
                    pending.truncated |= truncated;
                    return None;
                }
            }
            // Too old, or nothing to join: text on its own.
            return self.pending.replace(text_line(time, line, truncated));
        }

        self.pending
            .replace(recognise(self.choice, time, line, truncated))
    }

    /// Hands out the line kept back, if there is one.
    pub fn flush(&mut self) -> Option<ParsedLine> {
        self.pending.take()
    }

    /// Whether a line is kept back.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// Cuts a line that is longer than the limit, on a character boundary.
fn cut(line: &str) -> (&str, bool) {
    if line.len() <= MAX_LINE_BYTES {
        return (line, false);
    }
    let mut end = MAX_LINE_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    (&line[..end], true)
}

fn is_continuation(line: &str) -> bool {
    line.starts_with(char::is_whitespace)
        || line.starts_with("at ")
        || line.starts_with("Caused by:")
}

fn recognise(choice: FormatChoice, time: DateTime<Utc>, line: &str, truncated: bool) -> ParsedLine {
    let structured = match choice {
        FormatChoice::Auto => parse_json(line)
            .map(|fields| (Format::Json, fields))
            .or_else(|| parse_logfmt(line).map(|fields| (Format::Logfmt, fields))),
        FormatChoice::Json => parse_json(line).map(|fields| (Format::Json, fields)),
        FormatChoice::Logfmt => parse_logfmt(line).map(|fields| (Format::Logfmt, fields)),
        FormatChoice::Text => None,
    };
    match structured {
        Some((format, mut fields)) => {
            let (level, message) = normalise(&mut fields);
            ParsedLine {
                time,
                format,
                level,
                message,
                fields,
                raw: line.to_string(),
                truncated,
            }
        }
        None => text_line(time, line, truncated),
    }
}

fn text_line(time: DateTime<Utc>, line: &str, truncated: bool) -> ParsedLine {
    ParsedLine {
        time,
        format: Format::Text,
        level: guess_level(line),
        message: line.to_string(),
        fields: BTreeMap::new(),
        raw: line.to_string(),
        truncated,
    }
}

/// Moves the well-known keys out of the fields: the level and the message
/// into their own places, the application's timestamp under one name.
fn normalise(fields: &mut BTreeMap<String, Value>) -> (Option<Level>, String) {
    let mut level = None;
    for key in LEVEL_KEYS {
        if let Some(Value::String(word)) = fields.get(key)
            && let Some(found) = Level::from_word(word)
        {
            level = Some(found);
            fields.remove(key);
            break;
        }
    }

    let mut message = String::new();
    for key in MESSAGE_KEYS {
        if let Some(value) = fields.remove(key) {
            message = value_text(&value).into_owned();
            break;
        }
    }

    if !fields.contains_key(TIMESTAMP_FIELD) {
        for key in OTHER_TIMESTAMP_KEYS {
            if let Some(value) = fields.remove(key) {
                fields.insert(TIMESTAMP_FIELD.to_string(), value);
                break;
            }
        }
    }

    (level, message)
}

/// The JSON recogniser: a line that starts with `{` and is one JSON object.
/// Its top-level keys become the fields; nested objects and arrays are kept
/// as JSON text.
fn parse_json(line: &str) -> Option<BTreeMap<String, Value>> {
    if !line.starts_with('{') {
        return None;
    }
    let object: serde_json::Map<String, Value> = serde_json::from_str(line).ok()?;
    let fields = object
        .into_iter()
        .map(|(key, value)| {
            let value = match value {
                Value::Object(_) | Value::Array(_) => Value::String(value.to_string()),
                scalar => scalar,
            };
            (key, value)
        })
        .collect();
    Some(fields)
}

/// The logfmt recogniser: a line made of `key=value` pairs and nothing else.
/// A value is a bare word or a quoted string.
fn parse_logfmt(line: &str) -> Option<BTreeMap<String, Value>> {
    let mut fields = BTreeMap::new();
    let mut rest = line.trim();
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let key_end = rest.find(|c: char| c == '=' || c.is_whitespace())?;
        if !rest[key_end..].starts_with('=') {
            return None;
        }
        let key = &rest[..key_end];
        if !is_logfmt_key(key) {
            return None;
        }
        rest = &rest[key_end + 1..];

        let value = if let Some(quoted) = rest.strip_prefix('"') {
            let (value, after) = read_quoted(quoted)?;
            if !(after.is_empty() || after.starts_with(char::is_whitespace)) {
                return None;
            }
            rest = after;
            value
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let value = rest[..end].to_string();
            rest = &rest[end..];
            value
        };
        fields.insert(key.to_string(), Value::String(value));
        rest = rest.trim_start();
    }
    Some(fields)
}

fn is_logfmt_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|first| first.is_alphanumeric() || first == '_')
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '@'))
}

/// Reads a quoted logfmt value after its opening quote. Returns the value
/// and what follows the closing quote.
fn read_quoted(text: &str) -> Option<(String, &str)> {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Some((value, &text[index + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'r' => value.push('\r'),
                '"' => value.push('"'),
                '\\' => value.push('\\'),
                other => {
                    value.push('\\');
                    value.push(other);
                }
            },
            other => value.push(other),
        }
    }
    None
}

/// Guesses the level of a text line from a leading level word, also when a
/// timestamp or a bracketed module comes first.
fn guess_level(line: &str) -> Option<Level> {
    let mut rest = line.trim_start();
    for _ in 0..4 {
        if let Some(bracketed) = rest.strip_prefix('[') {
            let (inside, after) = bracketed.split_once(']')?;
            if let Some(level) = Level::from_word(inside.trim()) {
                return Some(level);
            }
            rest = after.trim_start();
        } else {
            let token = first_token(rest);
            if !is_timestamp(token) {
                break;
            }
            rest = rest[token.len()..].trim_start();
        }
    }
    level_word(first_token(rest))
}

fn first_token(text: &str) -> &str {
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    &text[..end]
}

/// A level word at the start of a text line counts when it is written in
/// capitals (`ERROR`) or followed by a colon (`error:`), so that a sentence
/// that merely starts with such a word is left alone.
fn level_word(token: &str) -> Option<Level> {
    let (word, marked) = match token.strip_suffix(':') {
        Some(word) => (word, true),
        None => (token, false),
    };
    let capitals = !word.is_empty() && word.bytes().all(|b| b.is_ascii_uppercase());
    if marked || capitals {
        Level::from_word(word)
    } else {
        None
    }
}

/// Whether a token looks like a date, a time of day, or both in one.
fn is_timestamp(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 8
        && bytes[0].is_ascii_digit()
        && bytes.iter().all(|b| {
            b.is_ascii_digit() || matches!(b, b'-' | b'/' | b':' | b'.' | b',' | b'T' | b'Z' | b'+')
        })
        && bytes
            .iter()
            .filter(|b| matches!(b, b'-' | b'/' | b':'))
            .count()
            >= 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn at(millis: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap() + TimeDelta::milliseconds(millis)
    }

    /// Parses one line on its own.
    fn one(choice: FormatChoice, line: &str) -> ParsedLine {
        let mut parser = LineParser::new(choice);
        assert_eq!(parser.push(at(0), line), None);
        parser.flush().expect("the line is kept back")
    }

    /// Parses lines that all carry the same time.
    fn all(choice: FormatChoice, lines: &[&str]) -> Vec<ParsedLine> {
        let mut parser = LineParser::new(choice);
        let mut parsed: Vec<ParsedLine> = lines
            .iter()
            .filter_map(|line| parser.push(at(0), line))
            .collect();
        parsed.extend(parser.flush());
        parsed
    }

    // ---- JSON ----------------------------------------------------------

    const TRACING_LINE: &str = r#"{"timestamp":"2026-11-18T14:02:11.482913Z","level":"WARN","message":"payment intent expired","tenant_id":"t_123","request_id":"8b1c","attempt":3,"target":"flowcpt_api::payments","span":{"method":"POST","name":"request"},"spans":[{"method":"POST","name":"request"}]}"#;

    #[test]
    fn a_tracing_subscriber_json_line_becomes_level_fields_and_message() {
        let parsed = one(FormatChoice::Auto, TRACING_LINE);
        assert_eq!(parsed.format, Format::Json);
        assert_eq!(parsed.level, Some(Level::Warn));
        assert_eq!(parsed.message, "payment intent expired");
        assert_eq!(parsed.raw, TRACING_LINE);
        assert!(!parsed.truncated);

        assert_eq!(parsed.fields["tenant_id"], json!("t_123"));
        assert_eq!(parsed.fields["request_id"], json!("8b1c"));
        assert_eq!(parsed.fields["attempt"], json!(3));
        assert_eq!(parsed.fields["target"], json!("flowcpt_api::payments"));
        assert_eq!(
            parsed.fields["timestamp"],
            json!("2026-11-18T14:02:11.482913Z")
        );
        // The level and the message have moved out of the fields.
        assert!(!parsed.fields.contains_key("level"));
        assert!(!parsed.fields.contains_key("message"));
        // Nested objects and arrays are kept as JSON text.
        assert_eq!(
            parsed.fields["span"],
            json!(r#"{"method":"POST","name":"request"}"#)
        );
        assert_eq!(
            parsed.fields["spans"],
            json!(r#"[{"method":"POST","name":"request"}]"#)
        );
    }

    #[test]
    fn json_knows_the_other_names_of_the_well_known_keys() {
        let parsed = one(
            FormatChoice::Auto,
            r#"{"severity":"error","msg":"boom","ts":1731938531.4,"caller":"main.go:12"}"#,
        );
        assert_eq!(parsed.level, Some(Level::Error));
        assert_eq!(parsed.message, "boom");
        assert_eq!(parsed.fields["timestamp"], json!(1731938531.4));
        assert_eq!(parsed.fields["caller"], json!("main.go:12"));
        assert!(!parsed.fields.contains_key("ts"));
        assert!(!parsed.fields.contains_key("severity"));
        assert!(!parsed.fields.contains_key("msg"));

        let parsed = one(
            FormatChoice::Auto,
            r#"{"lvl":"dbug","time":"14:02:11","message":"hi"}"#,
        );
        // `dbug` is no level this parser knows, so the key stays as it came.
        assert_eq!(parsed.level, None);
        assert_eq!(parsed.fields["lvl"], json!("dbug"));
        assert_eq!(parsed.fields["timestamp"], json!("14:02:11"));
    }

    #[test]
    fn the_first_name_of_a_well_known_key_wins() {
        let parsed = one(
            FormatChoice::Auto,
            r#"{"level":"info","severity":"error","message":"first","msg":"second","timestamp":"a","time":"b"}"#,
        );
        assert_eq!(parsed.level, Some(Level::Info));
        assert_eq!(parsed.message, "first");
        assert_eq!(parsed.fields["severity"], json!("error"));
        assert_eq!(parsed.fields["msg"], json!("second"));
        assert_eq!(parsed.fields["timestamp"], json!("a"));
        assert_eq!(parsed.fields["time"], json!("b"));
    }

    #[test]
    fn json_without_a_message_or_a_level() {
        let parsed = one(
            FormatChoice::Auto,
            r#"{"status":200,"ok":true,"error":null}"#,
        );
        assert_eq!(parsed.format, Format::Json);
        assert_eq!(parsed.level, None);
        assert_eq!(parsed.message, "");
        assert_eq!(parsed.fields["status"], json!(200));
        assert_eq!(parsed.fields["ok"], json!(true));
        assert_eq!(parsed.fields["error"], json!(null));
    }

    #[test]
    fn what_only_looks_like_json_is_text() {
        for line in [
            r#"{"level":"info","message":"cut off"#,
            "{not json at all}",
            r#"[{"level":"info"}]"#,
            r#""just a string""#,
            "{",
        ] {
            let parsed = one(FormatChoice::Auto, line);
            assert_eq!(parsed.format, Format::Text, "{line}");
            assert_eq!(parsed.message, line);
            assert!(parsed.fields.is_empty());
        }
    }

    // ---- logfmt --------------------------------------------------------

    #[test]
    fn a_logfmt_line_becomes_level_fields_and_message() {
        let line = r#"time=2026-11-18T14:02:11Z level=info msg="request done" status=200"#;
        let parsed = one(FormatChoice::Auto, line);
        assert_eq!(parsed.format, Format::Logfmt);
        assert_eq!(parsed.level, Some(Level::Info));
        assert_eq!(parsed.message, "request done");
        assert_eq!(parsed.fields["status"], json!("200"));
        assert_eq!(parsed.fields["timestamp"], json!("2026-11-18T14:02:11Z"));
        assert_eq!(parsed.fields.len(), 2);
        assert_eq!(parsed.raw, line);
    }

    #[test]
    fn a_logfmt_line_from_an_ingress_controller() {
        let line = r#"time="2026-11-18T14:02:11Z" level=warning msg="Error while dialing backend: dial tcp 10.2.0.17:8080: connect: connection refused" entryPointName=web routerName=flowcpt-api@kubernetes"#;
        let parsed = one(FormatChoice::Auto, line);
        assert_eq!(parsed.format, Format::Logfmt);
        assert_eq!(parsed.level, Some(Level::Warn));
        assert_eq!(
            parsed.message,
            "Error while dialing backend: dial tcp 10.2.0.17:8080: connect: connection refused"
        );
        assert_eq!(parsed.fields["entryPointName"], json!("web"));
        assert_eq!(parsed.fields["routerName"], json!("flowcpt-api@kubernetes"));
        assert_eq!(parsed.fields["timestamp"], json!("2026-11-18T14:02:11Z"));
    }

    #[test]
    fn logfmt_values() {
        let parsed = one(
            FormatChoice::Auto,
            r#"empty= quoted="a \"b\" c\n" path=/var/log/x.log dotted.key=1 expr=a=b"#,
        );
        assert_eq!(parsed.format, Format::Logfmt);
        assert_eq!(parsed.fields["empty"], json!(""));
        assert_eq!(parsed.fields["quoted"], json!("a \"b\" c\n"));
        assert_eq!(parsed.fields["path"], json!("/var/log/x.log"));
        assert_eq!(parsed.fields["dotted.key"], json!("1"));
        assert_eq!(parsed.fields["expr"], json!("a=b"));
    }

    #[test]
    fn one_bare_word_and_the_line_is_text() {
        for line in [
            "level=info msg=done oops",
            "starting level=info",
            "GET /healthz?probe=1 200",
            r#"msg="never closed"#,
            r#"msg="closed"tight=1"#,
            "=value",
            "http://host/path?a=b",
            "just words",
        ] {
            let parsed = one(FormatChoice::Auto, line);
            assert_eq!(parsed.format, Format::Text, "{line}");
            assert_eq!(parsed.message, line);
            assert!(parsed.fields.is_empty());
        }
    }

    // ---- text ----------------------------------------------------------

    #[test]
    fn a_text_line_keeps_the_raw_line_as_its_message() {
        let parsed = one(FormatChoice::Auto, "listening on 0.0.0.0:8080");
        assert_eq!(parsed.format, Format::Text);
        assert_eq!(parsed.level, None);
        assert_eq!(parsed.message, "listening on 0.0.0.0:8080");
        assert_eq!(parsed.raw, "listening on 0.0.0.0:8080");
        assert!(parsed.fields.is_empty());
    }

    #[test]
    fn an_empty_line_is_an_empty_text_line() {
        let parsed = one(FormatChoice::Auto, "");
        assert_eq!(parsed.format, Format::Text);
        assert_eq!(parsed.message, "");
    }

    #[test]
    fn the_level_of_a_text_line_is_guessed_from_a_leading_word() {
        for (line, level) in [
            ("ERROR connection refused", Some(Level::Error)),
            ("WARN  disk almost full", Some(Level::Warn)),
            ("INFO starting  version=0.14.2", Some(Level::Info)),
            ("DEBUG tick", Some(Level::Debug)),
            ("TRACE enter", Some(Level::Trace)),
            ("WARNING deprecated flag", Some(Level::Warn)),
            ("FATAL cannot bind", Some(Level::Error)),
            // After a timestamp.
            ("2026-11-18T14:02:11.482Z ERROR boom", Some(Level::Error)),
            ("2026-11-18 14:02:11,482 INFO ready", Some(Level::Info)),
            ("2026/11/18 14:02:11 WARN slow", Some(Level::Warn)),
            ("14:02:11.482 DEBUG tick", Some(Level::Debug)),
            // After a bracketed module, or both.
            ("[payments] ERROR declined", Some(Level::Error)),
            (
                "14:02:11.482 [main] WARN  c.f.Api - slow",
                Some(Level::Warn),
            ),
            (
                "[2026-11-18 14:02:11] [worker] INFO done",
                Some(Level::Info),
            ),
            // The level itself in brackets, or marked with a colon.
            ("[ERROR] declined", Some(Level::Error)),
            (
                "2026/11/18 14:02:11 [error] 31#31: upstream timed out",
                Some(Level::Error),
            ),
            ("error: could not compile `gossip`", Some(Level::Error)),
            ("Warning: using the default config", Some(Level::Warn)),
            // No level.
            ("listening on 0.0.0.0:8080", None),
            ("error handling was improved", None),
            ("Info for the reader", None),
            ("ERRORS were found", None),
            ("[main] started", None),
            ("2026-11-18T14:02:11Z started", None),
            ("[unclosed ERROR", None),
            ("", None),
        ] {
            assert_eq!(one(FormatChoice::Auto, line).level, level, "{line}");
        }
    }

    // ---- continuation --------------------------------------------------

    #[test]
    fn indented_lines_join_the_line_before_them() {
        let parsed = all(
            FormatChoice::Auto,
            &[
                "ERROR request failed",
                "    caused by a timeout",
                "\tafter 30 s",
                "INFO next request",
            ],
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0].message,
            "ERROR request failed\n    caused by a timeout\n\tafter 30 s"
        );
        assert_eq!(parsed[0].raw, parsed[0].message);
        assert_eq!(parsed[0].level, Some(Level::Error));
        assert_eq!(parsed[1].message, "INFO next request");
    }

    #[test]
    fn a_java_stack_trace_is_one_record() {
        let lines = [
            "2026-11-18 14:02:11,482 ERROR [http-1] c.f.Api - request failed",
            "java.lang.IllegalStateException: no tenant",
            "\tat com.flowcpt.Api.handle(Api.java:42)",
            "at com.flowcpt.Server.run(Server.java:17)",
            "Caused by: java.io.IOException: closed",
            "\tat com.flowcpt.Db.read(Db.java:7)",
            "\t... 12 more",
        ];
        let parsed = all(FormatChoice::Auto, &lines);
        // The exception line starts at the margin with none of the three
        // marks, so it begins a record of its own; the trace joins it.
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].message, lines[0]);
        assert_eq!(parsed[0].level, Some(Level::Error));
        assert_eq!(parsed[1].message, lines[1..].join("\n"));
        assert_eq!(parsed[1].raw, lines[1..].join("\n"));
    }

    #[test]
    fn a_rust_backtrace_joins_the_line_before_it() {
        let lines = [
            "thread 'main' panicked at src/main.rs:5:5:",
            "boom",
            "stack backtrace:",
            "   0: rust_begin_unwind",
            "             at /rustc/0ef1bc386/library/std/src/panicking.rs:697:5",
            "   1: core::panicking::panic_fmt",
            "             at /rustc/0ef1bc386/library/core/src/panicking.rs:75:14",
            "   2: flowcpt_api::main",
            "             at ./src/main.rs:5:5",
            "note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.",
        ];
        let parsed = all(FormatChoice::Auto, &lines);
        // The frames are continuation lines and join `stack backtrace:`.
        // The panic message, `stack backtrace:` and the note start at the
        // margin with none of the three marks, so by the continuation rule
        // each begins a record of its own.
        let messages: Vec<&str> = parsed.iter().map(|line| line.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                lines[0].to_string(),
                lines[1].to_string(),
                lines[2..9].join("\n"),
                lines[9].to_string(),
            ]
        );
        assert!(parsed.iter().all(|line| line.format == Format::Text));
    }

    #[test]
    fn a_continuation_keeps_the_level_and_fields_of_the_first_line() {
        let first = r#"{"level":"ERROR","message":"handler panicked","tenant_id":"t_123"}"#;
        let parsed = all(
            FormatChoice::Auto,
            &[first, "   0: rust_begin_unwind", "   1: core::panicking"],
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].format, Format::Json);
        assert_eq!(parsed[0].level, Some(Level::Error));
        assert_eq!(parsed[0].fields["tenant_id"], json!("t_123"));
        assert_eq!(
            parsed[0].message,
            "handler panicked\n   0: rust_begin_unwind\n   1: core::panicking"
        );
        assert_eq!(
            parsed[0].raw,
            format!("{first}\n   0: rust_begin_unwind\n   1: core::panicking")
        );
    }

    #[test]
    fn a_pretty_printed_json_object_joins_its_opening_brace() {
        let parsed = all(
            FormatChoice::Auto,
            &["{", r#"  "level": "info","#, r#"  "message": "hi""#, "}"],
        );
        let messages: Vec<&str> = parsed.iter().map(|line| line.message.as_str()).collect();
        assert_eq!(
            messages,
            ["{\n  \"level\": \"info\",\n  \"message\": \"hi\"", "}"]
        );
        assert!(parsed.iter().all(|line| line.format == Format::Text));
    }

    #[test]
    fn a_continuation_older_than_two_seconds_is_text_on_its_own() {
        let mut parser = LineParser::new(FormatChoice::Auto);
        assert_eq!(parser.push(at(0), "ERROR request failed"), None);
        // Within two seconds of the first line: joined.
        assert_eq!(parser.push(at(1_900), "    still the same thing"), None);
        assert_eq!(parser.push(at(2_000), "    and this"), None);
        // Later than that: the first record is complete.
        let first = parser.push(at(2_001), "    a late line").unwrap();
        assert_eq!(
            first.message,
            "ERROR request failed\n    still the same thing\n    and this"
        );
        assert_eq!(first.time, at(0));
        let late = parser.flush().unwrap();
        assert_eq!(late.message, "    a late line");
        assert_eq!(late.format, Format::Text);
        assert_eq!(late.time, at(2_001));
    }

    #[test]
    fn a_continuation_with_no_line_before_it_is_text_on_its_own() {
        let parsed = all(
            FormatChoice::Auto,
            &["    at the very start", "Caused by: nothing", "at last"],
        );
        // The first has nothing to join; the others then join it.
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].format, Format::Text);
        assert_eq!(
            parsed[0].message,
            "    at the very start\nCaused by: nothing\nat last"
        );

        let mut parser = LineParser::new(FormatChoice::Auto);
        parser.push(at(0), "INFO ready");
        assert_eq!(parser.flush().unwrap().message, "INFO ready");
        // The line before was handed out already, so this one stands alone.
        assert_eq!(parser.push(at(10), "    late"), None);
        assert_eq!(parser.flush().unwrap().message, "    late");
    }

    #[test]
    fn an_indented_structured_line_is_a_continuation_not_a_record() {
        let parsed = all(
            FormatChoice::Auto,
            &["INFO dump follows", r#"  {"level":"error"}"#, "  key=value"],
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].level, Some(Level::Info));
        assert!(parsed[0].fields.is_empty());
    }

    #[test]
    fn push_hands_out_the_line_before() {
        let mut parser = LineParser::new(FormatChoice::Auto);
        assert!(!parser.has_pending());
        assert_eq!(parser.push(at(0), "one"), None);
        assert!(parser.has_pending());
        assert_eq!(parser.push(at(1), "two").unwrap().message, "one");
        assert_eq!(parser.push(at(2), "three").unwrap().message, "two");
        assert_eq!(parser.flush().unwrap().message, "three");
        assert_eq!(parser.flush(), None);
        assert!(!parser.has_pending());
    }

    // ---- the format option ---------------------------------------------

    #[test]
    fn format_text_shows_lines_untouched() {
        let lines = [
            TRACING_LINE,
            r#"level=info msg="request done""#,
            "    indented",
            "at the margin",
            "Caused by: something",
        ];
        let parsed = all(FormatChoice::Text, &lines);
        assert_eq!(parsed.len(), lines.len());
        for (parsed, line) in parsed.iter().zip(lines) {
            assert_eq!(parsed.format, Format::Text);
            assert_eq!(parsed.message, line);
            assert_eq!(parsed.raw, line);
            assert!(parsed.fields.is_empty());
        }
    }

    #[test]
    fn format_json_runs_the_json_recogniser_only() {
        assert_eq!(one(FormatChoice::Json, TRACING_LINE).format, Format::Json);
        let parsed = one(FormatChoice::Json, "level=info msg=done");
        assert_eq!(parsed.format, Format::Text);
        assert_eq!(parsed.message, "level=info msg=done");
    }

    #[test]
    fn format_logfmt_runs_the_logfmt_recogniser_only() {
        let parsed = one(FormatChoice::Logfmt, "level=info msg=done");
        assert_eq!(parsed.format, Format::Logfmt);
        let parsed = one(FormatChoice::Logfmt, TRACING_LINE);
        assert_eq!(parsed.format, Format::Text);
        assert_eq!(parsed.message, TRACING_LINE);
    }

    #[test]
    fn the_continuation_rule_stays_on_for_forced_json_and_logfmt() {
        for choice in [FormatChoice::Json, FormatChoice::Logfmt] {
            let parsed = all(choice, &["ERROR boom", "    at somewhere"]);
            assert_eq!(parsed.len(), 1, "{choice}");
        }
    }

    #[test]
    fn format_names() {
        for choice in [
            FormatChoice::Auto,
            FormatChoice::Json,
            FormatChoice::Logfmt,
            FormatChoice::Text,
        ] {
            assert_eq!(choice.as_str().parse::<FormatChoice>(), Ok(choice));
        }
        assert!("yaml".parse::<FormatChoice>().is_err());
    }

    // ---- long lines ----------------------------------------------------

    #[test]
    fn a_line_longer_than_the_limit_is_cut_and_marked() {
        let long = "x".repeat(MAX_LINE_BYTES + 100);
        let parsed = one(FormatChoice::Auto, &long);
        assert!(parsed.truncated);
        assert_eq!(parsed.raw.len(), MAX_LINE_BYTES);
        assert_eq!(parsed.message.len(), MAX_LINE_BYTES);

        let exact = "x".repeat(MAX_LINE_BYTES);
        let parsed = one(FormatChoice::Auto, &exact);
        assert!(!parsed.truncated);
        assert_eq!(parsed.raw.len(), MAX_LINE_BYTES);
    }

    #[test]
    fn a_long_line_is_cut_on_a_character_boundary() {
        // `é` is two bytes; the limit falls in the middle of one.
        let long = format!("x{}", "é".repeat(MAX_LINE_BYTES / 2 + 10));
        let parsed = one(FormatChoice::Auto, &long);
        assert!(parsed.truncated);
        assert_eq!(parsed.raw.len(), MAX_LINE_BYTES - 1);
        assert!(parsed.raw.ends_with('é'));
    }

    #[test]
    fn a_json_line_that_was_cut_is_text() {
        let long = format!(r#"{{"message":"{}"}}"#, "x".repeat(MAX_LINE_BYTES));
        let parsed = one(FormatChoice::Auto, &long);
        assert!(parsed.truncated);
        assert_eq!(parsed.format, Format::Text);
    }

    #[test]
    fn continuation_lines_do_not_grow_a_record_beyond_the_limit() {
        let mut parser = LineParser::new(FormatChoice::Auto);
        let chunk = format!(" {}", "x".repeat(MAX_LINE_BYTES / 2));
        assert_eq!(parser.push(at(0), "ERROR boom"), None);
        assert_eq!(parser.push(at(1), &chunk), None);
        // The next one would not fit, so the record is complete.
        let first = parser.push(at(2), &chunk).unwrap();
        assert!(first.raw.len() <= MAX_LINE_BYTES);
        assert!(first.message.starts_with("ERROR boom\n "));
        let second = parser.flush().unwrap();
        assert_eq!(second.raw, chunk);
        assert_eq!(second.format, Format::Text);
    }
}
