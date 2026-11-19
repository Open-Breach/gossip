//! `gossip replay`: play a recording back, through the same output as
//! `gossip follow` and with the same keys.
//!
//! A recording is what `--record` and `:record` write: one record per line
//! as JSON, with everything a record carries, so a replayed line renders as
//! it did live. It holds what the filter hid at the time as well, which is
//! why the filter flags and the filter commands work on it.

use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Lines};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Days, Local, NaiveDateTime, NaiveTime, TimeZone, Utc};
use gossip_core::{Config, Filter, Record, SessionSpec};
use tokio::time::{Instant, sleep_until};

use crate::cli::ReplayArgs;
use crate::filters::{FilterState, LineRules};
use crate::follow::Printer;
use crate::interactive;
use crate::record::Failures;

pub async fn run(args: ReplayArgs) -> anyhow::Result<()> {
    let config = Config::load()?;
    // How the records are shown: the flags over the config file. What the
    // config file says about clusters and targets is not for a replay.
    let asked = SessionSpec {
        fields: args.output.fields.clone(),
        output: args.output.output,
        columns: args.output.columns.unwrap_or_default(),
        ..SessionSpec::default()
    };
    let mut settings = asked.or(config.base()).resolve();
    let filter = FilterState {
        base: settings.filter.take().filter(|_| !args.filter.no_filter),
        rules: LineRules::from_args(&args.filter)?,
    };
    let stdout_is_terminal = io::stdout().is_terminal();
    let printer = Printer::new(&args.output, &settings, stdout_is_terminal);
    let recording = Recording::open(&args.file, args.speed, args.from, args.to)?;

    if interactive::wanted(&args.output, io::stdin().is_terminal(), stdout_is_terminal) {
        let setup = interactive::Setup {
            printer,
            filter,
            pause_buffer: config.pause_buffer,
            template: None,
            config_path: Config::path()?,
            recorder: None,
            failures: Failures::new(),
        };
        return interactive::replay(recording, setup).await;
    }
    play_plainly(recording, printer, filter.filter()).await
}

/// Prints the records of a recording that pass the filter, until its end
/// or Ctrl-C.
async fn play_plainly(
    mut recording: Recording,
    mut printer: Printer,
    filter: Option<Filter>,
) -> anyhow::Result<()> {
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    loop {
        tokio::select! {
            _ = &mut interrupt => return Ok(()),
            played = recording.next() => match played {
                Some(Played::Record(record)) => {
                    if filter.as_ref().is_none_or(|filter| filter.accepts(&record)) {
                        printer.print(&record)?;
                    }
                }
                Some(Played::Unreadable(what)) => eprintln!("gossip: {what}"),
                None => return Ok(()),
            },
        }
    }
}

/// A time on the command line, for `--from` and `--to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeArg {
    /// A moment: a date and a time.
    At(Moment),
    /// A time of day, on the day the recording begins, in the local zone:
    /// what the time column shows.
    OfDay(NaiveTime),
}

/// A date and a time, with a zone or in the local one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    Utc(DateTime<Utc>),
    Local(NaiveDateTime),
}

impl FromStr for TimeArg {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if let Ok(time) = DateTime::parse_from_rfc3339(text) {
            return Ok(TimeArg::At(Moment::Utc(time.with_timezone(&Utc))));
        }
        for format in [
            "%Y-%m-%dT%H:%M:%S%.f",
            "%Y-%m-%d %H:%M:%S%.f",
            "%Y-%m-%dT%H:%M",
            "%Y-%m-%d %H:%M",
        ] {
            if let Ok(time) = NaiveDateTime::parse_from_str(text, format) {
                return Ok(TimeArg::At(Moment::Local(time)));
            }
        }
        for format in ["%H:%M:%S%.f", "%H:%M"] {
            if let Ok(time) = NaiveTime::parse_from_str(text, format) {
                return Ok(TimeArg::OfDay(time));
            }
        }
        Err(format!(
            "`{text}` is not a time; write a time of day as the time column shows it, 14:02 or \
             14:02:11.482, or a full time, 2026-11-18 14:02 or 2026-11-18T14:02:11Z"
        ))
    }
}

impl TimeArg {
    /// The moment this stands for in a recording that begins at `start`,
    /// with `zone` as the local zone. A time of day is on the day the
    /// recording begins.
    fn moment<Tz: TimeZone>(self, start: DateTime<Utc>, zone: &Tz) -> DateTime<Utc> {
        let local = |time: NaiveDateTime| {
            zone.from_local_datetime(&time)
                .earliest()
                .map_or_else(|| time.and_utc(), |time| time.with_timezone(&Utc))
        };
        match self {
            TimeArg::At(Moment::Utc(time)) => time,
            TimeArg::At(Moment::Local(time)) => local(time),
            TimeArg::OfDay(time) => local(start.with_timezone(zone).date_naive().and_time(time)),
        }
    }
}

/// The part of a recording that is played: from a time, to a time, both
/// included. Without one of them, that end is open.
type Span = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

fn span<Tz: TimeZone>(
    from: Option<TimeArg>,
    to: Option<TimeArg>,
    start: DateTime<Utc>,
    zone: &Tz,
) -> Span {
    let from_moment = from.map(|from| from.moment(start, zone));
    let mut to_moment = to.map(|to| to.moment(start, zone));
    // `--from 23:50 --to 00:10` ends on the day after.
    if let (Some(from), Some(to), Some(TimeArg::OfDay(_))) = (from_moment, to_moment, to)
        && to < from
    {
        to_moment = to.checked_add_days(Days::new(1));
    }
    (from_moment, to_moment)
}

/// What a recording gives next.
#[derive(Debug, PartialEq)]
pub enum Played {
    Record(Record),
    /// A line that is not a record, in words: which line, and why not.
    Unreadable(String),
}

/// A recording being played: its records one after the other, each when
/// it is due.
pub struct Recording {
    path: PathBuf,
    lines: Lines<Box<dyn BufRead + Send>>,
    line: usize,
    /// How many times faster than recorded; zero is without any waiting.
    speed: f64,
    from: Option<TimeArg>,
    to: Option<TimeArg>,
    /// The span as moments, once the first record has said when the
    /// recording begins.
    span: Option<Span>,
    /// The time of the newest record played so far.
    played_to: Option<DateTime<Utc>>,
    /// A record that was read and is not due yet.
    waiting: Option<(Record, Instant)>,
}

impl Recording {
    pub fn open(
        path: &Path,
        speed: f64,
        from: Option<TimeArg>,
        to: Option<TimeArg>,
    ) -> anyhow::Result<Recording> {
        let file = File::open(path)
            .with_context(|| format!("cannot read the recording {}", path.display()))?;
        Ok(Recording::from_reader(
            path,
            Box::new(BufReader::new(file)),
            speed,
            from,
            to,
        ))
    }

    fn from_reader(
        path: &Path,
        reader: Box<dyn BufRead + Send>,
        speed: f64,
        from: Option<TimeArg>,
        to: Option<TimeArg>,
    ) -> Recording {
        Recording {
            path: path.to_path_buf(),
            lines: reader.lines(),
            line: 0,
            speed,
            from,
            to,
            span: None,
            played_to: None,
            waiting: None,
        }
    }

    /// The file the recording is read from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The next record, when it is due; none at the end of the recording.
    ///
    /// Giving up on waiting loses nothing: the record that was read is
    /// kept, and handed out by the next call once it is due.
    pub async fn next(&mut self) -> Option<Played> {
        loop {
            if let Some((_, due)) = &self.waiting {
                sleep_until(*due).await;
                return self
                    .waiting
                    .take()
                    .map(|(record, _)| Played::Record(record));
            }
            let line = match self.lines.next()? {
                Ok(line) => line,
                Err(error) => {
                    // What cannot be read once cannot be read again.
                    self.lines = (Box::new(io::empty()) as Box<dyn BufRead + Send>).lines();
                    return Some(Played::Unreadable(format!(
                        "cannot read on in the recording {}: {error}",
                        self.path.display()
                    )));
                }
            };
            self.line += 1;
            if line.trim().is_empty() {
                continue;
            }
            let record: Record = match serde_json::from_str(&line) {
                Ok(record) => record,
                Err(error) => {
                    return Some(Played::Unreadable(format!(
                        "line {} of the recording {} is not a record: {error}",
                        self.line,
                        self.path.display()
                    )));
                }
            };
            let (from, to) = *self
                .span
                .get_or_insert_with(|| span(self.from, self.to, record.time, &Local));
            if from.is_some_and(|from| record.time < from) || to.is_some_and(|to| record.time > to)
            {
                continue;
            }
            // At the recorded pace, or a multiple of it: as long after the
            // record before as it came then. A record that came late, with
            // an older time, does not wait.
            let gap = self
                .played_to
                .and_then(|before| (record.time - before).to_std().ok())
                .unwrap_or_default();
            self.played_to = Some(self.played_to.map_or(record.time, |to| to.max(record.time)));
            if self.speed <= 0.0 || gap.is_zero() {
                return Some(Played::Record(record));
            }
            let wait = Duration::try_from_secs_f64(gap.as_secs_f64() / self.speed)
                .unwrap_or(Duration::MAX);
            let due = Instant::now().checked_add(wait).unwrap_or_else(far_away);
            self.waiting = Some((record, due));
        }
    }
}

/// A time so far off that it will not come: for a gap that is too long to
/// be a time at all.
fn far_away() -> Instant {
    Instant::now() + Duration::from_secs(60 * 60 * 24 * 365 * 30)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use chrono::{FixedOffset, TimeDelta};
    use gossip_core::{Kind, Level};
    use tokio::time::timeout;

    /// A time `millis` after 14:00 UTC on the day of the tests.
    fn at(millis: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 18, 14, 0, 0).unwrap() + TimeDelta::milliseconds(millis)
    }

    fn line(millis: i64, message: &str) -> Record {
        let mut record = Record::notice("c", "fc-stg", "fc-api-0", None, at(millis), message);
        record.kind = Kind::Line;
        record.container = Some("api".to_string());
        record.level = Some(Level::Info);
        record
    }

    fn text(records: &[Record]) -> String {
        records
            .iter()
            .map(|record| serde_json::to_string(record).unwrap() + "\n")
            .collect()
    }

    fn recording(text: &str, speed: f64, from: Option<&str>, to: Option<&str>) -> Recording {
        Recording::from_reader(
            Path::new("api.jsonl"),
            Box::new(Cursor::new(text.to_string().into_bytes())),
            speed,
            from.map(|from| from.parse().unwrap()),
            to.map(|to| to.parse().unwrap()),
        )
    }

    /// Everything a recording plays, as the messages of its records and
    /// the words about what it could not read.
    async fn played(mut recording: Recording) -> Vec<String> {
        let mut all = Vec::new();
        while let Some(played) = recording.next().await {
            all.push(match played {
                Played::Record(record) => record.message,
                Played::Unreadable(what) => format!("!! {what}"),
            });
        }
        all
    }

    #[tokio::test]
    async fn a_recording_is_played_record_by_record_as_it_was_written() {
        let records = [line(0, "one"), line(100, "two"), line(200, "three")];
        let mut recording = recording(&text(&records), 0.0, None, None);
        assert_eq!(recording.path(), Path::new("api.jsonl"));
        // Each record whole: a replayed line renders as it did live.
        for record in &records {
            assert_eq!(recording.next().await, Some(Played::Record(record.clone())));
        }
        assert_eq!(recording.next().await, None);
        // And the end stays the end.
        assert_eq!(recording.next().await, None);
    }

    #[tokio::test]
    async fn a_line_that_is_not_a_record_is_said_and_the_rest_is_played() {
        let text = format!(
            "{}not json at all\n\n{{\"half\":\"a record\"}}\n{}{}",
            text(&[line(0, "one")]),
            text(&[line(100, "two")]),
            // The last line was cut off while it was written.
            &text(&[line(200, "three")])[..40],
        );
        let all = played(recording(&text, 0.0, None, None)).await;
        assert_eq!(all.len(), 5, "{all:?}");
        assert_eq!(all[0], "one");
        assert!(
            all[1].starts_with("!! line 2 of the recording api.jsonl is not a record: "),
            "{}",
            all[1]
        );
        // The empty line is skipped without a word; the count goes on.
        assert!(all[2].starts_with("!! line 4 of the recording api.jsonl"));
        assert_eq!(all[3], "two");
        assert!(all[4].starts_with("!! line 6 of the recording api.jsonl"));
    }

    #[tokio::test]
    async fn an_empty_recording_plays_nothing() {
        assert!(played(recording("", 1.0, None, None)).await.is_empty());
        assert!(played(recording("\n\n", 0.0, None, None)).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn at_speed_one_the_records_come_at_the_pace_they_were_recorded_at() {
        let records = [
            line(0, "one"),
            line(2_000, "two"),
            line(2_500, "three"),
            line(60_000, "four"),
        ];
        let mut recording = recording(&text(&records), 1.0, None, None);
        let start = Instant::now();
        let mut came = Vec::new();
        while let Some(Played::Record(record)) = recording.next().await {
            came.push((record.message, start.elapsed().as_millis()));
        }
        // The first at once, the others as long after it as they were then:
        // a minute of recording takes a minute.
        assert_eq!(
            came,
            [
                ("one".to_string(), 0),
                ("two".to_string(), 2_000),
                ("three".to_string(), 2_500),
                ("four".to_string(), 60_000)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_higher_speed_is_that_many_times_faster_and_zero_does_not_wait() {
        let records = [line(0, "one"), line(2_000, "two"), line(60_000, "three")];
        let start = Instant::now();
        assert_eq!(
            played(recording(&text(&records), 10.0, None, None)).await,
            ["one", "two", "three"]
        );
        assert_eq!(start.elapsed().as_millis(), 6_000);

        let start = Instant::now();
        played(recording(&text(&records), 0.5, None, None)).await;
        assert_eq!(start.elapsed().as_millis(), 120_000);

        // The default: as fast as it is read.
        let start = Instant::now();
        played(recording(&text(&records), 0.0, None, None)).await;
        assert_eq!(start.elapsed().as_millis(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_record_that_came_late_does_not_wait_and_does_not_set_the_clock_back() {
        let records = [
            line(1_000, "one"),
            line(3_000, "two"),
            // It was emitted late, with an older time.
            line(2_000, "late"),
            line(4_000, "three"),
        ];
        let mut recording = recording(&text(&records), 1.0, None, None);
        let start = Instant::now();
        let mut came = Vec::new();
        while let Some(Played::Record(record)) = recording.next().await {
            came.push((record.message, start.elapsed().as_millis()));
        }
        assert_eq!(
            came,
            [
                ("one".to_string(), 0),
                ("two".to_string(), 2_000),
                ("late".to_string(), 2_000),
                ("three".to_string(), 3_000)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn giving_up_on_a_record_that_is_not_due_loses_nothing() {
        let records = [line(0, "one"), line(5_000, "two"), line(6_000, "three")];
        let mut recording = recording(&text(&records), 1.0, None, None);
        assert!(matches!(recording.next().await, Some(Played::Record(_))));
        // As when a key is pressed while the replay waits: the wait is
        // dropped, more than once.
        for _ in 0..3 {
            assert!(
                timeout(Duration::from_millis(1_000), recording.next())
                    .await
                    .is_err()
            );
        }
        // The record is still the next one, and still on time.
        let start = Instant::now();
        let Some(Played::Record(record)) = recording.next().await else {
            panic!("expected the second record");
        };
        assert_eq!(record.message, "two");
        assert_eq!(start.elapsed().as_millis(), 2_000);
        // After a long pause the pace goes on where it was: the next record
        // comes as long after the pause as it came after the one before it.
        tokio::time::sleep(Duration::from_secs(30)).await;
        let start = Instant::now();
        assert!(matches!(recording.next().await, Some(Played::Record(_))));
        assert_eq!(start.elapsed().as_millis(), 1_000);
    }

    #[tokio::test]
    async fn from_and_to_cut_the_recording_by_the_time_of_its_records() {
        let records: Vec<Record> = (0..=6)
            .map(|minute| line(minute * 60_000, &format!("14:0{minute}")))
            .collect();
        let text = text(&records);
        let cut = |from: Option<&'static str>, to: Option<&'static str>| {
            played(recording(&text, 0.0, from, to))
        };
        // A full time, with its zone. Both ends are included.
        assert_eq!(
            cut(Some("2026-11-18T14:02:00Z"), Some("2026-11-18T14:04:00Z")).await,
            ["14:02", "14:03", "14:04"]
        );
        assert_eq!(cut(Some("2026-11-18T14:05:00.001Z"), None).await, ["14:06"]);
        assert_eq!(
            cut(None, Some("2026-11-18T15:01:30+01:00")).await,
            ["14:00", "14:01"]
        );
        // Nothing in the span is nothing played.
        assert!(cut(Some("2026-11-19T00:00:00Z"), None).await.is_empty());
        assert!(cut(None, Some("2026-11-18T13:00:00Z")).await.is_empty());
    }

    #[test]
    fn a_time_of_day_is_on_the_day_the_recording_begins_in_the_local_zone() {
        // A zone an hour ahead of UTC: the recording begins at 15:00 there.
        let zone = FixedOffset::east_opt(3600).unwrap();
        let start = at(0);
        let moment = |text: &str| text.parse::<TimeArg>().unwrap().moment(start, &zone);
        // As the time column shows it.
        assert_eq!(moment("15:02"), at(2 * 60_000));
        assert_eq!(moment("15:02:11"), at(2 * 60_000 + 11_000));
        assert_eq!(moment("15:02:11.482"), at(2 * 60_000 + 11_482));
        // A date and a time without a zone are local too.
        assert_eq!(moment("2026-11-18 15:02"), at(2 * 60_000));
        assert_eq!(moment("2026-11-18T15:02:11"), at(2 * 60_000 + 11_000));
        assert_eq!(moment("2026-11-18 15:02:11.482"), at(2 * 60_000 + 11_482));
        // A time with a zone is what it says.
        assert_eq!(moment("2026-11-18T14:02:00Z"), at(2 * 60_000));
        assert_eq!(moment("2026-11-18T16:02:00+02:00"), at(2 * 60_000));

        // A recording that begins just before midnight there: the day is
        // the local one.
        let late = Utc.with_ymd_and_hms(2026, 11, 18, 22, 50, 0).unwrap();
        let moment = |text: &str| text.parse::<TimeArg>().unwrap().moment(late, &zone);
        assert_eq!(
            moment("23:55"),
            Utc.with_ymd_and_hms(2026, 11, 18, 22, 55, 0).unwrap()
        );
    }

    #[test]
    fn a_span_that_ends_before_it_begins_ends_on_the_day_after() {
        let zone = Utc;
        let start = Utc.with_ymd_and_hms(2026, 11, 18, 23, 40, 0).unwrap();
        let parsed = |text: &str| Some(text.parse::<TimeArg>().unwrap());
        assert_eq!(
            span(parsed("23:50"), parsed("00:10"), start, &zone),
            (
                Some(Utc.with_ymd_and_hms(2026, 11, 18, 23, 50, 0).unwrap()),
                Some(Utc.with_ymd_and_hms(2026, 11, 19, 0, 10, 0).unwrap())
            )
        );
        // In the usual order nothing moves.
        assert_eq!(
            span(parsed("23:45"), parsed("23:50"), start, &zone),
            (
                Some(Utc.with_ymd_and_hms(2026, 11, 18, 23, 45, 0).unwrap()),
                Some(Utc.with_ymd_and_hms(2026, 11, 18, 23, 50, 0).unwrap())
            )
        );
        assert_eq!(span(None, None, start, &zone), (None, None));
    }

    #[test]
    fn what_is_not_a_time_says_how_one_is_written() {
        for bad in ["soon", "25:00", "14", "14:61", "2026-13-01 10:00", ""] {
            let error = bad.parse::<TimeArg>().unwrap_err();
            assert!(error.contains("is not a time"), "{bad}: {error}");
        }
        assert_eq!(
            " 14:02 ".parse::<TimeArg>(),
            Ok(TimeArg::OfDay(NaiveTime::from_hms_opt(14, 2, 0).unwrap()))
        );
    }

    #[test]
    fn a_recording_that_is_not_there_is_an_error_that_names_it() {
        let error = Recording::open(Path::new("/no/such/recording.jsonl"), 0.0, None, None)
            .err()
            .expect("the file is not there");
        assert_eq!(
            error.to_string(),
            "cannot read the recording /no/such/recording.jsonl"
        );
    }
}
