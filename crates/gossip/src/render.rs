//! Rendering of records: one line of text per record, with a prefix per
//! source, or the record as JSON, or the raw line.

use std::collections::BTreeMap;

use anstyle::{AnsiColor, Style};
use chrono::{DateTime, Local, Utc};
use gossip_core::record::{event_field, value_text};
use gossip_core::resolver::EXITED;
use gossip_core::{Kind, Level, Record, Value};

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
    /// Which fields of a line are shown, in this order; all of them when none.
    pub fields: Option<Vec<String>>,
    /// Keep pod names whole.
    pub long_names: bool,
    /// Mark events with an emoji per reason; with `EVNT` otherwise.
    pub emoji: bool,
    pub zone: Zone,
}

/// Renders records as text, one line per record:
///
/// ```text
/// 14:02:11.482 flowcpt-api-7d9f-xk2p1 api  WARN  tenant_id=t_123 request_id=8b1c  payment intent expired
/// ```
///
/// The renderer remembers how wide the pod and container columns have
/// become, so that the lines stay aligned.
#[derive(Debug)]
pub struct TextRenderer {
    options: TextOptions,
    pod_width: usize,
    container_width: usize,
    /// Every pod seen so far is named like the pod of a deployment, so the
    /// names can be shortened to the replica-set hash and the pod suffix.
    only_deployment_pods: bool,
}

impl TextRenderer {
    pub fn new(options: TextOptions) -> Self {
        TextRenderer {
            options,
            pod_width: 0,
            container_width: 0,
            only_deployment_pods: true,
        }
    }

    /// The text of a record, without a line ending. A record whose message
    /// holds several lines gives several lines.
    pub fn render(&mut self, record: &Record) -> String {
        let time = self.time(record.time);
        if record.kind == Kind::Notice {
            let quiet = Style::new().dimmed();
            return self.paint(quiet, &format!("{time} -- {}", record.message));
        }

        if record.kind == Kind::Event {
            return self.render_event(record, &time);
        }

        let pod = self.pod_name(&record.pod);
        self.pod_width = self.pod_width.max(pod.chars().count());
        let container = record.container.as_deref().unwrap_or("");
        self.container_width = self.container_width.max(container.chars().count());
        let prefix = format!(
            "{pod:<pod_width$} {container:<container_width$}",
            pod_width = self.pod_width,
            container_width = self.container_width
        );
        let level = match record.level {
            Some(level) => format!("{:<5}", level.as_str().to_uppercase()),
            None => " ".repeat(5),
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
            let line = format!("{time} {prefix}  {level} {body}{cut}");
            return self.paint(Style::new().dimmed(), line.trim_end());
        }

        let pod_style = Style::new().fg_color(Some(pod_colour(&record.pod).into()));
        let dim = Style::new().dimmed();
        let fields: Vec<String> = fields
            .iter()
            .map(|(key, value)| format!("{}{value}", self.paint(dim, &format!("{key}="))))
            .collect();
        let body = join_body(&fields.join(" "), &record.message);
        let line = format!(
            "{} {}  {} {body}{}",
            self.paint(dim, &time),
            self.paint(pod_style, &prefix),
            self.paint(level_style(record.level), &level),
            self.paint(dim, cut),
        );
        line.trim_end().to_string()
    }

    /// An event: a mark in place of the container, then reason and message.
    ///
    /// ```text
    /// 14:02:11.490 flowcpt-api-7d9f-xk2p1 💀         Killing: Container api failed liveness probe
    /// ```
    fn render_event(&mut self, record: &Record, time: &str) -> String {
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
            self.pod_name(&record.pod)
        };
        self.pod_width = self.pod_width.max(subject.chars().count());

        // An emoji is two cells wide in most terminals.
        let (mark, mark_width) = if self.options.emoji {
            (event_emoji(&reason, warning), 2)
        } else {
            ("EVNT", 4)
        };
        self.container_width = self.container_width.max(mark_width);
        let line = format!(
            "{time} {subject:<pod_width$} {mark}{padding}  {level} {reason}: {message}",
            pod_width = self.pod_width,
            padding = " ".repeat(self.container_width - mark_width),
            level = " ".repeat(5),
            message = record.message,
        );
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

    /// The name a pod is shown under: whole, or shortened to the replica-set
    /// hash and the pod suffix while every pod seen is a deployment's.
    fn pod_name(&mut self, pod: &str) -> String {
        let short = deployment_pod_suffix(pod);
        if short.is_none() {
            self.only_deployment_pods = false;
        }
        match short {
            Some(short) if self.only_deployment_pods && !self.options.long_names => {
                short.to_string()
            }
            _ => pod.to_string(),
        }
    }

    /// The fields to show, as key and value: the ones asked for in the order
    /// asked, or all of them with the longest last.
    fn fields(&self, fields: &BTreeMap<String, Value>) -> Vec<(String, String)> {
        match &self.options.fields {
            Some(wanted) => wanted
                .iter()
                .filter_map(|key| {
                    fields
                        .get(key)
                        .map(|value| (key.clone(), field_text(value)))
                })
                .collect(),
            None => {
                let mut all: Vec<(String, String)> = fields
                    .iter()
                    .map(|(key, value)| (key.clone(), field_text(value)))
                    .collect();
                all.sort_by_key(|(key, value)| key.chars().count() + value.chars().count());
                all
            }
        }
    }

    fn paint(&self, style: Style, text: &str) -> String {
        if self.options.colour && !text.is_empty() {
            format!("{}{text}{}", style.render(), style.render_reset())
        } else {
            text.to_string()
        }
    }
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
            fields: None,
            long_names: true,
            emoji: true,
            zone: Zone::Utc,
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
        let mut renderer = TextRenderer::new(plain());
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
                "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  WARN  tenant_id=t_123  payment intent expired",
                "14:02:11.490 flowcpt-api-7d9f-xk2p1 💀         Killing: Container api failed liveness probe, will be restarted",
                "14:02:11.490 flowcpt-api-7d9f-xk2p1 🚀         Started: Started container api",
                "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  INFO  starting  version=0.14.2 environment=staging",
            ]
        );
    }

    #[test]
    fn without_emoji_an_event_is_marked_evnt_in_the_same_column() {
        let mut renderer = TextRenderer::new(TextOptions {
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
            "14:02:11.490 flowcpt-api-7d9f-xk2p1 EVNT        Killing: Stopping container api"
        );
        // The column is four wide from here on, so the lines stay aligned.
        assert_eq!(
            renderer.render(&line(POD, "api", Some(Level::Info), "ready")),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api   INFO  ready"
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
            "14:02:11.490 deployment/flowcpt-api ℹ️        ScalingReplicaSet: Scaled up replica set flowcpt-api-7d9f to 2"
        );
        // It does not stop pod names from being shortened.
        let mut renderer = TextRenderer::new(TextOptions {
            long_names: false,
            ..plain()
        });
        renderer.render(&scaled);
        assert!(
            renderer
                .render(&line(POD, "api", None, "x"))
                .contains(" 7d9f-xk2p1 "),
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
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  WARN  request_id=8b1c tenant_id=t_123  payment intent expired"
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
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  INFO  starting  version=0.14.2 environment=staging"
        );
    }

    #[test]
    fn a_line_without_a_level_keeps_the_column() {
        let record = line(POD, "api", None, "listening on 0.0.0.0:8080");
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api        listening on 0.0.0.0:8080"
        );
        let error = line(POD, "api", Some(Level::Error), "boom");
        assert_eq!(
            TextRenderer::new(plain()).render(&error),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  ERROR boom"
        );
    }

    #[test]
    fn fields_are_shown_longest_last_unless_asked_for_by_name() {
        let record = with_fields(
            line(POD, "api", Some(Level::Info), "done"),
            &[
                ("target", json!("flowcpt_api::payments")),
                ("status", json!(200)),
                ("ok", json!(true)),
                ("note", json!("two words")),
                ("empty", json!("")),
            ],
        );
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  INFO  ok=true empty=\"\" status=200 note=\"two words\" target=flowcpt_api::payments  done"
        );

        let chosen = TextOptions {
            fields: Some(vec!["status".into(), "missing".into(), "ok".into()]),
            ..plain()
        };
        assert_eq!(
            TextRenderer::new(chosen).render(&record),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  INFO  status=200 ok=true  done"
        );

        let none = TextOptions {
            fields: Some(vec![]),
            ..plain()
        };
        assert_eq!(
            TextRenderer::new(none).render(&record),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  INFO  done"
        );
    }

    #[test]
    fn a_line_with_fields_and_no_message_shows_the_fields() {
        let record = with_fields(line(POD, "api", None, ""), &[("status", json!(200))]);
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api        status=200"
        );
    }

    #[test]
    fn the_columns_grow_with_the_widest_name_seen() {
        let mut renderer = TextRenderer::new(plain());
        assert_eq!(
            renderer.render(&line("redis-0", "redis", Some(Level::Info), "ready")),
            "14:02:11.482 redis-0 redis  INFO  ready"
        );
        assert_eq!(
            renderer.render(&line(POD, "api", Some(Level::Info), "ready")),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api    INFO  ready"
        );
        // The first pod again, now in the wider columns.
        assert_eq!(
            renderer.render(&line("redis-0", "redis", Some(Level::Info), "ready")),
            "14:02:11.482 redis-0                redis  INFO  ready"
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
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  ERROR boom\n   0: main\n   1: start"
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
    fn pod_names_are_shortened_while_every_pod_is_a_deployments() {
        let short = TextOptions {
            long_names: false,
            ..plain()
        };
        let mut renderer = TextRenderer::new(short.clone());
        assert_eq!(
            renderer.render(&line(POD, "api", Some(Level::Info), "one")),
            "14:02:11.482 7d9f-xk2p1 api  INFO  one"
        );
        assert_eq!(
            renderer.render(&line(
                "flowcpt-web-api-5c8b6d7f9-m4q8z",
                "api",
                Some(Level::Info),
                "two"
            )),
            "14:02:11.482 5c8b6d7f9-m4q8z api  INFO  two"
        );
        // A pod of something else: from here on the names are whole.
        assert_eq!(
            renderer.render(&line("redis-0", "redis", Some(Level::Info), "three")),
            "14:02:11.482 redis-0         redis  INFO  three"
        );
        assert_eq!(
            renderer.render(&line(POD, "api", Some(Level::Info), "four")),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api    INFO  four"
        );

        // With --long-names they are whole from the start.
        assert_eq!(
            TextRenderer::new(plain()).render(&line(POD, "api", Some(Level::Info), "one")),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  INFO  one"
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
                "{}{POD} api{}",
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
                "{}14:02:11.482 flowcpt-api-7d9f-xk2p1 api  ERROR tenant_id=t_123  the panic{}",
                dim.render(),
                dim.render_reset()
            )
        );
        // Without colours it reads like any other line.
        assert_eq!(
            TextRenderer::new(plain()).render(&record),
            "14:02:11.482 flowcpt-api-7d9f-xk2p1 api  ERROR tenant_id=t_123  the panic"
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
