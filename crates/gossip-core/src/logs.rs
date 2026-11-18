//! Log streams: one per followed container, kept going across restarts of
//! the container and across dropped connections.
//!
//! A follower reads the log of one container with `follow` and `timestamps`
//! set. When the stream ends because the container ended, it waits for the
//! pod to show a new run and opens the stream again from the time of the
//! last line it saw, so that nothing is lost and nothing is repeated. When
//! the connection drops, it tries again with a growing pause, forever, and
//! says so in a notice each time.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::io::{AsyncBufRead, AsyncBufReadExt};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::jiff::Timestamp;
use kube::Client;
use kube::api::{Api, LogParams};
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep, sleep_until};

use crate::config::FormatSetting;
use crate::error::describe;
use crate::parser::{LineParser, MAX_LINE_BYTES, ParsedLine};
use crate::record::{Kind, Record};
use crate::resolver::{ContainerId, ContainerView};

/// How long a line is kept back for continuation lines that may follow it,
/// once the stream goes quiet.
const FLUSH_IDLE: Duration = Duration::from_millis(50);

/// How long the pod is given to report the end of a container after its
/// stream ended, before the end is taken for a dropped connection.
const STATUS_GRACE: Duration = Duration::from_secs(2);

/// How long a stream is still read after its container was let go, for the
/// last lines of a pod that went away.
const CLOSING_GRACE: Duration = Duration::from_secs(2);

/// The pause before the first retry, and the longest pause.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// A raw line may be this much longer than a parsed one: the kubelet's
/// timestamp and the space after it.
const TIMESTAMP_ROOM: usize = 128;

/// The history a container gets when it is first read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct History {
    pub since: Option<Duration>,
    pub tail: Option<u64>,
}

/// What every follower of a session shares.
pub(crate) struct Followers {
    pub client: Client,
    pub cluster: String,
    pub history: History,
    pub formats: FormatSetting,
    /// Where the records go: the merge of the session.
    pub records: mpsc::Sender<Record>,
}

/// The container was let go, or the session ended: stop following.
#[derive(Debug)]
struct Gone;

/// How a stream came to its end.
enum End {
    /// The server closed it in good order.
    Finished,
    /// It broke.
    Broken(String),
}

/// Which run of the container a stream reads, and what that means for its
/// lines.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// The current run.
    Current,
    /// The run before the current one, read at start: its lines are marked
    /// `previous` and do not move the position.
    PreviousAtStart,
    /// The rest of a run that ended while its stream was down.
    RestOfPrevious,
}

/// Why a stream could not be opened.
enum Refusal {
    /// The pod is not there.
    Gone,
    /// The container has no log to read (yet).
    NotReady,
    Other(String),
}

fn refusal(error: &kube::Error) -> Refusal {
    match error {
        kube::Error::Api(status) if status.code == 404 => Refusal::Gone,
        kube::Error::Api(status) if status.code == 400 => Refusal::NotReady,
        other => Refusal::Other(describe(other)),
    }
}

/// Follows one container until it is let go: `status` is the state the pod
/// reports for it, and following ends when the sender of `status` is dropped.
/// `initial` says the container was there when the session started.
pub(crate) async fn follow(
    context: Arc<Followers>,
    id: ContainerId,
    initial: bool,
    status: watch::Receiver<ContainerView>,
) {
    let api = Api::namespaced(context.client.clone(), &id.namespace);
    let format = context.formats.for_container(&id.container);
    let mut follower = Follower {
        context,
        id,
        api,
        status,
        format,
        position: None,
        backoff: Backoff::new(),
        waiting_noticed: None,
    };
    // Gone is the one way out, and not an error.
    let _ = follower.run(initial).await;
}

struct Follower {
    context: Arc<Followers>,
    id: ContainerId,
    api: Api<Pod>,
    status: watch::Receiver<ContainerView>,
    format: crate::parser::FormatChoice,
    /// The last line seen of the container's log.
    position: Option<Position>,
    backoff: Backoff,
    /// The reason of the waiting state that a notice has told already.
    waiting_noticed: Option<String>,
}

impl Follower {
    async fn run(&mut self, initial: bool) -> Result<(), Gone> {
        let first = self.status.borrow().clone();
        // A container that exists at start gets the start-up history; one
        // that starts later is read from its beginning.
        let mut history_pending = initial && first.state.has_output();
        if initial && has_previous_run(&first) {
            self.read_previous(Reading::PreviousAtStart).await?;
        }

        // The run, by its restart count, that was read to its end, and the
        // run of the stream opened last.
        let mut finished: Option<i32> = None;
        let mut reading: Option<i32> = None;
        loop {
            let view = self.wait_for_output(finished).await?;
            let run = view.restart_count;
            if let Some(read) = reading
                && run > read
                && finished != Some(read)
                && self.position.is_some()
            {
                // The run being read ended while its stream was down.
                self.read_previous(Reading::RestOfPrevious).await?;
            }
            reading = Some(run);

            let params = self.params(history_pending);
            match self.api.log_stream(&self.id.pod, &params).await {
                Ok(stream) => match self
                    .read(Box::pin(stream), Reading::Current, &mut history_pending)
                    .await?
                {
                    End::Finished => {
                        if self.ended_for_good(run).await? {
                            finished = Some(run);
                        } else {
                            let pause = self.backoff.next();
                            let message = format!(
                                "the log stream of {} ended while the container is still running; \
                                 opening it again in {}",
                                self.name(),
                                seconds(pause)
                            );
                            self.notice(message).await?;
                            self.pause(pause).await?;
                        }
                    }
                    End::Broken(reason) => {
                        let pause = self.backoff.next();
                        let message = format!(
                            "lost the log stream of {}: {reason}; opening it again in {}",
                            self.name(),
                            seconds(pause)
                        );
                        self.notice(message).await?;
                        self.pause(pause).await?;
                    }
                },
                Err(error) => match refusal(&error) {
                    // The resolver will let the container go in a moment.
                    Refusal::Gone => self.status.changed().await.map_err(|_| Gone)?,
                    Refusal::NotReady => {
                        let pause = self.backoff.next();
                        self.wait_for_change_or(pause).await?;
                    }
                    Refusal::Other(reason) => {
                        let pause = self.backoff.next();
                        let message = format!(
                            "cannot open the log stream of {}: {reason}; trying again in {}",
                            self.name(),
                            seconds(pause)
                        );
                        self.notice(message).await?;
                        self.pause(pause).await?;
                    }
                },
            }
        }
    }

    /// The container as notices name it.
    fn name(&self) -> String {
        format!("{} in pod {}", self.id.container, self.id.pod)
    }

    fn params(&self, history_pending: bool) -> LogParams {
        let mut params = LogParams {
            container: Some(self.id.container.clone()),
            follow: true,
            timestamps: true,
            ..LogParams::default()
        };
        if history_pending {
            self.add_history(&mut params);
        } else if let Some(position) = self.position {
            params.since_time = Some(to_timestamp(position.time));
        }
        params
    }

    fn add_history(&self, params: &mut LogParams) {
        let history = self.context.history;
        params.tail_lines = history
            .tail
            .map(|lines| i64::try_from(lines).unwrap_or(i64::MAX));
        // The API refuses zero seconds.
        params.since_seconds = history
            .since
            .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX).max(1));
    }

    /// Waits until the container has output that was not read to its end
    /// yet, and tells its waiting state on the way.
    async fn wait_for_output(&mut self, finished: Option<i32>) -> Result<ContainerView, Gone> {
        loop {
            let view = self.status.borrow_and_update().clone();
            self.notice_waiting(&view).await?;
            if view.state.has_output() && finished != Some(view.restart_count) {
                return Ok(view);
            }
            self.status.changed().await.map_err(|_| Gone)?;
        }
    }

    /// A container that waits has no stream; its state comes as a notice
    /// whenever the reason changes.
    async fn notice_waiting(&mut self, view: &ContainerView) -> Result<(), Gone> {
        let crate::resolver::ContainerState::Waiting { reason, message } = &view.state else {
            self.waiting_noticed = None;
            return Ok(());
        };
        let reason = reason
            .clone()
            .unwrap_or_else(|| "no reason given".to_string());
        if self.waiting_noticed.as_ref() == Some(&reason) {
            return Ok(());
        }
        let text = match message {
            Some(message) if !message.is_empty() => {
                format!("{} is waiting: {reason} ({message})", self.name())
            }
            _ => format!("{} is waiting: {reason}", self.name()),
        };
        self.waiting_noticed = Some(reason);
        self.notice(text).await
    }

    /// Whether the run whose stream just ended is over. The pod may report
    /// the end a moment after the stream closes, so it gets a short while
    /// to do so; if it still shows the container running, the stream was
    /// closed on us.
    async fn ended_for_good(&mut self, run: i32) -> Result<bool, Gone> {
        let grace = sleep(STATUS_GRACE);
        tokio::pin!(grace);
        loop {
            let view = self.status.borrow_and_update().clone();
            if view.restart_count != run || !view.state.is_running() {
                return Ok(true);
            }
            tokio::select! {
                _ = &mut grace => return Ok(false),
                changed = self.status.changed() => changed.map_err(|_| Gone)?,
            }
        }
    }

    /// Sleeps, unless the container is let go meanwhile.
    async fn pause(&mut self, pause: Duration) -> Result<(), Gone> {
        let wake = sleep(pause);
        tokio::pin!(wake);
        loop {
            tokio::select! {
                _ = &mut wake => return Ok(()),
                changed = self.status.changed() => changed.map_err(|_| Gone)?,
            }
        }
    }

    /// Waits for the pod to report something new about the container, but
    /// not longer than `pause`.
    async fn wait_for_change_or(&mut self, pause: Duration) -> Result<(), Gone> {
        tokio::select! {
            _ = sleep(pause) => Ok(()),
            changed = self.status.changed() => changed.map_err(|_| Gone),
        }
    }

    /// Reads the run before the current one: its tail at start, or the rest
    /// of it after an outage. Not being able to is not worth more than a
    /// notice, and often it is simply not there.
    async fn read_previous(&mut self, reading: Reading) -> Result<(), Gone> {
        let mut params = LogParams {
            container: Some(self.id.container.clone()),
            previous: true,
            timestamps: true,
            ..LogParams::default()
        };
        match (reading, self.position) {
            (Reading::RestOfPrevious, Some(position)) => {
                params.since_time = Some(to_timestamp(position.time));
            }
            _ => self.add_history(&mut params),
        }
        let failure = match self.api.log_stream(&self.id.pod, &params).await {
            Ok(stream) => match self.read(Box::pin(stream), reading, &mut false).await? {
                End::Finished => return Ok(()),
                End::Broken(reason) => reason,
            },
            Err(error) => match refusal(&error) {
                Refusal::Gone | Refusal::NotReady => return Ok(()),
                Refusal::Other(reason) => reason,
            },
        };
        let message = format!("cannot read the previous run of {}: {failure}", self.name());
        self.notice(message).await
    }

    /// Reads a stream to its end and passes its lines on as records.
    async fn read(
        &mut self,
        stream: Pin<Box<dyn AsyncBufRead + Send>>,
        reading: Reading,
        history_pending: &mut bool,
    ) -> Result<End, Gone> {
        let mut lines = LineReader::new(stream, MAX_LINE_BYTES + TIMESTAMP_ROOM);
        let mut parser = LineParser::new(self.format);
        let previous = reading == Reading::PreviousAtStart;
        // What was seen before is skipped when the server sends it again.
        let mut resume = match reading {
            Reading::PreviousAtStart => None,
            Reading::Current | Reading::RestOfPrevious => self.position.map(Resume::after),
        };
        let mut flush_at: Option<Instant> = None;
        let mut closing_at: Option<Instant> = None;

        let end = loop {
            tokio::select! {
                biased;
                line = lines.next_line() => match line {
                    Ok(Some(bytes)) => {
                        let text = String::from_utf8_lossy(&bytes);
                        // A line without the kubelet's timestamp is something the
                        // kubelet or the API server put there itself.
                        let (time, line) = split_timestamp(&text).unwrap_or_else(|| {
                            let time = self.position.map_or_else(Utc::now, |position| position.time);
                            (time, &text)
                        });
                        if resume.as_mut().is_some_and(|resume| resume.skips(time)) {
                            continue;
                        }
                        if reading != Reading::PreviousAtStart {
                            self.position = Some(Position::advance(self.position, time));
                        }
                        if reading == Reading::Current {
                            *history_pending = false;
                            self.backoff.reset();
                        }
                        if let Some(parsed) = parser.push(time, line) {
                            self.send_line(parsed, previous).await?;
                        }
                        flush_at = Some(Instant::now() + FLUSH_IDLE);
                    }
                    Ok(None) => break End::Finished,
                    Err(error) => break End::Broken(error.to_string()),
                },
                _ = sleep_until(flush_at.unwrap_or_else(Instant::now)), if flush_at.is_some() => {
                    flush_at = None;
                    if let Some(parsed) = parser.flush() {
                        self.send_line(parsed, previous).await?;
                    }
                }
                changed = self.status.changed(), if closing_at.is_none() => {
                    if changed.is_err() {
                        closing_at = Some(Instant::now() + CLOSING_GRACE);
                    }
                }
                _ = sleep_until(closing_at.unwrap_or_else(Instant::now)), if closing_at.is_some() => {
                    if let Some(parsed) = parser.flush() {
                        self.send_line(parsed, previous).await?;
                    }
                    return Err(Gone);
                }
            }
        };
        if let Some(parsed) = parser.flush() {
            self.send_line(parsed, previous).await?;
        }
        if closing_at.is_some() {
            return Err(Gone);
        }
        Ok(end)
    }

    async fn send_line(&self, line: ParsedLine, previous: bool) -> Result<(), Gone> {
        let record = Record {
            cluster: self.context.cluster.clone(),
            namespace: self.id.namespace.clone(),
            pod: self.id.pod.clone(),
            container: Some(self.id.container.clone()),
            time: line.time,
            kind: Kind::Line,
            format: line.format,
            level: line.level,
            message: line.message,
            fields: line.fields,
            raw: line.raw,
            previous,
            truncated: line.truncated,
        };
        self.context.records.send(record).await.map_err(|_| Gone)
    }

    async fn notice(&self, message: String) -> Result<(), Gone> {
        let record = Record::notice(
            self.context.cluster.clone(),
            self.id.namespace.clone(),
            self.id.pod.clone(),
            Some(self.id.container.clone()),
            Utc::now(),
            message,
        );
        self.context.records.send(record).await.map_err(|_| Gone)
    }
}

/// Whether a container gets the tail of its previous run at start: its
/// restart count is above zero, or it waits in a crash loop.
fn has_previous_run(view: &ContainerView) -> bool {
    view.restart_count > 0 || view.state.waiting_reason() == Some("CrashLoopBackOff")
}

fn seconds(pause: Duration) -> String {
    format!("{} s", pause.as_secs())
}

fn to_timestamp(time: DateTime<Utc>) -> Timestamp {
    Timestamp::new(time.timestamp(), time.timestamp_subsec_nanos() as i32)
        .unwrap_or(Timestamp::UNIX_EPOCH)
}

/// Splits the kubelet's timestamp off the front of a line.
fn split_timestamp(line: &str) -> Option<(DateTime<Utc>, &str)> {
    let (stamp, rest) = line.split_once(' ').unwrap_or((line, ""));
    let time = DateTime::parse_from_rfc3339(stamp).ok()?;
    Some((time.with_timezone(&Utc), rest))
}

/// The last line seen of a log: its time, and how many lines carried
/// exactly that time. The API takes a start time in whole seconds only, so
/// a stream that is opened again repeats the lines of that second; the
/// position tells which of them are old.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    time: DateTime<Utc>,
    count: usize,
}

impl Position {
    fn advance(position: Option<Position>, time: DateTime<Utc>) -> Position {
        match position {
            Some(position) if time == position.time => Position {
                time,
                count: position.count + 1,
            },
            // A line older than the last one does not move the position back.
            Some(position) if time < position.time => position,
            _ => Position { time, count: 1 },
        }
    }
}

/// Skips the lines of a reopened stream that were seen before.
#[derive(Debug)]
struct Resume {
    position: Position,
    /// How many lines with the time of the position were skipped so far.
    skipped: usize,
}

impl Resume {
    fn after(position: Position) -> Resume {
        Resume {
            position,
            skipped: 0,
        }
    }

    /// Whether a line with this time was seen before.
    fn skips(&mut self, time: DateTime<Utc>) -> bool {
        if time < self.position.time {
            return true;
        }
        if time == self.position.time && self.skipped < self.position.count {
            self.skipped += 1;
            return true;
        }
        false
    }
}

/// The pause between retries: 1 s, doubled each time, up to 30 s.
#[derive(Debug)]
struct Backoff {
    next: Duration,
}

impl Backoff {
    fn new() -> Backoff {
        Backoff {
            next: BACKOFF_START,
        }
    }

    fn next(&mut self) -> Duration {
        let pause = self.next;
        self.next = (self.next * 2).min(BACKOFF_MAX);
        pause
    }

    fn reset(&mut self) {
        self.next = BACKOFF_START;
    }
}

/// Reads lines of bytes off a stream. A line longer than the limit is cut
/// at the limit and the rest of it is thrown away, so that one endless line
/// cannot fill the memory.
struct LineReader<R> {
    reader: R,
    line: Vec<u8>,
    limit: usize,
}

impl<R: AsyncBufRead + Unpin> LineReader<R> {
    fn new(reader: R, limit: usize) -> Self {
        LineReader {
            reader,
            line: Vec::new(),
            limit,
        }
    }

    /// The next line without its line feed, or none at the end of the
    /// stream. A last line without a line feed is a line too.
    ///
    /// Dropping the future loses nothing: what was read stays in the reader.
    async fn next_line(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                if self.line.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(std::mem::take(&mut self.line)));
            }
            let (part, ends) = match available.iter().position(|byte| *byte == b'\n') {
                Some(index) => (&available[..index], true),
                None => (available, false),
            };
            let room = self.limit.saturating_sub(self.line.len());
            self.line.extend_from_slice(&part[..part.len().min(room)]);
            let used = part.len() + usize::from(ends);
            self.reader.consume_unpin(used);
            if ends {
                return Ok(Some(std::mem::take(&mut self.line)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeDelta, TimeZone};
    use futures::io::Cursor;

    fn at(millis: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap() + TimeDelta::milliseconds(millis)
    }

    async fn all_lines(text: &[u8], limit: usize) -> Vec<String> {
        let mut reader = LineReader::new(Cursor::new(text.to_vec()), limit);
        let mut lines = Vec::new();
        while let Some(line) = reader.next_line().await.unwrap() {
            lines.push(String::from_utf8_lossy(&line).into_owned());
        }
        lines
    }

    #[tokio::test]
    async fn lines_are_split_on_line_feeds() {
        assert_eq!(
            all_lines(b"one\ntwo\n\nfour\n", 100).await,
            ["one", "two", "", "four"]
        );
        assert_eq!(all_lines(b"", 100).await, Vec::<String>::new());
        assert_eq!(all_lines(b"\n", 100).await, [""]);
    }

    #[tokio::test]
    async fn a_last_line_without_a_line_feed_is_a_line() {
        assert_eq!(all_lines(b"one\ntwo", 100).await, ["one", "two"]);
    }

    #[tokio::test]
    async fn a_line_longer_than_the_limit_is_cut_and_the_next_is_whole() {
        assert_eq!(
            all_lines(b"0123456789abcdef\nshort\n", 10).await,
            ["0123456789", "short"]
        );
    }

    #[tokio::test]
    async fn a_line_that_comes_in_pieces_is_put_together() {
        // A reader that hands out three bytes at a time.
        let text = b"hello world\nsecond line\n".to_vec();
        let pieces = futures::stream::iter(
            text.chunks(3)
                .map(|chunk| Ok::<_, io::Error>(chunk.to_vec()))
                .collect::<Vec<_>>(),
        );
        let reader = futures::TryStreamExt::into_async_read(pieces);
        let mut reader = LineReader::new(reader, 100);
        assert_eq!(reader.next_line().await.unwrap().unwrap(), b"hello world");
        assert_eq!(reader.next_line().await.unwrap().unwrap(), b"second line");
        assert_eq!(reader.next_line().await.unwrap(), None);
    }

    #[test]
    fn the_timestamp_of_the_kubelet_is_split_off() {
        let (time, line) = split_timestamp("2026-11-18T14:02:11.482913845Z hello world").unwrap();
        assert_eq!(time, at(482) + TimeDelta::nanoseconds(913_845));
        assert_eq!(line, "hello world");

        // An empty line is a timestamp and a space, or just a timestamp.
        assert_eq!(
            split_timestamp("2026-11-18T14:02:11Z ").unwrap(),
            (at(0), "")
        );
        assert_eq!(
            split_timestamp("2026-11-18T14:02:11Z").unwrap(),
            (at(0), "")
        );
        // Another offset is brought to UTC.
        assert_eq!(
            split_timestamp("2026-11-18T15:02:11+01:00 x").unwrap(),
            (at(0), "x")
        );
        // Only the first space splits.
        assert_eq!(
            split_timestamp("2026-11-18T14:02:11Z  two spaces")
                .unwrap()
                .1,
            " two spaces"
        );
    }

    #[test]
    fn a_line_without_a_timestamp_is_left_whole() {
        assert_eq!(split_timestamp("unable to retrieve container logs"), None);
        assert_eq!(split_timestamp(""), None);
        assert_eq!(split_timestamp("14:02:11 not a full timestamp"), None);
    }

    #[test]
    fn the_position_counts_the_lines_of_the_last_instant() {
        let first = Position::advance(None, at(0));
        assert_eq!(
            first,
            Position {
                time: at(0),
                count: 1
            }
        );
        let same = Position::advance(Some(first), at(0));
        assert_eq!(
            same,
            Position {
                time: at(0),
                count: 2
            }
        );
        let later = Position::advance(Some(same), at(5));
        assert_eq!(
            later,
            Position {
                time: at(5),
                count: 1
            }
        );
        // An older line does not move it back.
        assert_eq!(Position::advance(Some(later), at(1)), later);
    }

    #[test]
    fn a_reopened_stream_skips_what_was_seen() {
        // Two lines at the last instant were seen before the stream broke.
        let mut resume = Resume::after(Position {
            time: at(100),
            count: 2,
        });
        // The server starts at the whole second, so it sends them all again.
        assert!(resume.skips(at(0)));
        assert!(resume.skips(at(50)));
        assert!(resume.skips(at(100)));
        assert!(resume.skips(at(100)));
        // A third line at the same instant is new, and so is everything after.
        assert!(!resume.skips(at(100)));
        assert!(!resume.skips(at(101)));
        assert!(!resume.skips(at(100)));
    }

    #[test]
    fn the_pause_doubles_from_one_second_to_thirty() {
        let mut backoff = Backoff::new();
        let pauses: Vec<u64> = (0..8).map(|_| backoff.next().as_secs()).collect();
        assert_eq!(pauses, [1, 2, 4, 8, 16, 30, 30, 30]);
        backoff.reset();
        assert_eq!(backoff.next().as_secs(), 1);
    }

    #[test]
    fn a_time_becomes_the_timestamp_the_api_takes() {
        let stamp = to_timestamp(at(482));
        assert_eq!(stamp.to_string(), "2026-11-18T14:02:11.482Z");
    }

    #[test]
    fn which_containers_get_the_tail_of_their_previous_run() {
        use crate::resolver::{ContainerRole, ContainerState};
        let view = |restart_count: i32, state: ContainerState| ContainerView {
            name: "api".into(),
            role: ContainerRole::Main,
            restart_count,
            state,
            has_previous: restart_count > 0,
            last_exit: None,
        };
        let running = ContainerState::Running { started_at: None };
        let crash_loop = ContainerState::Waiting {
            reason: Some("CrashLoopBackOff".into()),
            message: None,
        };
        let pulling = ContainerState::Waiting {
            reason: Some("ContainerCreating".into()),
            message: None,
        };
        assert!(!has_previous_run(&view(0, running.clone())));
        assert!(has_previous_run(&view(3, running)));
        assert!(has_previous_run(&view(0, crash_loop)));
        assert!(!has_previous_run(&view(0, pulling)));
    }
}
