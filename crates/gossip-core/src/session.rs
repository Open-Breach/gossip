//! The session: targets, a filter and options in, one stream of records out.
//!
//! A session resolves its targets, follows every container they select and
//! merges what comes in into one stream ordered by time. Stopping it, by
//! dropping it, closes every stream and every watch.

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use futures::Stream;
use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, sleep_until};

use crate::cluster::Cluster;
use crate::config::{DEFAULT_TAIL, FormatSetting, Settings};
use crate::filter::Filter;
use crate::logs::{self, Followers, History};
use crate::merge::{DEFAULT_HOLD, ReorderBuffer};
use crate::record::{Format, Kind, Level, Record, event_field};
use crate::resolver::{
    ClusterEvent, ContainerEvent, ContainerId, ContainerView, Resolver, ResolverEvent,
};
use crate::target::{Pattern, Target};

/// The options of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionOptions {
    /// History per container at start, as a duration.
    pub since: Option<Duration>,
    /// History per container at start, as a number of lines. With `since`
    /// as well, the last lines of that time.
    pub tail: Option<u64>,
    /// Whether the cluster's events about the followed pods, and about the
    /// workloads above them, are merged into the stream.
    pub events: bool,
    /// Only containers whose name matches are followed, to keep sidecars out.
    pub containers: Option<Pattern>,
    /// Which recogniser reads the lines, for all containers or per container.
    pub format: FormatSetting,
    /// How long a record is held back so that records of different sources
    /// come out in the order of their time.
    pub hold: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        SessionOptions {
            since: None,
            tail: Some(DEFAULT_TAIL),
            events: true,
            containers: None,
            format: FormatSetting::default(),
            hold: DEFAULT_HOLD,
        }
    }
}

impl From<&Settings> for SessionOptions {
    fn from(settings: &Settings) -> Self {
        SessionOptions {
            since: settings.since,
            tail: settings.tail,
            events: settings.events,
            containers: settings.container.clone(),
            format: settings.format.clone(),
            hold: DEFAULT_HOLD,
        }
    }
}

/// A running session: a stream of [`Record`]s. It never ends by itself;
/// dropping it stops everything behind it.
#[derive(Debug)]
pub struct Session {
    records: mpsc::Receiver<Record>,
    task: JoinHandle<()>,
}

impl Session {
    /// Starts following the targets on a cluster. Records that the filter
    /// does not accept are left out; notices always pass. Must be called
    /// inside a tokio runtime.
    ///
    /// Starting never fails: a cluster that cannot be reached shows as
    /// notices while the session keeps trying. Use [`Cluster::check`] first
    /// to find out before starting.
    pub fn start(
        cluster: &Cluster,
        targets: Vec<Target>,
        filter: Option<Filter>,
        options: SessionOptions,
    ) -> Session {
        let (tx, records) = mpsc::channel(1024);
        let task = tokio::spawn(drive(cluster.clone(), targets, filter, options, tx));
        Session { records, task }
    }

    /// The next record of the stream.
    pub async fn next(&mut self) -> Option<Record> {
        self.records.recv().await
    }
}

impl Stream for Session {
    type Item = Record;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.records.poll_recv(cx)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // The task owns the resolver and every follower.
        self.task.abort();
    }
}

/// The one task that owns every stream of a session: it starts and stops
/// followers as the resolver says, and passes their records through the
/// reorder buffer and the filter.
async fn drive(
    cluster: Cluster,
    targets: Vec<Target>,
    filter: Option<Filter>,
    options: SessionOptions,
    out: mpsc::Sender<Record>,
) {
    // Events are told from where the history of the lines begins: as far
    // back as `since` reaches, or from now on.
    let events_since = options.events.then(|| {
        let back = options
            .since
            .and_then(|since| TimeDelta::from_std(since).ok())
            .unwrap_or_default();
        // Many events carry their time in whole seconds, so the start of
        // the history is a whole second too; otherwise an event of this
        // very second would count as older than the session.
        let start = Utc::now() - back;
        DateTime::from_timestamp(start.timestamp(), 0).unwrap_or(start)
    });
    let mut resolver = Resolver::start(&cluster, targets, options.containers.clone(), events_since);
    let (records_tx, mut records_rx) = mpsc::channel(1024);
    let context = Arc::new(Followers {
        client: cluster.client(),
        cluster: cluster.name().to_string(),
        history: History {
            since: options.since,
            tail: options.tail,
        },
        formats: options.format.clone(),
        records: records_tx,
    });
    // The state of every followed container, as its follower sees it.
    let mut followers: HashMap<ContainerId, watch::Sender<ContainerView>> = HashMap::new();
    let mut tasks = JoinSet::new();
    let mut buffer = ReorderBuffer::new(options.hold);

    loop {
        let deadline = buffer.next_deadline().map(Instant::from_std);
        tokio::select! {
            event = resolver.next() => match event {
                Some(ResolverEvent::Container(ContainerEvent::Added { id, view, initial })) => {
                    let (status, receiver) = watch::channel(view);
                    tasks.spawn(logs::follow(context.clone(), id.clone(), initial, receiver));
                    followers.insert(id, status);
                }
                Some(ResolverEvent::Container(ContainerEvent::Changed { id, view })) => {
                    if let Some(status) = followers.get(&id) {
                        status.send_replace(view);
                    }
                }
                Some(ResolverEvent::Container(ContainerEvent::Removed { id })) => {
                    // Dropping the state lets the follower read what is left
                    // and finish.
                    followers.remove(&id);
                }
                Some(ResolverEvent::Cluster(event)) => {
                    buffer.push(event_record(cluster.name(), event), Instant::now().into_std());
                }
                Some(ResolverEvent::Notice(notice)) => {
                    let record = Record::notice(
                        cluster.name(),
                        notice.namespace,
                        notice.pod.unwrap_or_default(),
                        None,
                        Utc::now(),
                        notice.message,
                    );
                    buffer.push(record, Instant::now().into_std());
                }
                None => return,
            },
            Some(record) = records_rx.recv() => buffer.push(record, Instant::now().into_std()),
            _ = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {}
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }

        for record in buffer.pop_ready(Instant::now().into_std()) {
            let accepted = filter.as_ref().is_none_or(|filter| filter.accepts(&record));
            if accepted && out.send(record).await.is_err() {
                return;
            }
        }
    }
}

/// A cluster event as a record of kind `event`: the reason, the type and
/// the count are fields, `Warning` is the level `warn` and `Normal` is
/// `info`.
fn event_record(cluster: &str, event: ClusterEvent) -> Record {
    let fields = BTreeMap::from([
        (event_field::REASON.to_string(), json!(event.reason)),
        (
            event_field::TYPE.to_string(),
            json!(if event.warning { "Warning" } else { "Normal" }),
        ),
        (event_field::COUNT.to_string(), json!(event.count)),
        (event_field::REGARDING.to_string(), json!(event.regarding)),
    ]);
    Record {
        cluster: cluster.to_string(),
        namespace: event.namespace,
        pod: event.pod.unwrap_or_default(),
        container: event.container,
        time: event.time.unwrap_or_else(Utc::now),
        kind: Kind::Event,
        format: Format::Text,
        level: Some(if event.warning {
            Level::Warn
        } else {
            Level::Info
        }),
        raw: format!("{}: {}", event.reason, event.message),
        message: event.message,
        fields,
        previous: false,
        truncated: false,
    }
}

#[cfg(test)]
mod cluster_tests;
