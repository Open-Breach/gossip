//! Target resolution: from targets to the containers to follow, kept up to
//! date while the session runs.
//!
//! The resolver watches pods in every namespace a target names. A target
//! that takes every pod of a namespace gets one watch on all its pods. A
//! target that names a workload gets a watch on the workload kinds its name
//! is looked up in, and one pod watch per workload found, filtered by the
//! labels of that workload. Every report of a watch is matched against the
//! targets again: a pod that newly matches is announced, a pod that is gone
//! is taken back.
//!
//! The resolver only reads. Its logic lives in [`state`] and needs no
//! cluster; this module runs the watches and passes on what they report.

mod model;
mod state;

use std::collections::{BTreeSet, HashMap};
use std::fmt::Debug;
use std::pin::{Pin, pin};
use std::task::{Context, Poll};

use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{Namespace, Pod};
use k8s_openapi::api::events::v1::Event as KubeEvent;
use kube::api::Api;
use kube::runtime::WatchStreamExt;
use kube::runtime::watcher::{self, Event, watcher};
use kube::{Client, Resource};
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinHandle, JoinSet};

pub use self::model::{
    ContainerRole, ContainerState, ContainerView, EventView, Exit, OwnerKind, OwnerRef, PodView,
    WorkloadView,
};
pub use self::state::{ClusterEvent, ContainerEvent, ContainerId, EXITED};
use self::state::{Input, NamespaceState, Output, WatchSpec};
use crate::cluster::Cluster;
use crate::error::describe;
use crate::target::{Pattern, Target};

/// Something the resolver wants to tell the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// The namespace it is about, or the namespace pattern of a target.
    pub namespace: String,
    /// The pod it is about, if it is about one.
    pub pod: Option<String>,
    pub message: String,
}

/// What the resolver reports.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolverEvent {
    /// A container to follow, a new state of one, or one that is gone.
    Container(ContainerEvent),
    /// A cluster event about a followed pod or a workload above it.
    Cluster(ClusterEvent),
    Notice(Notice),
}

/// A running resolution of a set of targets: a stream of
/// [`ResolverEvent`]s. Dropping it stops every watch.
#[derive(Debug)]
pub struct Resolver {
    events: mpsc::Receiver<ResolverEvent>,
    task: JoinHandle<()>,
}

impl Resolver {
    /// Starts resolving the targets against a cluster. `containers` keeps
    /// only the containers whose name matches. With `events_since`, the
    /// cluster events about what is followed are reported too, from that
    /// time on. Must be called inside a tokio runtime.
    pub fn start(
        cluster: &Cluster,
        targets: Vec<Target>,
        containers: Option<Pattern>,
        events_since: Option<DateTime<Utc>>,
    ) -> Resolver {
        let (tx, events) = mpsc::channel(256);
        let task = tokio::spawn(run(cluster.client(), targets, containers, events_since, tx));
        Resolver { events, task }
    }

    /// The next thing the resolver reports. It never ends by itself.
    pub async fn next(&mut self) -> Option<ResolverEvent> {
        self.events.recv().await
    }
}

impl Stream for Resolver {
    type Item = ResolverEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_recv(cx)
    }
}

impl Drop for Resolver {
    fn drop(&mut self) {
        // The task owns every namespace task, and those own their watches.
        self.task.abort();
    }
}

/// What the tasks below the resolver tell it.
enum Control {
    /// A namespace has its first full picture.
    Settled {
        namespace: String,
        matched: BTreeSet<usize>,
    },
    Namespaces(Event<String>),
    NamespacesFailed(String),
}

/// Works out, once, which targets match nothing at start. That can only be
/// said when every namespace the targets name has its first full picture.
#[derive(Debug)]
struct NothingTracker {
    targets: usize,
    /// A target with a glob for its namespace needs the list of namespaces.
    waiting_for_namespaces: bool,
    /// The namespaces that are watched but have no full picture yet.
    pending: BTreeSet<String>,
    matched: BTreeSet<usize>,
    reported: bool,
}

impl NothingTracker {
    fn new(targets: usize, waiting_for_namespaces: bool) -> Self {
        NothingTracker {
            targets,
            waiting_for_namespaces,
            pending: BTreeSet::new(),
            matched: BTreeSet::new(),
            reported: false,
        }
    }

    fn started(&mut self, namespace: &str) {
        if !self.reported {
            self.pending.insert(namespace.to_string());
        }
    }

    fn settled(&mut self, namespace: &str, matched: BTreeSet<usize>) {
        self.pending.remove(namespace);
        self.matched.extend(matched);
    }

    fn namespaces_listed(&mut self) {
        self.waiting_for_namespaces = false;
    }

    /// The targets that match nothing, the one time this can first be said.
    fn report(&mut self) -> Vec<usize> {
        if self.reported || self.waiting_for_namespaces || !self.pending.is_empty() {
            return Vec::new();
        }
        self.reported = true;
        (0..self.targets)
            .filter(|index| !self.matched.contains(index))
            .collect()
    }
}

async fn run(
    client: Client,
    targets: Vec<Target>,
    containers: Option<Pattern>,
    events_since: Option<DateTime<Utc>>,
    tx: mpsc::Sender<ResolverEvent>,
) {
    let (control_tx, mut control_rx) = mpsc::channel::<Control>(64);
    let mut tasks = JoinSet::new();
    // The namespaces that are watched. Those found through a glob can go
    // away again; those a target names outright are watched for good.
    let mut found: HashMap<String, AbortHandle> = HashMap::new();
    let mut named: BTreeSet<String> = BTreeSet::new();

    let has_globs = targets
        .iter()
        .any(|target| target.namespace.literal().is_none());
    let mut nothing = NothingTracker::new(targets.len(), has_globs);
    let mut namespaces_listed = false;
    let mut last_failure: Option<String> = None;

    let start = |tasks: &mut JoinSet<()>, namespace: &str, initial: bool| {
        let matching: Vec<(usize, Target)> = targets
            .iter()
            .enumerate()
            .filter(|(_, target)| target.namespace.matches(namespace))
            .map(|(index, target)| (index, target.clone()))
            .collect();
        let state = NamespaceState::new(
            namespace,
            matching,
            containers.clone(),
            initial,
            events_since,
        );
        tasks.spawn(run_namespace(
            client.clone(),
            namespace.to_string(),
            state,
            tx.clone(),
            control_tx.clone(),
        ))
    };

    for target in &targets {
        if let Some(namespace) = target.namespace.literal()
            && named.insert(namespace.to_string())
        {
            nothing.started(namespace);
            start(&mut tasks, namespace, true);
        }
    }
    if has_globs {
        tasks.spawn(watch_namespaces(client.clone(), control_tx.clone()));
    }

    loop {
        for index in nothing.report() {
            let notice = Notice {
                namespace: targets[index].namespace.to_string(),
                pod: None,
                message: format!(
                    "target {} matches nothing yet; still watching",
                    targets[index]
                ),
            };
            if tx.send(ResolverEvent::Notice(notice)).await.is_err() {
                return;
            }
        }

        let Some(control) = control_rx.recv().await else {
            return;
        };
        match control {
            Control::Settled { namespace, matched } => nothing.settled(&namespace, matched),
            Control::Namespaces(Event::Init) => {}
            Control::Namespaces(Event::InitApply(namespace) | Event::Apply(namespace)) => {
                last_failure = None;
                let wanted = targets.iter().any(|target| {
                    target.namespace.literal().is_none() && target.namespace.matches(&namespace)
                });
                if wanted && !named.contains(&namespace) && !found.contains_key(&namespace) {
                    nothing.started(&namespace);
                    // A namespace made while the session runs has nothing in
                    // it that was there at the start.
                    let handle = start(&mut tasks, &namespace, !namespaces_listed);
                    found.insert(namespace, handle);
                }
            }
            Control::Namespaces(Event::InitDone) => {
                namespaces_listed = true;
                nothing.namespaces_listed();
            }
            Control::Namespaces(Event::Delete(namespace)) => {
                // Its pods were deleted before it, and reported as gone.
                if let Some(handle) = found.remove(&namespace) {
                    handle.abort();
                    nothing.settled(&namespace, BTreeSet::new());
                }
            }
            Control::NamespacesFailed(reason) => {
                // Do not hold the report up for a list that does not come.
                nothing.namespaces_listed();
                if last_failure.as_ref() != Some(&reason) {
                    let notice = Notice {
                        namespace: String::new(),
                        pod: None,
                        message: format!("cannot watch the namespaces: {reason}; trying again"),
                    };
                    if tx.send(ResolverEvent::Notice(notice)).await.is_err() {
                        return;
                    }
                    last_failure = Some(reason);
                }
            }
        }
        while tasks.try_join_next().is_some() {}
    }
}

/// Watches the namespaces of the cluster, for targets whose namespace is a
/// glob.
async fn watch_namespaces(client: Client, control: mpsc::Sender<Control>) {
    let api: Api<Namespace> = Api::all(client);
    let mut events = pin!(watcher(api, watcher::Config::default()).default_backoff());
    while let Some(item) = events.next().await {
        let message = match item {
            Ok(event) => Control::Namespaces(convert(event, |namespace: &Namespace| {
                namespace.metadata.name.clone().unwrap_or_default()
            })),
            Err(error) => Control::NamespacesFailed(describe_watch_error(&error)),
        };
        if control.send(message).await.is_err() {
            return;
        }
    }
}

/// Runs the watches of one namespace and passes on what follows from them.
async fn run_namespace(
    client: Client,
    namespace: String,
    (mut state, mut outputs): (NamespaceState, Vec<Output>),
    tx: mpsc::Sender<ResolverEvent>,
    control: mpsc::Sender<Control>,
) {
    let (input_tx, mut input_rx) = mpsc::channel::<Input>(256);
    let mut tasks = JoinSet::new();
    let mut watches: HashMap<WatchSpec, AbortHandle> = HashMap::new();

    loop {
        for output in outputs.drain(..) {
            let event = match output {
                Output::Start(spec) => {
                    let handle =
                        spawn_watch(&mut tasks, &client, &namespace, &spec, input_tx.clone());
                    watches.insert(spec, handle);
                    continue;
                }
                Output::Stop(spec) => {
                    if let Some(handle) = watches.remove(&spec) {
                        handle.abort();
                    }
                    while tasks.try_join_next().is_some() {}
                    continue;
                }
                Output::Settled { matched } => {
                    let settled = Control::Settled {
                        namespace: namespace.clone(),
                        matched,
                    };
                    if control.send(settled).await.is_err() {
                        return;
                    }
                    continue;
                }
                Output::Container(event) => ResolverEvent::Container(event),
                Output::Cluster(event) => ResolverEvent::Cluster(event),
                Output::Notice { pod, message } => ResolverEvent::Notice(Notice {
                    namespace: namespace.clone(),
                    pod,
                    message,
                }),
            };
            if tx.send(event).await.is_err() {
                return;
            }
        }
        let Some(input) = input_rx.recv().await else {
            return;
        };
        outputs = state.handle(input);
    }
}

fn spawn_watch(
    tasks: &mut JoinSet<()>,
    client: &Client,
    namespace: &str,
    spec: &WatchSpec,
    tx: mpsc::Sender<Input>,
) -> AbortHandle {
    fn api<K>(client: &Client, namespace: &str) -> Api<K>
    where
        K: Resource<Scope = k8s_openapi::NamespaceResourceScope>,
        K::DynamicType: Default,
    {
        Api::namespaced(client.clone(), namespace)
    }

    let spec = spec.clone();
    match &spec {
        WatchSpec::Pods { selector } => {
            let config = match selector {
                Some(selector) => watcher::Config::default().labels(selector),
                None => watcher::Config::default(),
            };
            tasks.spawn(watch::<Pod, PodView>(
                api(client, namespace),
                config,
                spec,
                Input::Pod,
                tx,
            ))
        }
        WatchSpec::Events => tasks.spawn(watch::<KubeEvent, EventView>(
            api(client, namespace),
            watcher::Config::default(),
            spec,
            Input::Cluster,
            tx,
        )),
        WatchSpec::Owners { kind, name } => {
            let config = match name {
                Some(name) => watcher::Config::default().fields(&format!("metadata.name={name}")),
                None => watcher::Config::default(),
            };
            let wrap = Input::Owner;
            match kind {
                OwnerKind::Deployment => tasks.spawn(watch::<Deployment, _>(
                    api(client, namespace),
                    config,
                    spec,
                    wrap,
                    tx,
                )),
                OwnerKind::ReplicaSet => tasks.spawn(watch::<ReplicaSet, _>(
                    api(client, namespace),
                    config,
                    spec,
                    wrap,
                    tx,
                )),
                OwnerKind::StatefulSet => tasks.spawn(watch::<StatefulSet, _>(
                    api(client, namespace),
                    config,
                    spec,
                    wrap,
                    tx,
                )),
                OwnerKind::DaemonSet => tasks.spawn(watch::<DaemonSet, _>(
                    api(client, namespace),
                    config,
                    spec,
                    wrap,
                    tx,
                )),
                OwnerKind::Job => tasks.spawn(watch::<Job, _>(
                    api(client, namespace),
                    config,
                    spec,
                    wrap,
                    tx,
                )),
                OwnerKind::CronJob => tasks.spawn(watch::<CronJob, _>(
                    api(client, namespace),
                    config,
                    spec,
                    wrap,
                    tx,
                )),
            }
        }
    }
}

/// One watch: lists, then follows the changes, forever. `kube` retries with
/// its default backoff when the watch breaks; every failure is passed on.
async fn watch<K, V>(
    api: Api<K>,
    config: watcher::Config,
    spec: WatchSpec,
    wrap: fn(WatchSpec, Event<V>) -> Input,
    tx: mpsc::Sender<Input>,
) where
    K: Resource + Clone + DeserializeOwned + Debug + Send + 'static,
    V: for<'a> From<&'a K>,
{
    let mut events = pin!(watcher(api, config).default_backoff());
    while let Some(item) = events.next().await {
        let input = match item {
            Ok(event) => wrap(spec.clone(), convert(event, |object: &K| V::from(object))),
            Err(error) => Input::Failed(spec.clone(), describe_watch_error(&error)),
        };
        if tx.send(input).await.is_err() {
            return;
        }
    }
}

/// Replaces the object of a watch event by a view of it.
fn convert<K, V>(event: Event<K>, view: impl Fn(&K) -> V) -> Event<V> {
    match event {
        Event::Apply(object) => Event::Apply(view(&object)),
        Event::Delete(object) => Event::Delete(view(&object)),
        Event::Init => Event::Init,
        Event::InitApply(object) => Event::InitApply(view(&object)),
        Event::InitDone => Event::InitDone,
    }
}

pub(crate) fn describe_watch_error(error: &watcher::Error) -> String {
    match error {
        watcher::Error::InitialListFailed(error)
        | watcher::Error::WatchStartFailed(error)
        | watcher::Error::WatchFailed(error) => describe(error),
        watcher::Error::WatchError(status) => format!("{} ({})", status.message, status.code),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod cluster_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_reported_once_every_namespace_has_its_picture() {
        let mut nothing = NothingTracker::new(3, false);
        nothing.started("flowcpt");
        nothing.started("monitoring");
        assert_eq!(nothing.report(), Vec::<usize>::new());

        nothing.settled("flowcpt", BTreeSet::from([0]));
        assert_eq!(nothing.report(), Vec::<usize>::new());
        nothing.settled("monitoring", BTreeSet::new());
        assert_eq!(nothing.report(), [1, 2]);

        // It is said once, also when more namespaces come later.
        nothing.started("flowcpt-stg");
        nothing.settled("flowcpt-stg", BTreeSet::new());
        assert_eq!(nothing.report(), Vec::<usize>::new());
    }

    #[test]
    fn a_namespace_glob_waits_for_the_list_of_namespaces() {
        let mut nothing = NothingTracker::new(1, true);
        // No namespace is known yet, which is not the same as none matching.
        assert_eq!(nothing.report(), Vec::<usize>::new());
        nothing.started("flowcpt");
        nothing.namespaces_listed();
        assert_eq!(nothing.report(), Vec::<usize>::new());
        nothing.settled("flowcpt", BTreeSet::from([0]));
        assert_eq!(nothing.report(), Vec::<usize>::new());
    }

    #[test]
    fn a_namespace_glob_that_matches_no_namespace_matches_nothing() {
        let mut nothing = NothingTracker::new(1, true);
        nothing.namespaces_listed();
        assert_eq!(nothing.report(), [0]);
    }

    #[test]
    fn a_watch_event_keeps_its_kind_when_its_object_is_replaced() {
        let double = |number: &i32| number * 2;
        assert!(matches!(convert(Event::Apply(2), double), Event::Apply(4)));
        assert!(matches!(
            convert(Event::Delete(2), double),
            Event::Delete(4)
        ));
        assert!(matches!(
            convert(Event::InitApply(2), double),
            Event::InitApply(4)
        ));
        assert!(matches!(convert(Event::<i32>::Init, double), Event::Init));
        assert!(matches!(
            convert(Event::<i32>::InitDone, double),
            Event::InitDone
        ));
    }
}
