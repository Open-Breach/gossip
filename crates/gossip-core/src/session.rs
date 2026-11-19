//! The session: targets, a filter and options in, one stream of records out.
//!
//! A session resolves its targets, follows every container they select and
//! merges what comes in into one stream ordered by time. Stopping it, by
//! dropping it, closes every stream and every watch.
//!
//! What a session follows can change while it runs: its [`SessionHandle`]
//! replaces the filter, adds and drops targets, switches the events and
//! changes the container glob. Nothing in here knows who asks, a key on a
//! command line or a click in a window. A session nobody changes does what
//! it did before it could be changed.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use futures::Stream;
use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, sleep_until};

use crate::cluster::Cluster;
use crate::config::{DEFAULT_TAIL, FormatSetting, SessionSpec, Settings};
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
            events: true,
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
            events: settings.events,
            containers: settings.container.clone(),
            format: settings.format.clone(),
            hold: DEFAULT_HOLD,
            idle: DEFAULT_IDLE,
        }
    }
}

/// A running session: a stream of [`Record`]s. It never ends by itself;
/// dropping it stops everything behind it, and so does
/// [`SessionHandle::stop`].
///
/// What it follows can change while it runs, through its
/// [handle](Session::handle).
#[derive(Debug)]
pub struct Session {
    records: mpsc::Receiver<Record>,
    task: JoinHandle<()>,
    handle: SessionHandle,
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
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let driver = Driver::new(cluster.clone(), targets, filter, options, tx, commands_rx);
        let handle = SessionHandle {
            commands: commands_tx,
            state: driver.state.subscribe(),
        };
        let task = tokio::spawn(driver.run());
        Session {
            records,
            task,
            handle,
        }
    }

    /// The handle of the session: what changes it while it runs. It can be
    /// cloned and used from another task.
    pub fn handle(&self) -> SessionHandle {
        self.handle.clone()
    }

    /// The next record of the stream. None once the session was stopped.
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

/// How many pods a target of a running session follows right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetStatus {
    pub target: Target,
    /// The pods the target has followed containers in.
    pub pods: usize,
}

/// What a session follows right now, as its handle reads it.
#[derive(Debug, Clone, PartialEq)]
struct SessionState {
    spec: SessionSpec,
    targets: Vec<TargetStatus>,
}

/// What a running session can be told.
#[derive(Debug)]
enum Command {
    SetFilter(Option<Filter>),
    AddTarget(Target),
    RemoveTarget(Target),
    SetEvents(bool),
    SetContainer(Option<Pattern>),
    Load(Box<SessionSpec>),
    Tap(Option<mpsc::UnboundedSender<Record>>),
    Stop,
}

/// Changes a running session: its filter, its targets, its events and its
/// container glob. Cloneable, and usable from another task.
///
/// Every change is applied by the session's own task, in the order the
/// changes were asked for, and answered with a `notice` record in the
/// stream, so whoever reads the stream sees what changed. A change of the
/// filter is immediate. A change of what is followed goes through the
/// resolver and may take a moment against the cluster; its notice is
/// written when it is done, not when it is asked.
///
/// A session that has stopped takes no more changes; they are dropped.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    commands: mpsc::UnboundedSender<Command>,
    state: watch::Receiver<SessionState>,
}

impl SessionHandle {
    /// Replaces the filter, or removes it with `None`. It takes effect on
    /// the next record.
    pub fn set_filter(&self, filter: Option<Filter>) {
        self.send(Command::SetFilter(filter));
    }

    /// Follows one more target: resolves it and opens the streams of its
    /// containers, each with the start-up history of the session. A target
    /// that is followed already is left as it is.
    pub fn add_target(&self, target: Target) {
        self.send(Command::AddTarget(target));
    }

    /// Stops following a target: closes the streams of the pods no other
    /// target still matches. Lines already received are still emitted.
    pub fn remove_target(&self, target: Target) {
        self.send(Command::RemoveTarget(target));
    }

    /// Starts or stops merging the cluster's events into the stream.
    pub fn set_events(&self, events: bool) {
        self.send(Command::SetEvents(events));
    }

    /// Applies another container glob to every followed pod, or none.
    pub fn set_container(&self, container: Option<Pattern>) {
        self.send(Command::SetContainer(container));
    }

    /// Replaces targets, filter, container glob, events switch and history
    /// in one step. What the spec leaves out is the built-in default, as
    /// for a session that is started from it; streams that are followed
    /// before and after stay open. The fields, the output and the columns
    /// of the spec are not the session's and are ignored here, and so is
    /// its context, except that a spec for another cluster is refused.
    pub fn load(&self, spec: SessionSpec) {
        self.send(Command::Load(Box::new(spec)));
    }

    /// Hands every record of the session to a tap as well, before the
    /// filter: what passed the resolver, whether or not it is shown. This
    /// is what a recording is made from. `None` takes the tap away; a tap
    /// whose receiver is gone is forgotten.
    ///
    /// The tap is no change of what is followed or shown, and the one call
    /// that is not answered with a notice: whoever taps says so itself.
    pub fn set_tap(&self, tap: Option<mpsc::UnboundedSender<Record>>) {
        self.send(Command::Tap(tap));
    }

    /// What the session follows right now, as a template would hold it:
    /// the context, the targets, the filter, the container glob, the events
    /// switch, the history and the format. The fields, the output and the
    /// columns are left out; they belong to whoever renders the records.
    pub fn spec(&self) -> SessionSpec {
        self.state.borrow().spec.clone()
    }

    /// The targets of the session, each with the number of pods it follows
    /// right now.
    pub fn targets(&self) -> Vec<TargetStatus> {
        self.state.borrow().targets.clone()
    }

    /// Stops the session: closes every stream and every watch, and ends
    /// the stream of records. What dropping the [`Session`] does.
    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    /// Whether the session has stopped and takes no more changes.
    pub fn is_stopped(&self) -> bool {
        self.commands.is_closed()
    }

    fn send(&self, command: Command) {
        // A session that has stopped has nothing left to change.
        let _ = self.commands.send(command);
    }
}

/// A followed container, as the session holds on to it.
struct Followed {
    /// The state of the container, as its follower sees it. Dropping it
    /// lets the follower read what is left and finish.
    status: watch::Sender<ContainerView>,
    /// Set before the state is dropped when the container is let go on
    /// purpose: its follower then stops at once.
    dismissed: Arc<AtomicBool>,
}

/// A resolver that took the place of another and has not yet listed what
/// there is: until it has, nobody can say which of the followed containers
/// are still wanted.
struct Change {
    /// The containers the new resolver has announced so far.
    kept: HashSet<ContainerId>,
    /// What to say once the change is done.
    answers: Vec<Answer>,
}

/// The notice that answers a change of what is followed.
enum Answer {
    /// About the targets or the containers: said with what is followed now.
    Following(String),
    /// Anything else, said as it is.
    Plain(String),
}

/// How many notices and events of earlier resolvers are remembered, to not
/// say them twice after a change.
const REMEMBERED: usize = 16_384;

/// A bounded memory of things that were said.
#[derive(Default)]
struct Memory {
    order: VecDeque<u64>,
    known: HashSet<u64>,
}

impl Memory {
    fn insert(&mut self, print: u64) {
        if self.known.insert(print) {
            self.order.push_back(print);
            if self.order.len() > REMEMBERED
                && let Some(oldest) = self.order.pop_front()
            {
                self.known.remove(&oldest);
            }
        }
    }

    fn contains(&self, print: u64) -> bool {
        self.known.contains(&print)
    }
}

fn print_of(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// What tells one cluster event from another, as far as a reader can see.
fn event_print(event: &ClusterEvent) -> u64 {
    print_of(&(
        &event.namespace,
        &event.pod,
        &event.container,
        &event.regarding,
        &event.reason,
        &event.message,
        event.warning,
        event.count,
        event.time,
    ))
}

/// What woke the task of a session.
enum Wake {
    Resolver(Option<ResolverEvent>),
    Record(Record),
    Command(Option<Box<Command>>),
    /// The hold of a record ran out, or a follower finished.
    Tick,
}

/// The one task that owns every stream of a session: it starts and stops
/// followers as the resolver says, passes their records through the reorder
/// buffer and the filter, and applies the changes its handles ask for.
struct Driver {
    cluster: Cluster,
    targets: Vec<Target>,
    filter: Option<Filter>,
    options: SessionOptions,
    out: mpsc::Sender<Record>,
    commands: mpsc::UnboundedReceiver<Command>,
    state: watch::Sender<SessionState>,

    resolver: Resolver,
    /// Where the followers send their records, and what new ones are given.
    records_tx: mpsc::Sender<Record>,
    records_rx: mpsc::Receiver<Record>,
    context: Arc<Followers>,
    followers: HashMap<ContainerId, Followed>,
    tasks: JoinSet<()>,
    buffer: ReorderBuffer,

    /// A change of what is followed that is under way.
    change: Option<Change>,
    /// Whether what is followed was ever changed. Until then the session
    /// does exactly what it did before it could be changed.
    changed: bool,
    /// The pods each target follows, by namespace and by the index of the
    /// target in the list the resolver was started with.
    matched: HashMap<String, BTreeMap<usize, usize>>,
    /// The notices about targets and watches that were said, and the events
    /// that were told, so that a resolver that takes over repeats neither.
    said: Memory,
    told: Memory,
    /// Where every record goes as well, before the filter.
    tap: Option<mpsc::UnboundedSender<Record>>,
}

impl Driver {
    fn new(
        cluster: Cluster,
        targets: Vec<Target>,
        filter: Option<Filter>,
        options: SessionOptions,
        out: mpsc::Sender<Record>,
        commands: mpsc::UnboundedReceiver<Command>,
    ) -> Driver {
        // Events that are there when the session starts are told as far back
        // as `since` reaches; with a history in lines, none of them is. Events
        // that happen while the session runs are always told.
        let events_since = options.events.then(|| match options.since {
            Some(since) => history_start(since),
            None => DateTime::<Utc>::MAX_UTC,
        });
        let resolver = Resolver::start(
            &cluster,
            targets.clone(),
            options.containers.clone(),
            events_since,
        );
        let (records_tx, records_rx) = mpsc::channel(1024);
        let context = followers_context(&cluster, &options, records_tx.clone());
        let buffer = ReorderBuffer::new(options.hold);
        let (state, _) = watch::channel(SessionState {
            spec: SessionSpec::default(),
            targets: Vec::new(),
        });
        let driver = Driver {
            cluster,
            targets,
            filter,
            options,
            out,
            commands,
            state,
            resolver,
            records_tx,
            records_rx,
            context,
            followers: HashMap::new(),
            tasks: JoinSet::new(),
            buffer,
            change: None,
            changed: false,
            matched: HashMap::new(),
            said: Memory::default(),
            told: Memory::default(),
            tap: None,
        };
        driver.publish();
        driver
    }

    async fn run(mut self) {
        loop {
            let deadline = self.buffer.next_deadline().map(Instant::from_std);
            let wake = tokio::select! {
                event = self.resolver.next() => Wake::Resolver(event),
                Some(record) = self.records_rx.recv() => Wake::Record(record),
                command = self.commands.recv() => Wake::Command(command.map(Box::new)),
                _ = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => Wake::Tick,
                Some(_) = self.tasks.join_next(), if !self.tasks.is_empty() => Wake::Tick,
            };
            match wake {
                Wake::Resolver(Some(event)) => self.on_resolver(event),
                Wake::Resolver(None) => return,
                Wake::Record(record) => self.buffer.push(record, Instant::now().into_std()),
                // Stopping is what dropping the session does: everything
                // this task owns goes with it.
                Wake::Command(None) => return,
                Wake::Command(Some(command)) => match *command {
                    Command::Stop => return,
                    command => self.apply(command),
                },
                Wake::Tick => {}
            }

            for record in self.buffer.pop_ready(Instant::now().into_std()) {
                if let Some(tap) = &self.tap
                    && tap.send(record.clone()).is_err()
                {
                    // Nobody listens any more.
                    self.tap = None;
                }
                let accepted = self
                    .filter
                    .as_ref()
                    .is_none_or(|filter| filter.accepts(&record));
                if accepted && self.out.send(record).await.is_err() {
                    return;
                }
            }
        }
    }

    fn on_resolver(&mut self, event: ResolverEvent) {
        match event {
            ResolverEvent::Container(ContainerEvent::Added { id, view, initial }) => {
                if let Some(change) = &mut self.change {
                    change.kept.insert(id.clone());
                }
                // After a change, a resolver that took over announces again
                // what is followed already: that stream stays as it is.
                if self.changed
                    && let Some(followed) = self.followers.get(&id)
                {
                    followed.status.send_replace(view);
                    return;
                }
                let (status, receiver) = watch::channel(view);
                let dismissed = Arc::new(AtomicBool::new(false));
                self.tasks.spawn(logs::follow(
                    self.context.clone(),
                    id.clone(),
                    initial,
                    receiver,
                    dismissed.clone(),
                ));
                self.followers.insert(id, Followed { status, dismissed });
            }
            ResolverEvent::Container(ContainerEvent::Changed { id, view }) => {
                if let Some(followed) = self.followers.get(&id) {
                    followed.status.send_replace(view);
                }
            }
            ResolverEvent::Container(ContainerEvent::Removed { id }) => {
                // Dropping the state lets the follower read what is left
                // and finish.
                self.followers.remove(&id);
                if let Some(change) = &mut self.change {
                    change.kept.remove(&id);
                }
            }
            ResolverEvent::Cluster(event) => {
                let print = event_print(&event);
                // A resolver that took over lists the events that are there
                // again; the ones that were told are not told twice.
                if self.change.is_some() && self.told.contains(print) {
                    return;
                }
                self.told.insert(print);
                let record = event_record(self.cluster.name(), event);
                self.buffer.push(record, Instant::now().into_std());
            }
            ResolverEvent::Notice(notice) => {
                // What is said about a target or a watch, as opposed to a
                // pod, is said again by a resolver that takes over, although
                // nothing about it changed.
                if notice.pod.is_none() {
                    let print = print_of(&notice.message);
                    if self.change.is_some() && self.said.contains(print) {
                        return;
                    }
                    self.said.insert(print);
                }
                let record = Record::notice(
                    self.cluster.name(),
                    notice.namespace,
                    notice.pod.unwrap_or_default(),
                    None,
                    crate::clock::now(),
                    notice.message,
                );
                self.buffer.push(record, Instant::now().into_std());
            }
            ResolverEvent::Matched { namespace, pods } => {
                if pods.is_empty() {
                    self.matched.remove(&namespace);
                } else {
                    self.matched.insert(namespace, pods);
                }
                self.publish();
            }
            ResolverEvent::Settled => self.finish_change(),
        }
    }

    /// Applies one change a handle asked for.
    fn apply(&mut self, command: Command) {
        match command {
            Command::SetFilter(filter) => {
                self.filter = filter;
                let message = match &self.filter {
                    Some(filter) => format!("filter: {filter}"),
                    None => "filter off".to_string(),
                };
                self.say(message);
                self.publish();
            }
            Command::AddTarget(target) => {
                if self.targets.contains(&target) {
                    self.say(format!("{target} is followed already"));
                    return;
                }
                let answer = Answer::Following(format!("added {target}"));
                self.targets.push(target);
                self.resolve_again(answer);
            }
            Command::RemoveTarget(target) => {
                let Some(index) = self.targets.iter().position(|known| *known == target) else {
                    self.say(format!("{target} is not followed; {}", self.following()));
                    return;
                };
                self.targets.remove(index);
                self.resolve_again(Answer::Following(format!("dropped {target}")));
            }
            Command::SetEvents(events) => {
                let word = if events { "on" } else { "off" };
                if self.options.events == events {
                    self.say(format!("events are {word} already"));
                    return;
                }
                self.options.events = events;
                self.resolve_again(Answer::Plain(format!("events {word}")));
            }
            Command::SetContainer(container) => {
                let message = match &container {
                    Some(pattern) => format!("containers matching {pattern}"),
                    None => "every container".to_string(),
                };
                if self.options.containers == container {
                    self.say(format!("{message}, as before"));
                    return;
                }
                self.options.containers = container;
                self.resolve_again(Answer::Following(message));
            }
            Command::Load(spec) => self.load(*spec),
            Command::Tap(tap) => self.tap = tap,
            // Taken care of by the loop.
            Command::Stop => {}
        }
    }

    fn load(&mut self, spec: SessionSpec) {
        if let Some(context) = &spec.context
            && context != self.cluster.name()
        {
            self.say(format!(
                "cannot load a session of the cluster {context}: this one follows the cluster {}; \
                 nothing changed",
                self.cluster.name()
            ));
            return;
        }
        let settings = spec.resolve();
        let followed_before = (
            self.targets.clone(),
            self.options.containers.clone(),
            self.options.events,
        );
        self.targets = settings.targets;
        self.filter = settings.filter;
        self.options.containers = settings.container;
        self.options.events = settings.events;
        self.options.since = settings.since;
        self.options.tail = settings.tail;
        self.options.format = settings.format;
        // The history and the formats are for the streams opened from here
        // on; the ones that stay open are not read again.
        self.context = followers_context(&self.cluster, &self.options, self.records_tx.clone());

        let answer = Answer::Following("session loaded".to_string());
        let followed_now = (&self.targets, &self.options.containers, self.options.events);
        if (&followed_before.0, &followed_before.1, followed_before.2) == followed_now {
            // Only the filter or the history changed: nothing to resolve.
            let message = self.answer_text(&answer);
            self.say(message);
            self.publish();
        } else {
            self.resolve_again(answer);
        }
    }

    /// Starts a resolver for what is followed now, in place of the one that
    /// ran. The containers it announces again keep their streams; the rest
    /// is let go once it has listed what there is.
    fn resolve_again(&mut self, answer: Answer) {
        // A resolver that takes over lists the events that are there: as
        // far back as the history reaches, as at the start, or else only
        // what happened around the change. What was told is left out.
        let events_since = self.options.events.then(|| match self.options.since {
            Some(since) => history_start(since),
            None => history_start(CHANGE_TOLERANCE),
        });
        self.resolver = Resolver::start(
            &self.cluster,
            self.targets.clone(),
            self.options.containers.clone(),
            events_since,
        );
        self.changed = true;
        self.matched.clear();
        match &mut self.change {
            Some(change) => {
                change.kept.clear();
                change.answers.push(answer);
            }
            None => {
                self.change = Some(Change {
                    kept: HashSet::new(),
                    answers: vec![answer],
                });
            }
        }
        self.publish();
    }

    /// The resolver that took over has listed what there is: lets go of
    /// what it did not announce again, and answers the change.
    fn finish_change(&mut self) {
        let Some(change) = self.change.take() else {
            return;
        };
        let pod_of = |id: &ContainerId| (id.namespace.clone(), id.pod.clone(), id.pod_uid.clone());
        let before: BTreeSet<_> = self.followers.keys().map(pod_of).collect();
        // These are let go on purpose: their streams close at once, and
        // what was received of them is still emitted.
        self.followers.retain(|id, followed| {
            let kept = change.kept.contains(id);
            if !kept {
                followed.dismissed.store(true, Ordering::Relaxed);
            }
            kept
        });
        let after: BTreeSet<_> = self.followers.keys().map(pod_of).collect();
        for (namespace, pod, _) in before.difference(&after) {
            let record = Record::notice(
                self.cluster.name(),
                namespace,
                pod,
                None,
                crate::clock::now(),
                format!("pod {pod} is no longer selected by a target"),
            );
            self.buffer.push(record, Instant::now().into_std());
        }
        for answer in &change.answers {
            let message = self.answer_text(answer);
            self.say(message);
        }
        self.publish();
    }

    fn answer_text(&self, answer: &Answer) -> String {
        match answer {
            Answer::Following(what) => format!("{what}; {}", self.following()),
            Answer::Plain(message) => message.clone(),
        }
    }

    /// What is followed, in words: every target with its pods.
    fn following(&self) -> String {
        if self.targets.is_empty() {
            return "following nothing".to_string();
        }
        let targets: Vec<String> = self
            .target_status()
            .iter()
            .map(|status| match status.pods {
                1 => format!("{} (1 pod)", status.target),
                pods => format!("{} ({pods} pods)", status.target),
            })
            .collect();
        format!("following {}", targets.join(", "))
    }

    fn target_status(&self) -> Vec<TargetStatus> {
        self.targets
            .iter()
            .enumerate()
            .map(|(index, target)| TargetStatus {
                target: target.clone(),
                pods: self
                    .matched
                    .values()
                    .filter_map(|pods| pods.get(&index))
                    .sum(),
            })
            .collect()
    }

    /// A notice of the session itself, in the stream like any other.
    fn say(&mut self, message: String) {
        let record = Record::notice(
            self.cluster.name(),
            "",
            "",
            None,
            crate::clock::now(),
            message,
        );
        self.buffer.push(record, Instant::now().into_std());
    }

    /// Lets the handles read what is followed now.
    fn publish(&self) {
        let format =
            (self.options.format != FormatSetting::default()).then(|| self.options.format.clone());
        let spec = SessionSpec {
            context: Some(self.cluster.name().to_string()),
            targets: self.targets.clone(),
            container: self.options.containers.clone(),
            since: self.options.since,
            tail: self.options.tail,
            events: Some(self.options.events),
            format,
            filter: self.filter.clone(),
            ..SessionSpec::default()
        };
        self.state.send_replace(SessionState {
            spec,
            targets: self.target_status(),
        });
    }
}

/// How far around a change of what is followed the events that are there
/// are told when the history is counted in lines: the watches are down for
/// a moment, and the clocks of this machine and the cluster differ.
const CHANGE_TOLERANCE: Duration = Duration::from_secs(5);

/// The time a history that reaches this far back starts at. Many events
/// carry their time in whole seconds, so it is a whole second too.
fn history_start(back: Duration) -> DateTime<Utc> {
    let start = crate::clock::now() - TimeDelta::from_std(back).unwrap_or_default();
    DateTime::from_timestamp(start.timestamp(), 0).unwrap_or(start)
}

/// What the followers started from here on share.
fn followers_context(
    cluster: &Cluster,
    options: &SessionOptions,
    records: mpsc::Sender<Record>,
) -> Arc<Followers> {
    Arc::new(Followers {
        client: cluster.client(),
        cluster: cluster.name().to_string(),
        history: History {
            since: options.since,
            tail: options.tail,
        },
        formats: options.format.clone(),
        idle: options.idle,
        records,
    })
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
#[cfg(test)]
mod handle_tests;
