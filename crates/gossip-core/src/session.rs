//! The session: targets, a filter and options in, one stream of records out.
//!
//! A session resolves its targets, follows every container they select and
//! merges what comes in into one stream ordered by time. Stopping it, by
//! dropping it, closes every stream and every watch.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use chrono::Utc;
use futures::Stream;
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, sleep_until};

use crate::cluster::Cluster;
use crate::config::{DEFAULT_TAIL, FormatSetting, Settings};
use crate::filter::Filter;
use crate::logs::{self, Followers, History};
use crate::merge::{DEFAULT_HOLD, ReorderBuffer};
use crate::record::Record;
use crate::resolver::{ContainerEvent, ContainerId, ContainerView, Resolver, ResolverEvent};
use crate::target::{Pattern, Target};

/// The options of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionOptions {
    /// History per container at start, as a duration.
    pub since: Option<Duration>,
    /// History per container at start, as a number of lines. With `since`
    /// as well, the last lines of that time.
    pub tail: Option<u64>,
    /// Only containers whose name matches are followed, to keep sidecars out.
    pub containers: Option<Pattern>,
    /// Which recogniser reads the lines, for all containers or per container.
    pub format: FormatSetting,
    /// How long a record is held back so that records of different sources
    /// come out in the order of their time.
    pub hold: Duration,
    /// How long a log stream may be silent before it is opened again. A
    /// connection can die without a word, and silence is the only sign.
    pub idle: Duration,
}

/// How long a silent log stream is left alone by default.
pub const DEFAULT_IDLE: Duration = Duration::from_secs(90);

impl Default for SessionOptions {
    fn default() -> Self {
        SessionOptions {
            since: None,
            tail: Some(DEFAULT_TAIL),
            containers: None,
            format: FormatSetting::default(),
            hold: DEFAULT_HOLD,
            idle: DEFAULT_IDLE,
        }
    }
}

impl From<&Settings> for SessionOptions {
    fn from(settings: &Settings) -> Self {
        SessionOptions {
            since: settings.since,
            tail: settings.tail,
            containers: settings.container.clone(),
            format: settings.format.clone(),
            hold: DEFAULT_HOLD,
            idle: DEFAULT_IDLE,
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
    let mut resolver = Resolver::start(&cluster, targets, options.containers.clone());
    let (records_tx, mut records_rx) = mpsc::channel(1024);
    let context = Arc::new(Followers {
        client: cluster.client(),
        cluster: cluster.name().to_string(),
        history: History {
            since: options.since,
            tail: options.tail,
        },
        formats: options.format.clone(),
        idle: options.idle,
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

#[cfg(test)]
mod cluster_tests;
