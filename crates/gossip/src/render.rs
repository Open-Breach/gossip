//! Rendering of records: one line of text per record, with a prefix per
//! source, or the record as JSON, or the raw line.

use std::collections::BTreeMap;

use anstyle::{AnsiColor, Style};
use chrono::{DateTime, Local, Utc};
use gossip_core::record::{event_field, value_text};
use gossip_core::resolver::EXITED;
use gossip_core::{Columns, Cut, Kind, Level, Record, Value};

use crate::columns::{cells, fit, keep_ends};
use crate::fields::FieldChoice;

/// The colours a pod prefix can take. Red and yellow are left to the levels.
const POD_COLOURS: [AnsiColor; 10] = [
    AnsiColor::Cyan,
    AnsiColor::Green,
    AnsiColor::Magenta,
    AnsiColor::Blue,
    AnsiColor::BrightCyan,
    AnsiColor::BrightGreen,
    AnsiColor::BrightMagenta,
    AnsiColor::BrightBlue,
    AnsiColor::White,
    AnsiColor::BrightWhite,
];

/// The alphabet Kubernetes makes the hash of a replica set and the suffix
/// of a pod name from.
const NAME_ALPHABET: &str = "bcdfghjklmnpqrstvwxz2456789";

/// In which time zone the time of a record is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Local,
    #[cfg_attr(not(test), allow(dead_code))]
    Utc,
}

/// How the text output looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextOptions {
    pub colour: bool,
    /// Which fields of a line are shown, and in which order.
    pub fields: FieldChoice,
    /// Mark events with an emoji per reason; with `EVNT` otherwise.
    pub emoji: bool,
    pub zone: Zone,
    /// The widths of the columns of the prefix, and where a value that is
    /// too long for its column is cut.
    pub columns: Columns,
}

/// Renders records as text, one line per record:
///
/// ```text
/// 14:02:11.482 fc-api…vxfgs         api         WARN  tenant_id=t_123 request_id=8b1c  payment intent expired
/// ```
///
/// The time, the pod, the container and the level each stand in a column
/// of a fixed width, so the message starts at the same place on every line.
/// The pod column always gets the full name of the pod; a name that is too
/// long for it is cut by the rule of the column.
#[derive(Debug)]
pub struct TextRenderer {
    options: TextOptions,
}

impl TextRenderer {
    pub fn new(options: TextOptions) -> Self {
        TextRenderer { options }
    }

    /// Which fields of a line are shown from here on.
    pub fn set_fields(&mut self, fields: FieldChoice) {
        self.options.fields = fields;
    }

    /// Which fields of a line are shown.
    pub fn fields_shown(&self) -> &FieldChoice {
        &self.options.fields
    }

    /// The columns of the prefix from here on.
    pub fn set_columns(&mut self, columns: Columns) {
        self.options.columns = columns;
    }

    /// The columns of the prefix.
    pub fn columns(&self) -> Columns {
        self.options.columns
    }

    /// The text of a record, without a line ending. A record whose message
    /// holds several lines gives several lines.
    pub fn render(&self, record: &Record) -> String {
        let columns = self.options.columns;
        let time = fit(&self.time(record.time), columns.time);
        if record.kind == Kind::Notice {
            let quiet = Style::new().dimmed();
            let line = join(&[
                ("", time.as_deref()),
                (" ", Some(&format!("-- {}", record.message))),
            ]);
            return self.paint(quiet, &line);
        }

        if record.kind == Kind::Event {
            return self.render_event(record, time.as_deref());
        }

        let pod = self.pod_cell(&record.pod);
        let container = fit(record.container.as_deref().unwrap_or(""), columns.container);
        // The pod and the container are one run of the pod's colour.
        let source = join(&[("", pod.as_deref()), (" ", container.as_deref())]);
        let source = (!source.is_empty()).then_some(source);
        let level = match record.level {
            Some(level) => fit(&level.as_str().to_uppercase(), columns.level),
            None => fit("", columns.level),
        };
        let fields = self.fields(&record.fields);
        let cut = if record.truncated {
            " [line cut at 1 MiB]"
        } else {
            ""
        };

        if record.previous {
            // A line of the run before the current one: all of it dimmed.
            let fields: Vec<String> = fields
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            let body = join_body(&fields.join(" "), &record.message);
            let line = join(&[
                ("", time.as_deref()),
                (" ", source.as_deref()),
                ("  ", level.as_deref()),
                (" ", Some(&format!("{body}{cut}"))),
            ]);
            return self.paint(Style::new().dimmed(), line.trim_end());
        }

        let pod_style = Style::new().fg_color(Some(pod_colour(&record.pod).into()));
        let dim = Style::new().dimmed();
        let fields: Vec<String> = fields
            .iter()
            .map(|(key, value)| format!("{}{value}", self.paint(dim, &format!("{key}="))))
            .collect();
        let body = join_body(&fields.join(" "), &record.message);
        let time = time.map(|time| self.paint(dim, &time));
        let source = source.map(|source| self.paint(pod_style, &source));
        let level = level.map(|level| self.paint(level_style(record.level), &level));
        let line = join(&[
            ("", time.as_deref()),
            (" ", source.as_deref()),
            ("  ", level.as_deref()),
            (" ", Some(&format!("{body}{}", self.paint(dim, cut)))),
        ]);
        line.trim_end().to_string()
    }

    /// An event: a mark in place of the container, then reason and message.
    ///
    /// ```text
    /// 14:02:11.490 fc-api…vxfgs         💀               Killing: Container api failed liveness probe
    /// ```
    fn render_event(&self, record: &Record, time: Option<&str>) -> String {
        let field = |key: &str| record.fields.get(key).map(value_text).unwrap_or_default();
        let reason = field(event_field::REASON);
        let warning = record.level == Some(Level::Warn);

        // An event about a workload has no pod; it is shown under what it
        // is about, as in `deployment/flowcpt-api`.
        let subject = if record.pod.is_empty() {
            let regarding = field(event_field::REGARDING);
            match regarding.split_once('/') {
                Some((kind, name)) => format!("{}/{name}", kind.to_lowercase()),
                None => regarding.into_owned(),
            }
        } else {
            record.pod.clone()
        };
        let subject = self.pod_cell(&subject);

        // The mark stands in the column of the container. An emoji is two
        // cells wide in most terminals, and is measured as that.
        let columns = self.options.columns;
        let mark = if self.options.emoji {
            event_emoji(&reason, warning)
        } else {
            EVENT_TAG
        };
        let mark = fit(mark, columns.container);
        let level = fit("", columns.level);
        let line = join(&[
            ("", time),
            (" ", subject.as_deref()),
            (" ", mark.as_deref()),
            ("  ", level.as_deref()),
            (" ", Some(&format!("{reason}: {}", record.message))),
        ]);
        // Normal events are dimmed; warnings take the warning colour.
        let style = if warning {
            level_style(Some(Level::Warn))
        } else {
            Style::new().dimmed()
        };
        self.paint(style, &line)
    }

    fn time(&self, time: DateTime<Utc>) -> String {
        const FORMAT: &str = "%H:%M:%S%.3f";
        match self.options.zone {
            Zone::Local => time.with_timezone(&Local).format(FORMAT).to_string(),
            Zone::Utc => time.format(FORMAT).to_string(),
        }
    }

    /// A pod name, or what stands in its place, as it stands in the column
    /// of the pod: always the full name, cut when it is too long. Cut in
    /// the middle, the name of a deployment's pod keeps the workload and the
    /// pod suffix and loses the hash of the replica set between them:
    /// `fc-api…vxfgs`. The start of the name says which workload it is, the
    /// end which pod; the hash in the middle is the part nobody needs.
    fn pod_cell(&self, name: &str) -> Option<String> {
        let column = self.options.columns.pod;
        let width = usize::from(column.width);
        if column.cut == Cut::Middle
            && cells(name) > width
            && let Some(suffix) = deployment_pod_suffix(name)
            && let Some((_, pod_suffix)) = suffix.rsplit_once('-')
        {
            let workload = name.len() - suffix.len() - 1;
            // The ellipsis and the pod suffix, and at least one cell of the
            // workload before them.
            let room = width.saturating_sub(1 + pod_suffix.len());
            if room > 0 {
                let head = room.min(workload);
                return Some(keep_ends(name, head, pod_suffix.len(), width));
            }
        }
        fit(name, column)
    }

    /// The fields to show, as key and value, in the order to show them in.
    fn fields(&self, fields: &BTreeMap<String, Value>) -> Vec<(String, String)> {
        self.options
            .fields
            .of(fields)
            .into_iter()
            .map(|(key, value)| (key.clone(), field_text(value)))
            .collect()
    }

    fn paint(&self, style: Style, text: &str) -> String {
        if self.options.colour && !text.is_empty() {
            format!("{}{text}{}", style.render(), style.render_reset())
        } else {
            text.to_string()
        }
    }
}

/// What marks an event where emoji are not wanted or would not show.
const EVENT_TAG: &str = "EVNT";

/// The parts of a line that are there, each after its separator. The first
/// one stands at the margin, without its separator; a part that is not
/// there, the cell of a hidden column, takes its separator with it.
fn join(parts: &[(&str, Option<&str>)]) -> String {
    let mut line = String::new();
    for (separator, part) in parts {
        let Some(part) = part else {
            continue;
        };
        if !line.is_empty() {
            line.push_str(separator);
        }
        line.push_str(part);
    }
    line
}

/// The emoji of an event, one per reason, so that the eye reads the life of
/// a pod without reading words.
fn event_emoji(reason: &str, warning: bool) -> &'static str {
    match reason {
        "Scheduled" => "📍",
        "Pulling" | "Pulled" => "📦",
        "Started" => "🚀",
        // `Exited` is the end of a run with an exit code other than zero.
        "Killing" => "💀",
        reason if reason == EXITED => "💀",
        // A crash loop and an image pull back-off.
        "BackOff" => "🔥",
        "Evicted" | "Preempted" => "🧹",
        // `Unhealthy`, and every other warning.
        _ if warning => "⚠️",
        _ => "ℹ️",
    }
}

/// The fields, two spaces, the message; or whichever of the two there is.
fn join_body(fields: &str, message: &str) -> String {
    match (fields.is_empty(), message.is_empty()) {
        (true, _) => message.to_string(),
        (false, true) => fields.to_string(),
        (false, false) => format!("{fields}  {message}"),
    }
}

/// The value of a field as text; in quotes when it would not read as one
/// word otherwise.
fn field_text(value: &Value) -> String {
    let text = value_text(value);
    if text.is_empty() || text.contains(|c: char| c.is_whitespace() || c == '"' || c == '=') {
        format!("{text:?}")
    } else {
        text.into_owned()
    }
}

fn level_style(level: Option<Level>) -> Style {
    let colour = |colour: AnsiColor| Style::new().fg_color(Some(colour.into()));
    match level {
        Some(Level::Error) => colour(AnsiColor::Red).bold(),
        Some(Level::Warn) => colour(AnsiColor::Yellow),
        Some(Level::Info) => colour(AnsiColor::Green),
        Some(Level::Debug) => colour(AnsiColor::Blue),
        Some(Level::Trace) => Style::new().dimmed(),
        None => Style::new(),
    }
}

/// The colour of a pod: always the same for the same name, so that it stays
/// what it is for the life of the pod.
fn pod_colour(pod: &str) -> AnsiColor {
    // FNV-1a: small, and stable across runs and platforms.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in pod.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    POD_COLOURS[(hash % POD_COLOURS.len() as u64) as usize]
}

/// The replica-set hash and pod suffix of a pod name that is shaped like the
/// name of a deployment's pod, `<deployment>-<hash>-<suffix>`.
///
/// The hash is four to ten characters of the alphabet Kubernetes generates
/// names from, with a letter among them, which keeps out the pods of daemon
/// sets (`node-exporter-x7k2p`) and of cron jobs (`sync-29391840-x7k2p`).
/// The suffix is five lowercase letters or digits.
fn deployment_pod_suffix(pod: &str) -> Option<&str> {
    let (rest, suffix) = pod.rsplit_once('-')?;
    let (deployment, hash) = rest.rsplit_once('-')?;
    let suffix_fits = suffix.len() == 5
        && suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let hash_fits = (4..=10).contains(&hash.len())
        && hash.chars().all(|c| NAME_ALPHABET.contains(c))
        && hash.chars().any(|c| c.is_ascii_alphabetic());
    (!deployment.is_empty() && suffix_fits && hash_fits).then(|| &pod[deployment.len() + 1..])
}

/// A record as one line of JSON, with every field of the record.
pub fn json_line(record: &Record) -> serde_json::Result<String> {
    serde_json::to_string(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use gossip_core::Format;
    use serde_json::json;

    fn plain() -> TextOptions {
        TextOptions {
            colour: false,
            fields: FieldChoice::Default,
            emoji: true,
            zone: Zone::Utc,
            columns: Columns::default(),
        }
    }

    /// The plain options with these columns over the built-in ones.
    fn with_columns(columns: &str) -> TextOptions {
        TextOptions {
            columns: columns
                .parse::<gossip_core::ColumnsSetting>()
                .unwrap()
                .resolve(),
            ..plain()
        }
    }

    fn event(pod: &str, regarding: &str, reason: &str, warning: bool, message: &str) -> Record {
        Record {
            cluster: "aks-vulcan-01".into(),
            namespace: "flowcpt".into(),
            pod: pod.into(),
            container: None,
            time: Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap()
                + chrono::TimeDelta::milliseconds(490),
            kind: Kind::Event,
            format: Format::Text,
            level: Some(if warning { Level::Warn } else { Level::Info }),
            message: message.into(),
            fields: BTreeMap::from([
                ("reason".to_string(), json!(reason)),
                (
                    "type".to_string(),
                    json!(if warning { "Warning" } else { "Normal" }),
                ),
                ("count".to_string(), json!(1)),
                ("regarding".to_string(), json!(regarding)),
            ]),
            raw: format!("{reason}: {message}"),
            previous: false,
            truncated: false,
        }
    }

    #[test]
    fn an_event_carries_an_emoji_in_place_of_the_container() {
        let renderer = TextRenderer::new(plain());
        // The four lines of the example of the spec, section 5.3.
        let warn = with_fields(
            line(POD, "api", Some(Level::Warn), "payment intent expired"),
            &[("tenant_id", json!("t_123"))],
        );
        let killing = event(
            POD,
            &format!("Pod/{POD}"),
            "Killing",
            false,
            "Container api failed liveness probe, will be restarted",
        );
        let started = event(
            POD,
            &format!("Pod/{POD}"),
            "Started",
            false,
            "Started container api",
        );
        let info = line(
            POD,
            "api",
            Some(Level::Info),
            "starting  version=0.14.2 environment=staging",
        );
        let lines = [
            renderer.render(&warn),
            renderer.render(&killing),
            renderer.render(&started),
            renderer.render(&info),
        ];
        assert_eq!(
            lines,
            [
                "14:02:11.482 flowcpt-api…xk2p1    api         WARN  tenant_id=t_123  payment intent expired",
                "14:02:11.490 flowcpt-api…xk2p1    💀                Killing: Container api failed liveness probe, will be restarted",
                "14:02:11.490 flowcpt-api…xk2p1    🚀                Started: Started container api",
                "14:02:11.482 flowcpt-api…xk2p1    api         INFO  starting  version=0.14.2 environment=staging",
            ]
        );
    }

    #[test]
    fn without_emoji_an_event_is_marked_evnt_in_the_same_column() {
        let renderer = TextRenderer::new(TextOptions {
            emoji: false,
            ..plain()
        });
        let killing = event(
            POD,
            &format!("Pod/{POD}"),
            "Killing",
            false,
            "Stopping container api",
        );
        assert_eq!(
            renderer.render(&killing),
            "14:02:11.490 flowcpt-api…xk2p1    EVNT              Killing: Stopping container api"
        );
        // The mark stands in the column of the container, so the lines stay
        // aligned.
        assert_eq!(
            renderer.render(&line(POD, "api", Some(Level::Info), "ready")),
            "14:02:11.482 flowcpt-api…xk2p1    api         INFO  ready"
        );
    }

    #[test]
    fn the_emoji_of_every_reason() {
        for (reason, warning, emoji) in [
            ("Scheduled", false, "📍"),
            ("Pulling", false, "📦"),
            ("Pulled", false, "📦"),
            ("Started", false, "🚀"),
            ("Killing", false, "💀"),
            ("Exited", true, "💀"),
            ("BackOff", true, "🔥"),
            ("Unhealthy", true, "⚠️"),
            ("FailedScheduling", true, "⚠️"),
            ("Evicted", true, "🧹"),
            ("Preempted", false, "🧹"),
            ("Created", false, "ℹ️"),
            ("ScalingReplicaSet", false, "ℹ️"),
        ] {
            assert_eq!(event_emoji(reason, warning), emoji, "{reason}");
        }
    }

    #[test]
    fn an_event_about_a_workload_is_shown_under_what_it_is_about() {
        let scaled = event(
            "",
            "Deployment/flowcpt-api",
            "ScalingReplicaSet",
            false,
            "Scaled up replica set flowcpt-api-7d9f to 2",
        );
        assert_eq!(
            TextRenderer::new(plain()).render(&scaled),
            "14:02:11.490 deployment…owcpt-api ℹ️                ScalingReplicaSet: Scaled up replica set flowcpt-api-7d9f to 2"
        );
        // An event about a pod stands under the name of the pod, cut as it
        // is on a line of that pod.
        let started = event(FC_POD, "Pod/x", "Started", false, "Started container api");
        assert_eq!(
            TextRenderer::new(plain()).render(&started),
            "14:02:11.490 fc-api…vxfgs         🚀                Started: Started container api"
        );
    }

    #[test]
    fn normal_events_are_dimmed_and_warnings_take_the_warning_colour() {
        let coloured = TextOptions {
            colour: true,
            ..plain()
        };
        let started = event(
            POD,
            &format!("Pod/{POD}"),
            "Started",
            false,
            "Started container api",
        );
        let dim = Style::new().dimmed();
        let text = TextRenderer::new(coloured.clone()).render(&started);
        assert!(text.starts_with(&dim.render().to_string()), "{text:?}");
        assert!(text.ends_with(&dim.render_reset().to_string()), "{text:?}");

        let back_off = event(
            POD,
            &format!("Pod/{POD}"),
            "BackOff",
            true,
            "Back-off restarting failed container",
        );
        let yellow = Style::new().fg_color(Some(AnsiColor::Yellow.into()));
        let text = TextRenderer::new(coloured).render(&back_off);
        assert!(text.starts_with(&yellow.render().to_string()), "{text:?}");
        assert!(text.contains("🔥"), "{text:?}");
    }

    fn line(pod: &str, container: &str, level: Option<Level>, message: &str) -> Record {
        Record {
            cluster: "aks-vulcan-01".into(),
            namespace: "flowcpt".into(),
            pod: pod.into(),
            container: Some(container.into()),
            time: Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap()
                + chrono::TimeDelta::milliseconds(482),
            kind: Kind::Line,
            format: Format::Text,
            level,
            message: message.into(),
            fields: BTreeMap::new(),
            raw: message.into(),
            previous: false,
            truncated: false,
        }
    }

    fn with_fields(mut record: Record, fields: &[(&str, Value)]) -> Record {
        record.format = Format::Json;
        record.fields = fields
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect();
        record
    }

    const POD: &str = "flowcpt-api-7d9f-xk2p1";

    #[test]
    fn a_json_line_is_level_fields_and_message() {
        let record = with_fields(
            line(POD, "api", Some(Level::Warn), "payment intent expired"),
            &[("tenant_id", json!("t_123")), ("request_id", json!("8b1c"))],
        );
        // The example of the spec, section 5.3.
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         WARN  tenant_id=t_123 request_id=8b1c  payment intent expired"
        );
    }

    #[test]
    fn a_text_line_is_level_and_message() {
        let record = line(
            POD,
            "api",
            Some(Level::Info),
            "starting  version=0.14.2 environment=staging",
        );
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         INFO  starting  version=0.14.2 environment=staging"
        );
    }

    #[test]
    fn a_line_without_a_level_keeps_the_column() {
        let record = line(POD, "api", None, "listening on 0.0.0.0:8080");
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api               listening on 0.0.0.0:8080"
        );
        let error = line(POD, "api", Some(Level::Error), "boom");
        assert_eq!(
            TextRenderer::new(plain()).render(&error),
            "14:02:11.482 flowcpt-api…xk2p1    api         ERROR boom"
        );
    }

    fn named(names: &[&str]) -> FieldChoice {
        FieldChoice::Named(names.iter().map(|name| name.to_string()).collect())
    }

    #[test]
    fn fields_are_shown_in_the_default_order_unless_asked_for_by_name() {
        let record = with_fields(
            line(POD, "api", Some(Level::Info), "done"),
            &[
                ("module", json!("flowcpt_api::payments")),
                ("status", json!(200)),
                ("ok", json!(true)),
                ("note", json!("two words")),
                ("empty", json!("")),
                ("request_id", json!("8b1c")),
                ("tenant_id", json!("t_123")),
                ("http.method", json!("GET")),
            ],
        );
        // What a request is read by first, then the rest by name.
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         INFO  http.method=GET tenant_id=t_123 request_id=8b1c empty=\"\" module=flowcpt_api::payments note=\"two words\" ok=true status=200  done"
        );

        let chosen = TextOptions {
            fields: named(&["status", "missing", "ok", "tenant_id"]),
            ..plain()
        };
        assert_eq!(
            TextRenderer::new(chosen).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         INFO  status=200 ok=true tenant_id=t_123  done"
        );

        let none = TextOptions {
            fields: named(&[]),
            ..plain()
        };
        assert_eq!(
            TextRenderer::new(none).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         INFO  done"
        );
    }

    #[test]
    fn a_hidden_field_is_shown_only_when_it_is_asked_for() {
        let record = with_fields(
            line(POD, "api", Some(Level::Info), "done"),
            &[
                ("timestamp", json!("2026-11-18T14:02:11.482Z")),
                ("target", json!("flowcpt_api::payments")),
                ("name", json!("request")),
                ("status", json!(200)),
            ],
        );
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         INFO  status=200  done"
        );

        // By name.
        let by_name = TextOptions {
            fields: named(&["target", "status", "timestamp"]),
            ..plain()
        };
        assert_eq!(
            TextRenderer::new(by_name).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         INFO  target=flowcpt_api::payments status=200 timestamp=2026-11-18T14:02:11.482Z  done"
        );

        // Or all of them at once, in the default order.
        let all = TextOptions {
            fields: FieldChoice::All,
            ..plain()
        };
        assert_eq!(
            TextRenderer::new(all).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         INFO  name=request status=200 target=flowcpt_api::payments timestamp=2026-11-18T14:02:11.482Z  done"
        );

        // A line with nothing but hidden fields is its message only.
        let record = with_fields(
            line(POD, "api", Some(Level::Info), "done"),
            &[
                ("timestamp", json!("2026-11-18T14:02:11.482Z")),
                ("target", json!("flowcpt_api::payments")),
            ],
        );
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         INFO  done"
        );
    }

    #[test]
    fn a_line_with_fields_and_no_message_shows_the_fields() {
        let record = with_fields(line(POD, "api", None, ""), &[("status", json!(200))]);
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api               status=200"
        );
    }

    #[test]
    fn the_columns_have_fixed_widths_whatever_the_names() {
        let renderer = TextRenderer::new(plain());
        let lines = [
            renderer.render(&line("redis-0", "redis", Some(Level::Info), "ready")),
            renderer.render(&line(POD, "api", Some(Level::Error), "ready")),
            renderer.render(&line("redis-0", "linkerd-proxy", None, "ready")),
        ];
        assert_eq!(
            lines,
            [
                "14:02:11.482 redis-0              redis       INFO  ready",
                "14:02:11.482 flowcpt-api…xk2p1    api         ERROR ready",
                "14:02:11.482 redis-0              linkerd-p…        ready",
            ]
        );
        // Twelve cells of time, twenty of pod, ten of container, five of
        // level: the message starts in the same cell on every line.
        for line in &lines {
            assert_eq!(cells(line.strip_suffix("ready").unwrap()), 52, "{line}");
        }
    }

    /// The pod of acceptance point 6.
    const FC_POD: &str = "fc-api-7f698fb95f-vxfgs";

    #[test]
    fn a_pod_name_cut_in_the_middle_keeps_the_workload_and_the_pod_suffix() {
        let record = line(FC_POD, "api", Some(Level::Info), "ready");
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 fc-api…vxfgs         api         INFO  ready"
        );
        // A column that holds the name shows it whole.
        assert_eq!(
            TextRenderer::new(with_columns("pod=40")).render(&record),
            "14:02:11.482 fc-api-7f698fb95f-vxfgs                  api         INFO  ready"
        );
        assert_eq!(
            TextRenderer::new(with_columns("pod=23")).render(&record),
            "14:02:11.482 fc-api-7f698fb95f-vxfgs api         INFO  ready"
        );
        // A workload too long for the column gives way; the pod suffix stays.
        let web = line(
            "flowcpt-web-api-5c8b6d7f9-m4q8z",
            "api",
            Some(Level::Info),
            "ready",
        );
        assert_eq!(
            TextRenderer::new(plain()).render(&web),
            "14:02:11.482 flowcpt-web-ap…m4q8z api         INFO  ready"
        );
        assert_eq!(
            TextRenderer::new(with_columns("pod=8")).render(&web),
            "14:02:11.482 fl…m4q8z api         INFO  ready"
        );
        // No room for more than the ellipsis and the suffix: cut like any
        // other text.
        assert_eq!(
            TextRenderer::new(with_columns("pod=6")).render(&web),
            "14:02:11.482 flo…8z api         INFO  ready"
        );
    }

    #[test]
    fn a_name_that_is_not_a_deployments_is_cut_like_any_other_text() {
        let job = line(
            "nightly-sync-29391840-x7k2p",
            "sync",
            Some(Level::Info),
            "ready",
        );
        assert_eq!(
            TextRenderer::new(plain()).render(&job),
            "14:02:11.482 nightly-sy…840-x7k2p sync        INFO  ready"
        );
    }

    #[test]
    fn the_pod_column_can_be_cut_on_either_side_or_not_at_all() {
        let record = line(FC_POD, "api", Some(Level::Info), "ready");
        let render = |columns: &str| TextRenderer::new(with_columns(columns)).render(&record);
        assert_eq!(
            render("pod=10:start"),
            "14:02:11.482 …95f-vxfgs api         INFO  ready"
        );
        assert_eq!(
            render("pod=13:end"),
            "14:02:11.482 fc-api-7f698… api         INFO  ready"
        );
        assert_eq!(
            render("pod=12:middle"),
            "14:02:11.482 fc-api…vxfgs api         INFO  ready"
        );
        assert_eq!(
            render("pod=10:none"),
            "14:02:11.482 fc-api-7f698fb95f-vxfgs api         INFO  ready"
        );
    }

    #[test]
    fn the_pod_column_always_gets_the_full_name() {
        // Every pod here is a deployment's, and none of the names is
        // shortened to its hash and suffix: each is the full name, cut.
        let renderer = TextRenderer::new(plain());
        let lines = [
            renderer.render(&line(FC_POD, "api", Some(Level::Info), "one")),
            renderer.render(&line(POD, "api", Some(Level::Info), "two")),
            renderer.render(&line(
                "fc-app-6d5c7b8f4-q7w2x",
                "app",
                Some(Level::Info),
                "three",
            )),
        ];
        assert_eq!(
            lines,
            [
                "14:02:11.482 fc-api…vxfgs         api         INFO  one",
                "14:02:11.482 flowcpt-api…xk2p1    api         INFO  two",
                "14:02:11.482 fc-app…q7w2x         app         INFO  three",
            ]
        );
        // A name that fits its column is whole, hash and all.
        let fits = line("api-7d9f-xk2p1", "api", Some(Level::Info), "four");
        assert_eq!(
            renderer.render(&fits),
            "14:02:11.482 api-7d9f-xk2p1       api         INFO  four"
        );
        // Which pods came before changes nothing: the same pod is the same
        // text on every line.
        let other = line("redis-0", "redis", Some(Level::Info), "five");
        assert_eq!(
            renderer.render(&other),
            "14:02:11.482 redis-0              redis       INFO  five"
        );
        assert_eq!(
            renderer.render(&line(FC_POD, "api", Some(Level::Info), "one")),
            lines[0]
        );
    }

    #[test]
    fn a_width_of_zero_hides_a_column_and_the_message_moves_left() {
        let record = line(FC_POD, "api", Some(Level::Warn), "ready");
        let render = |columns: &str| TextRenderer::new(with_columns(columns)).render(&record);
        assert_eq!(render("pod=0"), "14:02:11.482 api         WARN  ready");
        assert_eq!(
            render("container=0"),
            "14:02:11.482 fc-api…vxfgs          WARN  ready"
        );
        assert_eq!(
            render("level=0"),
            "14:02:11.482 fc-api…vxfgs         api        ready"
        );
        assert_eq!(
            render("time=0"),
            "fc-api…vxfgs         api         WARN  ready"
        );
        assert_eq!(render("pod=0,container=0"), "14:02:11.482  WARN  ready");
        assert_eq!(render("time=0,pod=0,container=0,level=0"), "ready");

        // An event and a notice lose the same columns.
        let killing = event(FC_POD, "Pod/x", "Killing", false, "Stopping container api");
        let render = |columns: &str| TextRenderer::new(with_columns(columns)).render(&killing);
        assert_eq!(
            render("pod=0"),
            "14:02:11.490 💀                Killing: Stopping container api"
        );
        assert_eq!(
            render("container=0,level=0"),
            "14:02:11.490 fc-api…vxfgs         Killing: Stopping container api"
        );
        assert_eq!(
            render("time=0,pod=0,container=0,level=0"),
            "Killing: Stopping container api"
        );
        let time = Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap();
        let notice = Record::notice("c", "flowcpt", "", None, time, "reconnecting");
        assert_eq!(
            TextRenderer::new(with_columns("time=0")).render(&notice),
            "-- reconnecting"
        );
    }

    #[test]
    fn the_container_and_the_level_are_cut_at_the_end() {
        let record = line("redis-0", "linkerd-proxy", Some(Level::Error), "ready");
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 redis-0              linkerd-p…  ERROR ready"
        );
        assert_eq!(
            TextRenderer::new(with_columns("container=13,level=3")).render(&record),
            "14:02:11.482 redis-0              linkerd-proxy  ER… ready"
        );
        assert_eq!(
            TextRenderer::new(with_columns("container=6:start,level=8")).render(&record),
            "14:02:11.482 redis-0              …proxy  ERROR    ready"
        );
    }

    #[test]
    fn the_mark_of_an_event_stands_in_the_column_of_the_container() {
        let back_off = event(POD, "Pod/x", "BackOff", true, "Back-off");
        let render = |options: TextOptions| TextRenderer::new(options).render(&back_off);
        // An emoji takes two cells of the ten; `EVNT` takes four.
        assert_eq!(
            render(plain()),
            "14:02:11.490 flowcpt-api…xk2p1    🔥                BackOff: Back-off"
        );
        let tag = |columns: &str| TextOptions {
            emoji: false,
            ..with_columns(columns)
        };
        assert_eq!(
            render(tag("container=10")),
            "14:02:11.490 flowcpt-api…xk2p1    EVNT              BackOff: Back-off"
        );
        // In the same cells, whichever of the two it is.
        assert_eq!(cells(&render(plain())), cells(&render(tag("container=10"))));
        // A column too narrow for the mark cuts it like any other value.
        assert_eq!(
            render(tag("container=3")),
            "14:02:11.490 flowcpt-api…xk2p1    EV…        BackOff: Back-off"
        );
        assert_eq!(
            render(with_columns("container=2")),
            "14:02:11.490 flowcpt-api…xk2p1    🔥        BackOff: Back-off"
        );
        assert_eq!(
            render(with_columns("container=1")),
            "14:02:11.490 flowcpt-api…xk2p1    …        BackOff: Back-off"
        );
    }

    #[test]
    fn every_mark_is_measured_as_wide_as_a_terminal_shows_it() {
        for reason in [
            "Scheduled",
            "Pulling",
            "Started",
            "Killing",
            "Exited",
            "BackOff",
            "Evicted",
            "Unhealthy",
            "Created",
        ] {
            for warning in [false, true] {
                let emoji = event_emoji(reason, warning);
                assert_eq!(cells(emoji), 2, "{reason}: {emoji}");
            }
        }
        assert_eq!(cells(EVENT_TAG), 4);
    }

    #[test]
    fn a_wider_time_column_pads_the_time() {
        let record = line("redis-0", "api", Some(Level::Info), "ready");
        assert_eq!(
            TextRenderer::new(with_columns("time=14")).render(&record),
            "14:02:11.482   redis-0              api         INFO  ready"
        );
        // It is not cut unless a side is named.
        assert_eq!(
            TextRenderer::new(with_columns("time=8")).render(&record),
            "14:02:11.482 redis-0              api         INFO  ready"
        );
        assert_eq!(
            TextRenderer::new(with_columns("time=9:end")).render(&record),
            "14:02:11… redis-0              api         INFO  ready"
        );
    }

    #[test]
    fn a_notice_is_set_off_with_two_dashes() {
        let time = Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap();
        let notice = Record::notice(
            "c",
            "flowcpt",
            POD,
            None,
            time,
            format!("pod {POD} went away"),
        );
        assert_eq!(
            TextRenderer::new(plain()).render(&notice),
            "14:02:11.000 -- pod flowcpt-api-7d9f-xk2p1 went away"
        );
    }

    #[test]
    fn a_message_of_several_lines_is_printed_as_it_is() {
        let record = line(
            POD,
            "api",
            Some(Level::Error),
            "boom\n   0: main\n   1: start",
        );
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         ERROR boom\n   0: main\n   1: start"
        );
    }

    #[test]
    fn a_line_that_was_cut_says_so() {
        let mut record = line(POD, "api", None, "xxxx");
        record.truncated = true;
        assert!(
            TextRenderer::new(plain())
                .render(&record)
                .ends_with("xxxx [line cut at 1 MiB]")
        );
    }

    #[test]
    fn which_pod_names_are_shaped_like_a_deployments() {
        assert_eq!(
            deployment_pod_suffix("flowcpt-api-7d9f-xk2p1"),
            Some("7d9f-xk2p1")
        );
        assert_eq!(
            deployment_pod_suffix("flowcpt-web-api-5c8b6d7f9-m4q8z"),
            Some("5c8b6d7f9-m4q8z")
        );
        for other in [
            "redis-0",
            "node-exporter-x7k2p",
            "nightly-sync-29391840-x7k2p",
            // A job name whose digits all happen to be in the alphabet.
            "nightly-sync-29292929-x7k2p",
            "kube-proxy-abcde",
            "7d9f-xk2p1",
            "shell",
            "",
        ] {
            assert_eq!(deployment_pod_suffix(other), None, "{other}");
        }
    }

    #[test]
    fn colours_mark_the_pod_the_level_and_the_field_names() {
        let coloured = TextOptions {
            colour: true,
            ..plain()
        };
        let record = with_fields(
            line(POD, "api", Some(Level::Error), "boom"),
            &[("tenant_id", json!("t_123"))],
        );
        let text = TextRenderer::new(coloured.clone()).render(&record);
        let red_bold = Style::new().fg_color(Some(AnsiColor::Red.into())).bold();
        assert!(
            text.contains(&format!(
                "{}ERROR{}",
                red_bold.render(),
                red_bold.render_reset()
            )),
            "{text:?}"
        );
        let pod_style = Style::new().fg_color(Some(pod_colour(POD).into()));
        assert!(
            text.contains(&format!(
                "{}flowcpt-api…xk2p1    api       {}",
                pod_style.render(),
                pod_style.render_reset()
            )),
            "{text:?}"
        );
        assert!(text.ends_with("t_123  boom"), "{text:?}");

        // Without the escape codes it is the plain line.
        let mut plain_text = String::new();
        let mut rest = text.as_str();
        while let Some(start) = rest.find('\u{1b}') {
            plain_text.push_str(&rest[..start]);
            let end = rest[start..].find('m').expect("an escape code ends in m");
            rest = &rest[start + end + 1..];
        }
        plain_text.push_str(rest);
        assert_eq!(plain_text, TextRenderer::new(plain()).render(&record));
    }

    #[test]
    fn a_pod_keeps_its_colour_and_pods_differ() {
        assert_eq!(pod_colour(POD), pod_colour(POD));
        let colours: std::collections::BTreeSet<String> = (0..50)
            .map(|number| format!("{:?}", pod_colour(&format!("flowcpt-api-7d9f-{number:05}"))))
            .collect();
        assert!(colours.len() >= 5, "{colours:?}");
    }

    #[test]
    fn a_line_of_a_previous_run_is_dimmed_as_a_whole() {
        let coloured = TextOptions {
            colour: true,
            ..plain()
        };
        let mut record = with_fields(
            line(POD, "api", Some(Level::Error), "the panic"),
            &[("tenant_id", json!("t_123"))],
        );
        record.previous = true;
        let dim = Style::new().dimmed();
        assert_eq!(
            TextRenderer::new(coloured).render(&record),
            format!(
                "{}14:02:11.482 flowcpt-api…xk2p1    api         ERROR tenant_id=t_123  the panic{}",
                dim.render(),
                dim.render_reset()
            )
        );
        // Without colours it reads like any other line.
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api…xk2p1    api         ERROR tenant_id=t_123  the panic"
        );
    }

    #[test]
    fn json_output_carries_every_field_of_the_record() {
        let record = with_fields(
            line(POD, "api", Some(Level::Warn), "payment intent expired"),
            &[("tenant_id", json!("t_123"))],
        );
        let text = json_line(&record).unwrap();
        assert!(!text.contains('\n'));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value,
            json!({
                "cluster": "aks-vulcan-01",
                "namespace": "flowcpt",
                "pod": "flowcpt-api-7d9f-xk2p1",
                "container": "api",
                "time": "2026-11-18T14:02:11.482Z",
                "kind": "line",
                "format": "json",
                "level": "warn",
                "message": "payment intent expired",
                "fields": { "tenant_id": "t_123" },
                "raw": "payment intent expired",
                "previous": false,
                "truncated": false
            })
        );
    }
}
