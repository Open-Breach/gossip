//! What the resolver knows about one namespace, and what follows from it.
//!
//! This is the whole logic of target resolution, with no cluster in it: it
//! takes what the watches report and answers with the watches to start or
//! stop and the containers to follow or let go. The module above it runs
//! the watches.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use kube::core::SelectorExt;
use kube::runtime::watcher::Event;

use super::model::{ContainerView, OwnerKind, PodView, WorkloadView};
use crate::target::{Pattern, Target, WorkloadKind, WorkloadSelector};

/// One watch the resolver runs in a namespace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WatchSpec {
    /// The objects of one kind, or only the one with this name.
    Owners {
        kind: OwnerKind,
        name: Option<String>,
    },
    /// The pods, all of them or those that carry these labels.
    Pods { selector: Option<String> },
}

impl WatchSpec {
    /// What the watch watches, in words, for a notice.
    fn describe(&self) -> String {
        match self {
            WatchSpec::Owners { kind, name: None } => format!("{} objects", kind.kind_name()),
            WatchSpec::Owners {
                kind,
                name: Some(name),
            } => format!("the {} `{name}`", kind.kind_name()),
            WatchSpec::Pods { selector: None } => "pods".to_string(),
            WatchSpec::Pods {
                selector: Some(selector),
            } => format!("pods with the labels `{selector}`"),
        }
    }
}

/// What a watch reported.
#[derive(Debug, Clone)]
pub enum Input {
    Owner(WatchSpec, Event<WorkloadView>),
    Pod(WatchSpec, Event<PodView>),
    /// The watch failed and is being retried.
    Failed(WatchSpec, String),
}

/// What follows from what a watch reported.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Start(WatchSpec),
    Stop(WatchSpec),
    Container(ContainerEvent),
    Notice {
        pod: Option<String>,
        message: String,
    },
    /// Sent once, when every watch has listed what there is: the first full
    /// picture of the namespace. `matched` holds the targets, by their index,
    /// that select at least one container in it.
    Settled {
        matched: BTreeSet<usize>,
    },
}

/// One container of one pod.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContainerId {
    pub namespace: String,
    pub pod: String,
    /// The uid of the pod: a pod that is deleted and made again under the
    /// same name is another pod.
    pub pod_uid: String,
    pub container: String,
}

/// A change in the set of containers to follow.
#[derive(Debug, Clone, PartialEq)]
pub enum ContainerEvent {
    /// A container to follow from now on.
    Added {
        id: ContainerId,
        view: ContainerView,
        /// Whether the container was there when the session started, as
        /// opposed to one that came up while it ran.
        initial: bool,
    },
    /// The pod reports another state or restart count for the container.
    Changed {
        id: ContainerId,
        view: ContainerView,
    },
    /// The container is gone, or no target selects it any more.
    Removed { id: ContainerId },
}

/// How far a watch has come.
#[derive(Debug, Default)]
struct Progress {
    /// It has listed what there is, or failed trying; either way the
    /// namespace does not wait for it any longer.
    live: bool,
    /// It has listed what there is at least once.
    listed: bool,
    /// For a pod watch: it was started while the namespace was first being
    /// looked at, so what its first list holds was there at the start.
    at_start: bool,
    /// The keys seen so far while it lists.
    listing: Option<HashSet<String>>,
}

#[derive(Debug)]
struct PodEntry {
    view: PodView,
    /// The watches that report this pod.
    seen_by: BTreeSet<WatchSpec>,
    /// The containers followed right now, as last reported.
    followed: BTreeMap<String, ContainerView>,
    /// The pod was there when the session started: it came with the first
    /// list of a watch that was started at the start.
    at_start: bool,
    /// The containers it had when it was first seen. One that is added to
    /// a pod later is new, also in a pod that was there at the start.
    first_containers: BTreeSet<String>,
}

/// The resolver's state for one namespace.
#[derive(Debug)]
pub struct NamespaceState {
    namespace: String,
    /// The targets that name this namespace, with their index in the session.
    targets: Vec<(usize, Target)>,
    /// Only containers whose name matches are followed.
    containers: Option<Pattern>,
    /// Whether what the first lists hold counts as having been there when
    /// the session started.
    initial_allowed: bool,
    owner_watches: BTreeMap<WatchSpec, Progress>,
    pod_watches: BTreeMap<WatchSpec, Progress>,
    owners: BTreeMap<(OwnerKind, String), WorkloadView>,
    pods: HashMap<String, PodEntry>,
    settled: bool,
    /// Every owner watch has listed what there is, so whatever pod watch is
    /// started from now on is about something that came later.
    started: bool,
    /// The last failure of each watch, to say it once and not on every retry.
    failures: HashMap<WatchSpec, String>,
    /// The kinds a target's name was found in, when a notice said so.
    ambiguous: BTreeMap<usize, BTreeSet<WorkloadKind>>,
}

impl NamespaceState {
    /// The state of a namespace before anything is known about it, and the
    /// watches to start.
    pub fn new(
        namespace: &str,
        targets: Vec<(usize, Target)>,
        containers: Option<Pattern>,
        initial_allowed: bool,
    ) -> (Self, Vec<Output>) {
        // Which kinds to watch, and whether one name is enough.
        let mut wanted: BTreeMap<WorkloadKind, Option<String>> = BTreeMap::new();
        let mut want = |kind: WorkloadKind, name: Option<&str>| {
            wanted
                .entry(kind)
                .and_modify(|known| {
                    if known.as_deref() != name {
                        *known = None;
                    }
                })
                .or_insert(name.map(str::to_string));
        };
        for (_, target) in &targets {
            if let WorkloadSelector::Named { kind, name } = &target.workload {
                match kind {
                    Some(kind) => want(*kind, name.literal()),
                    None => WorkloadKind::ALL
                        .into_iter()
                        .for_each(|kind| want(kind, name.literal())),
                }
            }
        }
        // A cron job is followed through its jobs, whatever they are called.
        if wanted.contains_key(&WorkloadKind::CronJob) {
            wanted.insert(WorkloadKind::Job, None);
        }

        let mut owner_watches = BTreeMap::new();
        for (kind, name) in wanted {
            // A deployment is followed through its replica sets.
            if kind == WorkloadKind::Deployment {
                let sets = WatchSpec::Owners {
                    kind: OwnerKind::ReplicaSet,
                    name: None,
                };
                owner_watches.insert(sets, Progress::default());
            }
            let spec = WatchSpec::Owners {
                kind: kind.into(),
                name,
            };
            owner_watches.insert(spec, Progress::default());
        }

        let mut state = NamespaceState {
            namespace: namespace.to_string(),
            targets,
            containers,
            initial_allowed,
            owner_watches,
            pod_watches: BTreeMap::new(),
            owners: BTreeMap::new(),
            pods: HashMap::new(),
            settled: false,
            started: false,
            failures: HashMap::new(),
            ambiguous: BTreeMap::new(),
        };
        let mut out: Vec<Output> = state
            .owner_watches
            .keys()
            .cloned()
            .map(Output::Start)
            .collect();
        state.reconcile(&mut out);
        (state, out)
    }

    /// Takes what a watch reported and says what follows from it.
    pub fn handle(&mut self, input: Input) -> Vec<Output> {
        let mut out = Vec::new();
        match input {
            Input::Owner(spec, event) => {
                if !self.owner_watches.contains_key(&spec) {
                    return out;
                }
                self.failures.remove(&spec);
                self.on_owner(&spec, event);
            }
            Input::Pod(spec, event) => {
                // A watch that was stopped may still have something under way.
                if !self.pod_watches.contains_key(&spec) {
                    return out;
                }
                self.failures.remove(&spec);
                self.on_pod(&spec, event);
            }
            Input::Failed(spec, reason) => {
                let progress = match self.owner_watches.get_mut(&spec) {
                    Some(progress) => Some(progress),
                    None => self.pod_watches.get_mut(&spec),
                };
                let Some(progress) = progress else {
                    return out;
                };
                progress.live = true;
                if self.failures.get(&spec) != Some(&reason) {
                    out.push(Output::Notice {
                        pod: None,
                        message: format!(
                            "cannot watch {} in the namespace {}: {reason}; trying again",
                            spec.describe(),
                            self.namespace
                        ),
                    });
                    self.failures.insert(spec, reason);
                }
            }
        }
        self.reconcile(&mut out);
        out
    }

    fn on_owner(&mut self, spec: &WatchSpec, event: Event<WorkloadView>) {
        let WatchSpec::Owners { kind, .. } = spec else {
            return;
        };
        let kind = *kind;
        let Some(progress) = self.owner_watches.get_mut(spec) else {
            return;
        };
        match event {
            Event::Init => progress.listing = Some(HashSet::new()),
            Event::InitApply(owner) => {
                if let Some(listing) = &mut progress.listing {
                    listing.insert(owner.name.clone());
                }
                self.owners.insert((kind, owner.name.clone()), owner);
            }
            Event::InitDone => {
                if let Some(listed) = progress.listing.take() {
                    self.owners.retain(|(owner_kind, name), _| {
                        *owner_kind != kind || listed.contains(name)
                    });
                }
                progress.live = true;
                progress.listed = true;
            }
            Event::Apply(owner) => {
                self.owners.insert((kind, owner.name.clone()), owner);
            }
            Event::Delete(owner) => {
                self.owners.remove(&(kind, owner.name));
            }
        }
    }

    fn on_pod(&mut self, spec: &WatchSpec, event: Event<PodView>) {
        let Some(progress) = self.pod_watches.get_mut(spec) else {
            return;
        };
        // What the first list of a watch started at the start holds was
        // there when the session started. A list that failed and is tried
        // again is still the first.
        let first_list = progress.at_start && !progress.listed;
        let upsert = |pods: &mut HashMap<String, PodEntry>, pod: PodView, at_start: bool| {
            let entry = pods.entry(pod_key(&pod)).or_insert_with(|| PodEntry {
                first_containers: pod
                    .containers
                    .iter()
                    .map(|container| container.name.clone())
                    .collect(),
                view: pod.clone(),
                seen_by: BTreeSet::new(),
                followed: BTreeMap::new(),
                at_start,
            });
            entry.view = pod;
            entry.seen_by.insert(spec.clone());
        };
        match event {
            Event::Init => progress.listing = Some(HashSet::new()),
            Event::InitApply(pod) => {
                if let Some(listing) = &mut progress.listing {
                    listing.insert(pod_key(&pod));
                }
                upsert(&mut self.pods, pod, first_list);
            }
            Event::InitDone => {
                if let Some(listed) = progress.listing.take() {
                    for (key, entry) in &mut self.pods {
                        if !listed.contains(key) {
                            entry.seen_by.remove(spec);
                        }
                    }
                }
                progress.live = true;
                progress.listed = true;
            }
            Event::Apply(pod) => upsert(&mut self.pods, pod, false),
            Event::Delete(pod) => {
                if let Some(entry) = self.pods.get_mut(&pod_key(&pod)) {
                    entry.seen_by.remove(spec);
                }
            }
        }
    }

    /// Brings the watches and the followed containers in line with what is
    /// known now.
    fn reconcile(&mut self, out: &mut Vec<Output>) {
        self.reconcile_pod_watches(out);
        // The pod watches started up to here belong to the start; later
        // ones are for workloads that came while the session ran.
        if !self.started && self.owner_watches.values().all(|progress| progress.listed) {
            self.started = true;
        }
        self.reconcile_containers(out);
        self.note_ambiguous_names(out);

        let all_live = |watches: &BTreeMap<WatchSpec, Progress>| {
            watches.values().all(|progress| progress.live)
        };
        if !self.settled && all_live(&self.owner_watches) && all_live(&self.pod_watches) {
            self.settled = true;
            out.push(Output::Settled {
                matched: self.matched_targets(),
            });
        }
    }

    fn reconcile_pod_watches(&mut self, out: &mut Vec<Output>) {
        let desired = self.desired_pod_watches();
        let stopped: Vec<WatchSpec> = self
            .pod_watches
            .keys()
            .filter(|spec| !desired.contains(*spec))
            .cloned()
            .collect();
        for spec in stopped {
            self.pod_watches.remove(&spec);
            self.failures.remove(&spec);
            for entry in self.pods.values_mut() {
                entry.seen_by.remove(&spec);
            }
            out.push(Output::Stop(spec));
        }
        for spec in desired {
            if !self.pod_watches.contains_key(&spec) {
                let progress = Progress {
                    at_start: self.initial_allowed && !self.started,
                    ..Progress::default()
                };
                self.pod_watches.insert(spec.clone(), progress);
                out.push(Output::Start(spec));
            }
        }
    }

    /// The pod watches the targets need: one for all pods when a target
    /// takes every pod of the namespace, otherwise one for the labels of
    /// each workload a target names.
    fn desired_pod_watches(&self) -> BTreeSet<WatchSpec> {
        let mut desired = BTreeSet::new();
        let every_pod = self
            .targets
            .iter()
            .any(|(_, target)| target.workload == WorkloadSelector::Any);
        if every_pod {
            desired.insert(WatchSpec::Pods { selector: None });
            return desired;
        }
        for owner in self.owners.values() {
            if !self.is_followed_workload(owner) {
                continue;
            }
            if let Some(selector) = &owner.selector
                && !selector.selects_all()
            {
                desired.insert(WatchSpec::Pods {
                    selector: Some(selector.to_string()),
                });
            }
        }
        desired
    }

    /// Whether the pods of this object are followed: a target names it, or
    /// it is a job of a cron job that a target names.
    fn is_followed_workload(&self, owner: &WorkloadView) -> bool {
        if self.is_named(owner) {
            return true;
        }
        owner.kind == OwnerKind::Job
            && owner.controller.as_ref().is_some_and(|controller| {
                self.owners
                    .get(&(OwnerKind::CronJob, controller.name.clone()))
                    .is_some_and(|cron| cron.uid == controller.uid && self.is_named(cron))
            })
    }

    /// Whether a target names this workload.
    fn is_named(&self, owner: &WorkloadView) -> bool {
        owner.kind.workload().is_some_and(|kind| {
            self.targets.iter().any(|(_, target)| {
                target.workload != WorkloadSelector::Any
                    && target.workload.matches(kind, &owner.name)
            })
        })
    }

    fn reconcile_containers(&mut self, out: &mut Vec<Output>) {
        let mut keys: Vec<(String, String)> = self
            .pods
            .iter()
            .map(|(key, entry)| (entry.view.name.clone(), key.clone()))
            .collect();
        keys.sort();

        for (_, key) in keys {
            let entry = &self.pods[&key];
            let pod_at_start = entry.at_start;
            let gone = entry.seen_by.is_empty();
            let wanted = if gone {
                BTreeMap::new()
            } else {
                self.wanted_containers(&entry.view)
            };

            let entry = self.pods.get_mut(&key).expect("the key was just listed");
            let id = |container: &str| ContainerId {
                namespace: self.namespace.clone(),
                pod: entry.view.name.clone(),
                pod_uid: entry.view.uid.clone(),
                container: container.to_string(),
            };
            for name in entry.followed.keys() {
                if !wanted.contains_key(name) {
                    out.push(Output::Container(ContainerEvent::Removed { id: id(name) }));
                }
            }
            for (name, view) in &wanted {
                match entry.followed.get(name) {
                    None => out.push(Output::Container(ContainerEvent::Added {
                        id: id(name),
                        view: view.clone(),
                        initial: pod_at_start && entry.first_containers.contains(name),
                    })),
                    Some(known) if known != view => {
                        out.push(Output::Container(ContainerEvent::Changed {
                            id: id(name),
                            view: view.clone(),
                        }));
                    }
                    Some(_) => {}
                }
            }

            let was_followed = !entry.followed.is_empty();
            let is_followed = !wanted.is_empty();
            let pod = entry.view.name.clone();
            if !was_followed && is_followed && !pod_at_start {
                let names: Vec<&str> = wanted.keys().map(String::as_str).collect();
                out.push(Output::Notice {
                    message: format!("pod {pod} appeared; following {}", names.join(", ")),
                    pod: Some(pod),
                });
            } else if was_followed && !is_followed {
                let message = if gone {
                    format!("pod {pod} went away")
                } else {
                    format!("pod {pod} is no longer selected by a target")
                };
                out.push(Output::Notice {
                    message,
                    pod: Some(pod),
                });
            }

            entry.followed = wanted;
            if gone {
                self.pods.remove(&key);
            }
        }
    }

    /// The containers of a pod that the targets select.
    fn wanted_containers(&self, pod: &PodView) -> BTreeMap<String, ContainerView> {
        let mut wanted = BTreeMap::new();
        for (_, target) in &self.targets {
            if !self.selects_pod(target, pod) {
                continue;
            }
            for container in &pod.containers {
                if self.selects_container(target, &container.name) {
                    wanted.insert(container.name.clone(), container.clone());
                }
            }
        }
        wanted
    }

    fn selects_container(&self, target: &Target, container: &str) -> bool {
        target.container.matches(container)
            && self
                .containers
                .as_ref()
                .is_none_or(|pattern| pattern.matches(container))
    }

    fn selects_pod(&self, target: &Target, pod: &PodView) -> bool {
        if !target.pod.matches(&pod.name) {
            return false;
        }
        match &target.workload {
            WorkloadSelector::Any => true,
            named => self.owners.values().any(|owner| {
                owner
                    .kind
                    .workload()
                    .is_some_and(|kind| named.matches(kind, &owner.name))
                    && self.belongs(pod, owner)
            }),
        }
    }

    /// Whether a pod belongs to a workload. A deployment resolves through
    /// its replica sets and a cron job through its jobs; the others through
    /// their selector, to the pods that carry those labels.
    fn belongs(&self, pod: &PodView, workload: &WorkloadView) -> bool {
        let carries_labels = || {
            workload
                .selector
                .as_ref()
                .is_some_and(|selector| !selector.selects_all() && selector.matches(&pod.labels))
        };
        // The pod's controller is an object of `kind` that `workload` controls.
        let through = |kind: OwnerKind| {
            pod.controller.as_ref().is_some_and(|controller| {
                controller.kind == kind.kind_name()
                    && self
                        .owners
                        .get(&(kind, controller.name.clone()))
                        .is_some_and(|between| {
                            between.uid == controller.uid
                                && between
                                    .controller
                                    .as_ref()
                                    .is_some_and(|above| above.uid == workload.uid)
                        })
            })
        };
        match workload.kind {
            OwnerKind::Deployment => carries_labels() && through(OwnerKind::ReplicaSet),
            OwnerKind::CronJob => through(OwnerKind::Job),
            OwnerKind::StatefulSet | OwnerKind::DaemonSet | OwnerKind::Job => carries_labels(),
            OwnerKind::ReplicaSet => false,
        }
    }

    /// The targets that select at least one container right now.
    fn matched_targets(&self) -> BTreeSet<usize> {
        let mut matched = BTreeSet::new();
        for (index, target) in &self.targets {
            let selects_something = self.pods.values().any(|entry| {
                !entry.seen_by.is_empty()
                    && self.selects_pod(target, &entry.view)
                    && entry
                        .view
                        .containers
                        .iter()
                        .any(|container| self.selects_container(target, &container.name))
            });
            if selects_something {
                matched.insert(*index);
            }
        }
        matched
    }

    /// Says so when a plain name is the name of workloads of several kinds.
    /// Both are followed; a prefix picks one.
    fn note_ambiguous_names(&mut self, out: &mut Vec<Output>) {
        for (index, target) in &self.targets {
            let WorkloadSelector::Named { kind: None, name } = &target.workload else {
                continue;
            };
            let Some(name) = name.literal() else {
                continue;
            };
            let kinds: BTreeSet<WorkloadKind> = self
                .owners
                .values()
                .filter(|owner| owner.name == name)
                .filter_map(|owner| owner.kind.workload())
                .collect();
            if kinds.len() < 2 || self.ambiguous.get(index) == Some(&kinds) {
                continue;
            }
            let listed: Vec<String> = kinds
                .iter()
                .map(|kind| format!("a {}", kind.kind_name()))
                .collect();
            let first = kinds.first().expect("there are at least two kinds");
            out.push(Output::Notice {
                pod: None,
                message: format!(
                    "{name} in the namespace {namespace} is the name of {kinds}; all of them are \
                     followed. A prefix picks one, as in {namespace}/{prefix}:{name}",
                    namespace = self.namespace,
                    kinds = listed.join(" and "),
                    prefix = first.prefix(),
                ),
            });
            self.ambiguous.insert(*index, kinds);
        }
    }
}

fn pod_key(pod: &PodView) -> String {
    if pod.uid.is_empty() {
        pod.name.clone()
    } else {
        pod.uid.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::model::{ContainerRole, ContainerState, OwnerRef};
    use kube::core::Selector;

    const NS: &str = "flowcpt";

    // ---- builders ------------------------------------------------------

    fn targets(texts: &[&str]) -> Vec<(usize, Target)> {
        texts
            .iter()
            .enumerate()
            .map(|(index, text)| (index, text.parse().unwrap()))
            .collect()
    }

    fn state(texts: &[&str]) -> (NamespaceState, Vec<Output>) {
        NamespaceState::new(NS, targets(texts), None, true)
    }

    fn selector(pairs: &[(&'static str, &'static str)]) -> Selector {
        pairs.iter().copied().collect()
    }

    fn owner_ref(kind: OwnerKind, name: &str) -> OwnerRef {
        OwnerRef {
            kind: kind.kind_name().to_string(),
            name: name.to_string(),
            uid: format!("uid-{name}"),
        }
    }

    fn workload(
        kind: OwnerKind,
        name: &str,
        labels: &[(&'static str, &'static str)],
        controller: Option<OwnerRef>,
    ) -> WorkloadView {
        WorkloadView {
            kind,
            name: name.to_string(),
            uid: format!("uid-{name}"),
            controller,
            selector: (kind != OwnerKind::CronJob).then(|| selector(labels)),
        }
    }

    fn deployment(name: &str, labels: &[(&'static str, &'static str)]) -> WorkloadView {
        workload(OwnerKind::Deployment, name, labels, None)
    }

    fn replica_set(name: &str, deployment: &str) -> WorkloadView {
        let controller = owner_ref(OwnerKind::Deployment, deployment);
        workload(
            OwnerKind::ReplicaSet,
            name,
            &[("hash", "x")],
            Some(controller),
        )
    }

    fn running(name: &str) -> ContainerView {
        ContainerView {
            name: name.to_string(),
            role: ContainerRole::Main,
            restart_count: 0,
            state: ContainerState::Running { started_at: None },
            has_previous: false,
        }
    }

    fn pod(
        name: &str,
        labels: &[(&str, &str)],
        controller: Option<OwnerRef>,
        containers: &[&str],
    ) -> PodView {
        PodView {
            name: name.to_string(),
            uid: format!("uid-{name}"),
            labels: labels
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            controller,
            containers: containers.iter().map(|name| running(name)).collect(),
        }
    }

    /// A pod of the replica set `set`, with the labels `app=<app>`.
    fn set_pod(name: &str, app: &str, set: &str, containers: &[&str]) -> PodView {
        let controller = owner_ref(OwnerKind::ReplicaSet, set);
        pod(name, &[("app", app)], Some(controller), containers)
    }

    fn owners(kind: OwnerKind, name: Option<&str>) -> WatchSpec {
        WatchSpec::Owners {
            kind,
            name: name.map(str::to_string),
        }
    }

    fn pods(selector: Option<&str>) -> WatchSpec {
        WatchSpec::Pods {
            selector: selector.map(str::to_string),
        }
    }

    // ---- reading the outputs -------------------------------------------

    fn starts(out: &[Output]) -> Vec<WatchSpec> {
        out.iter()
            .filter_map(|output| match output {
                Output::Start(spec) => Some(spec.clone()),
                _ => None,
            })
            .collect()
    }

    fn stops(out: &[Output]) -> Vec<WatchSpec> {
        out.iter()
            .filter_map(|output| match output {
                Output::Stop(spec) => Some(spec.clone()),
                _ => None,
            })
            .collect()
    }

    /// The container events as short texts: `+pod/container` for added (with
    /// `!` when it was there from the start), `~` for changed, `-` for removed.
    fn changes(out: &[Output]) -> Vec<String> {
        out.iter()
            .filter_map(|output| match output {
                Output::Container(ContainerEvent::Added { id, initial, .. }) => Some(format!(
                    "+{}/{}{}",
                    id.pod,
                    id.container,
                    if *initial { "!" } else { "" }
                )),
                Output::Container(ContainerEvent::Changed { id, .. }) => {
                    Some(format!("~{}/{}", id.pod, id.container))
                }
                Output::Container(ContainerEvent::Removed { id }) => {
                    Some(format!("-{}/{}", id.pod, id.container))
                }
                _ => None,
            })
            .collect()
    }

    fn notices(out: &[Output]) -> Vec<String> {
        out.iter()
            .filter_map(|output| match output {
                Output::Notice { message, .. } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    fn settled(out: &[Output]) -> Option<Vec<usize>> {
        out.iter().find_map(|output| match output {
            Output::Settled { matched } => Some(matched.iter().copied().collect()),
            _ => None,
        })
    }

    // ---- feeding the state ---------------------------------------------

    /// Lets a watch list these objects, as a watch does when it starts.
    fn list_owners(
        state: &mut NamespaceState,
        spec: &WatchSpec,
        items: Vec<WorkloadView>,
    ) -> Vec<Output> {
        let mut out = state.handle(Input::Owner(spec.clone(), Event::Init));
        for item in items {
            out.extend(state.handle(Input::Owner(spec.clone(), Event::InitApply(item))));
        }
        out.extend(state.handle(Input::Owner(spec.clone(), Event::InitDone)));
        out
    }

    fn list_pods(state: &mut NamespaceState, spec: &WatchSpec, items: Vec<PodView>) -> Vec<Output> {
        let mut out = state.handle(Input::Pod(spec.clone(), Event::Init));
        for item in items {
            out.extend(state.handle(Input::Pod(spec.clone(), Event::InitApply(item))));
        }
        out.extend(state.handle(Input::Pod(spec.clone(), Event::InitDone)));
        out
    }

    fn apply_pod(state: &mut NamespaceState, spec: &WatchSpec, pod: PodView) -> Vec<Output> {
        state.handle(Input::Pod(spec.clone(), Event::Apply(pod)))
    }

    fn delete_pod(state: &mut NamespaceState, spec: &WatchSpec, pod: PodView) -> Vec<Output> {
        state.handle(Input::Pod(spec.clone(), Event::Delete(pod)))
    }

    fn apply_owner(
        state: &mut NamespaceState,
        spec: &WatchSpec,
        owner: WorkloadView,
    ) -> Vec<Output> {
        state.handle(Input::Owner(spec.clone(), Event::Apply(owner)))
    }

    /// A state that follows the deployment `flowcpt-api`, with every owner
    /// watch listed: the deployment and its replica set `flowcpt-api-7d9f`.
    fn following_the_api() -> (NamespaceState, WatchSpec) {
        let (mut state, _) = state(&["flowcpt/deployment:flowcpt-api"]);
        let deployments = owners(OwnerKind::Deployment, Some("flowcpt-api"));
        let sets = owners(OwnerKind::ReplicaSet, None);
        list_owners(
            &mut state,
            &deployments,
            vec![deployment("flowcpt-api", &[("app", "flowcpt-api")])],
        );
        list_owners(
            &mut state,
            &sets,
            vec![replica_set("flowcpt-api-7d9f", "flowcpt-api")],
        );
        (state, pods(Some("app=flowcpt-api")))
    }

    // ---- which watches -------------------------------------------------

    #[test]
    fn a_bare_namespace_needs_one_watch_on_all_pods() {
        let (_, out) = state(&["flowcpt"]);
        assert_eq!(starts(&out), [pods(None)]);
        assert_eq!(settled(&out), None);
    }

    #[test]
    fn a_plain_name_is_looked_up_across_the_five_kinds() {
        let (_, out) = state(&["flowcpt/flowcpt-api"]);
        assert_eq!(
            starts(&out),
            [
                owners(OwnerKind::Deployment, Some("flowcpt-api")),
                owners(OwnerKind::ReplicaSet, None),
                owners(OwnerKind::StatefulSet, Some("flowcpt-api")),
                owners(OwnerKind::DaemonSet, Some("flowcpt-api")),
                // All jobs: the cron job of that name is followed through them.
                owners(OwnerKind::Job, None),
                owners(OwnerKind::CronJob, Some("flowcpt-api")),
            ]
        );
    }

    #[test]
    fn a_prefix_limits_the_lookup_to_one_kind() {
        let (_, out) = state(&["flowcpt/statefulset:redis"]);
        assert_eq!(
            starts(&out),
            [owners(OwnerKind::StatefulSet, Some("redis"))]
        );

        let (_, out) = state(&["flowcpt/deployment:flowcpt-api"]);
        assert_eq!(
            starts(&out),
            [
                owners(OwnerKind::Deployment, Some("flowcpt-api")),
                owners(OwnerKind::ReplicaSet, None),
            ]
        );

        let (_, out) = state(&["flowcpt/cronjob:nightly-sync"]);
        assert_eq!(
            starts(&out),
            [
                owners(OwnerKind::Job, None),
                owners(OwnerKind::CronJob, Some("nightly-sync")),
            ]
        );
    }

    #[test]
    fn a_glob_or_two_names_watch_the_whole_kind() {
        let (_, out) = state(&["flowcpt/deployment:flowcpt-*"]);
        assert!(starts(&out).contains(&owners(OwnerKind::Deployment, None)));

        let (_, out) = state(&["flowcpt/job:a", "flowcpt/job:b"]);
        assert_eq!(starts(&out), [owners(OwnerKind::Job, None)]);

        let (_, out) = state(&["flowcpt/job:a", "flowcpt/job:a/*/main"]);
        assert_eq!(starts(&out), [owners(OwnerKind::Job, Some("a"))]);
    }

    // ---- a bare namespace ----------------------------------------------

    #[test]
    fn a_bare_namespace_follows_every_container_of_every_pod() {
        let (mut state, _) = state(&["flowcpt"]);
        let out = list_pods(
            &mut state,
            &pods(None),
            vec![
                set_pod(
                    "flowcpt-api-7d9f-xk2p1",
                    "flowcpt-api",
                    "flowcpt-api-7d9f",
                    &["api", "envoy"],
                ),
                pod("shell", &[], None, &["sh"]),
            ],
        );
        assert_eq!(
            changes(&out),
            [
                "+flowcpt-api-7d9f-xk2p1/api!",
                "+flowcpt-api-7d9f-xk2p1/envoy!",
                "+shell/sh!"
            ]
        );
        // Pods that were there at the start get no notice of their own.
        assert_eq!(notices(&out), Vec::<String>::new());
        assert_eq!(settled(&out), Some(vec![0]));
    }

    #[test]
    fn the_pod_and_container_levels_narrow_a_target() {
        let (mut state, _) = state(&["flowcpt/-/flowcpt-api-*/api"]);
        let out = list_pods(
            &mut state,
            &pods(None),
            vec![
                pod("flowcpt-api-7d9f-xk2p1", &[], None, &["api", "envoy"]),
                pod("redis-0", &[], None, &["api"]),
            ],
        );
        assert_eq!(changes(&out), ["+flowcpt-api-7d9f-xk2p1/api!"]);
    }

    #[test]
    fn the_containers_option_keeps_sidecars_out() {
        let (mut state, _) = NamespaceState::new(
            NS,
            targets(&["flowcpt"]),
            Some(Pattern::new("{api,worker}").unwrap()),
            true,
        );
        let out = list_pods(
            &mut state,
            &pods(None),
            vec![
                pod("a", &[], None, &["api", "envoy"]),
                pod("b", &[], None, &["envoy"]),
            ],
        );
        assert_eq!(changes(&out), ["+a/api!"]);
    }

    #[test]
    fn several_targets_add_up() {
        let (mut state, _) = state(&["flowcpt/-/a/api", "flowcpt/-/*/envoy", "flowcpt/-/zzz"]);
        let out = list_pods(
            &mut state,
            &pods(None),
            vec![
                pod("a", &[], None, &["api", "envoy", "other"]),
                pod("b", &[], None, &["api", "envoy"]),
            ],
        );
        assert_eq!(changes(&out), ["+a/api!", "+a/envoy!", "+b/envoy!"]);
        // The third target selects nothing.
        assert_eq!(settled(&out), Some(vec![0, 1]));
    }

    // ---- a deployment --------------------------------------------------

    #[test]
    fn a_deployment_starts_a_pod_watch_on_its_labels() {
        let (mut state, _) = state(&["flowcpt/deployment:flowcpt-api"]);
        let out = list_owners(
            &mut state,
            &owners(OwnerKind::Deployment, Some("flowcpt-api")),
            vec![deployment("flowcpt-api", &[("app", "flowcpt-api")])],
        );
        assert_eq!(starts(&out), [pods(Some("app=flowcpt-api"))]);
        // The replica sets and the pods are still to come.
        assert_eq!(settled(&out), None);
    }

    #[test]
    fn a_deployment_resolves_through_its_replica_sets() {
        let (mut state, pod_watch) = following_the_api();
        let out = list_pods(
            &mut state,
            &pod_watch,
            vec![
                set_pod(
                    "flowcpt-api-7d9f-xk2p1",
                    "flowcpt-api",
                    "flowcpt-api-7d9f",
                    &["api"],
                ),
                // The same labels, but the replica set of another deployment.
                set_pod("imposter-1", "flowcpt-api", "other-5c8b", &["api"]),
                // The same labels and no controller at all.
                pod("bare", &[("app", "flowcpt-api")], None, &["api"]),
            ],
        );
        assert_eq!(changes(&out), ["+flowcpt-api-7d9f-xk2p1/api!"]);
        assert_eq!(settled(&out), Some(vec![0]));
    }

    #[test]
    fn a_pod_waits_for_its_replica_set_to_be_known() {
        let (mut state, _) = state(&["flowcpt/deployment:flowcpt-api"]);
        let sets = owners(OwnerKind::ReplicaSet, None);
        list_owners(
            &mut state,
            &owners(OwnerKind::Deployment, Some("flowcpt-api")),
            vec![deployment("flowcpt-api", &[("app", "flowcpt-api")])],
        );
        list_owners(&mut state, &sets, vec![]);
        let pod_watch = pods(Some("app=flowcpt-api"));
        list_pods(&mut state, &pod_watch, vec![]);

        // A rollout: the pod of the new replica set is reported first.
        let new_pod = set_pod(
            "flowcpt-api-5c8b-abcde",
            "flowcpt-api",
            "flowcpt-api-5c8b",
            &["api"],
        );
        let out = apply_pod(&mut state, &pod_watch, new_pod);
        assert_eq!(changes(&out), Vec::<String>::new());

        let out = apply_owner(
            &mut state,
            &sets,
            replica_set("flowcpt-api-5c8b", "flowcpt-api"),
        );
        assert_eq!(changes(&out), ["+flowcpt-api-5c8b-abcde/api"]);
        assert_eq!(
            notices(&out),
            ["pod flowcpt-api-5c8b-abcde appeared; following api"]
        );
    }

    #[test]
    fn a_rollout_follows_the_pods_of_both_replica_sets() {
        let (mut state, pod_watch) = following_the_api();
        let old = set_pod(
            "flowcpt-api-7d9f-xk2p1",
            "flowcpt-api",
            "flowcpt-api-7d9f",
            &["api"],
        );
        list_pods(&mut state, &pod_watch, vec![old.clone()]);

        // The new replica set and its pod come up while the old pod still runs.
        apply_owner(
            &mut state,
            &owners(OwnerKind::ReplicaSet, None),
            replica_set("flowcpt-api-5c8b", "flowcpt-api"),
        );
        let new = set_pod(
            "flowcpt-api-5c8b-abcde",
            "flowcpt-api",
            "flowcpt-api-5c8b",
            &["api"],
        );
        let out = apply_pod(&mut state, &pod_watch, new);
        assert_eq!(changes(&out), ["+flowcpt-api-5c8b-abcde/api"]);
        assert_eq!(
            notices(&out),
            ["pod flowcpt-api-5c8b-abcde appeared; following api"]
        );

        // Then the old pod goes.
        let out = delete_pod(&mut state, &pod_watch, old);
        assert_eq!(changes(&out), ["-flowcpt-api-7d9f-xk2p1/api"]);
        assert_eq!(notices(&out), ["pod flowcpt-api-7d9f-xk2p1 went away"]);
    }

    #[test]
    fn a_target_that_matches_nothing_settles_and_keeps_watching() {
        // The deployment is there, scaled to zero.
        let (mut state, pod_watch) = following_the_api();
        let out = list_pods(&mut state, &pod_watch, vec![]);
        assert_eq!(settled(&out), Some(vec![]));
        assert_eq!(changes(&out), Vec::<String>::new());

        // It is scaled up: the pod was not there at the start.
        let out = apply_pod(
            &mut state,
            &pod_watch,
            set_pod(
                "flowcpt-api-7d9f-xk2p1",
                "flowcpt-api",
                "flowcpt-api-7d9f",
                &["api"],
            ),
        );
        assert_eq!(changes(&out), ["+flowcpt-api-7d9f-xk2p1/api"]);
        assert_eq!(
            notices(&out),
            ["pod flowcpt-api-7d9f-xk2p1 appeared; following api"]
        );
        // Settled is said once.
        assert_eq!(settled(&out), None);
    }

    #[test]
    fn a_deployment_that_does_not_exist_settles_with_nothing_and_is_found_later() {
        let (mut state, _) = state(&["flowcpt/deployment:flowcpt-api"]);
        let deployments = owners(OwnerKind::Deployment, Some("flowcpt-api"));
        let sets = owners(OwnerKind::ReplicaSet, None);
        let out = list_owners(&mut state, &deployments, vec![]);
        assert_eq!(settled(&out), None);
        let out = list_owners(&mut state, &sets, vec![]);
        assert_eq!(settled(&out), Some(vec![]));

        // The deployment is created while the session runs.
        let out = apply_owner(
            &mut state,
            &deployments,
            deployment("flowcpt-api", &[("app", "flowcpt-api")]),
        );
        let pod_watch = pods(Some("app=flowcpt-api"));
        assert_eq!(starts(&out), std::slice::from_ref(&pod_watch));
        apply_owner(
            &mut state,
            &sets,
            replica_set("flowcpt-api-7d9f", "flowcpt-api"),
        );
        let out = list_pods(
            &mut state,
            &pod_watch,
            vec![set_pod(
                "flowcpt-api-7d9f-xk2p1",
                "flowcpt-api",
                "flowcpt-api-7d9f",
                &["api"],
            )],
        );
        assert_eq!(changes(&out), ["+flowcpt-api-7d9f-xk2p1/api"]);
    }

    #[test]
    fn a_deployment_that_is_deleted_stops_its_pod_watch_and_lets_its_pods_go() {
        let (mut state, pod_watch) = following_the_api();
        list_pods(
            &mut state,
            &pod_watch,
            vec![set_pod(
                "flowcpt-api-7d9f-xk2p1",
                "flowcpt-api",
                "flowcpt-api-7d9f",
                &["api"],
            )],
        );
        let out = state.handle(Input::Owner(
            owners(OwnerKind::Deployment, Some("flowcpt-api")),
            Event::Delete(deployment("flowcpt-api", &[("app", "flowcpt-api")])),
        ));
        assert_eq!(stops(&out), std::slice::from_ref(&pod_watch));
        assert_eq!(changes(&out), ["-flowcpt-api-7d9f-xk2p1/api"]);
        assert_eq!(notices(&out), ["pod flowcpt-api-7d9f-xk2p1 went away"]);

        // What the stopped watch still had under way is ignored.
        let late = set_pod(
            "flowcpt-api-7d9f-zzzzz",
            "flowcpt-api",
            "flowcpt-api-7d9f",
            &["api"],
        );
        assert_eq!(apply_pod(&mut state, &pod_watch, late), []);
    }

    // ---- the other kinds -----------------------------------------------

    #[test]
    fn stateful_sets_daemon_sets_and_jobs_resolve_through_their_selector() {
        for (prefix, kind) in [
            ("statefulset", OwnerKind::StatefulSet),
            ("daemonset", OwnerKind::DaemonSet),
            ("job", OwnerKind::Job),
        ] {
            let (mut state, _) = state(&[&format!("flowcpt/{prefix}:redis")]);
            let out = list_owners(
                &mut state,
                &owners(kind, Some("redis")),
                vec![workload(kind, "redis", &[("app", "redis")], None)],
            );
            let pod_watch = pods(Some("app=redis"));
            assert_eq!(starts(&out), std::slice::from_ref(&pod_watch), "{prefix}");

            let out = list_pods(
                &mut state,
                &pod_watch,
                vec![pod("redis-0", &[("app", "redis")], None, &["redis"])],
            );
            assert_eq!(changes(&out), ["+redis-0/redis!"], "{prefix}");
            assert_eq!(settled(&out), Some(vec![0]), "{prefix}");
        }
    }

    /// A job of the cron job `nightly-sync`, selecting pods by `job=<name>`.
    fn run_of_nightly_sync(name: &'static str) -> WorkloadView {
        let controller = owner_ref(OwnerKind::CronJob, "nightly-sync");
        workload(OwnerKind::Job, name, &[("job", name)], Some(controller))
    }

    #[test]
    fn a_cron_job_resolves_through_its_jobs_and_picks_up_every_run() {
        let (mut state, _) = state(&["flowcpt/cronjob:nightly-sync"]);
        let jobs = owners(OwnerKind::Job, None);
        list_owners(
            &mut state,
            &owners(OwnerKind::CronJob, Some("nightly-sync")),
            vec![workload(OwnerKind::CronJob, "nightly-sync", &[], None)],
        );
        let out = list_owners(
            &mut state,
            &jobs,
            vec![
                run_of_nightly_sync("nightly-sync-001"),
                // A job of its own, which the target does not name.
                workload(OwnerKind::Job, "migrate", &[("job", "migrate")], None),
            ],
        );
        let first_run = pods(Some("job=nightly-sync-001"));
        assert_eq!(starts(&out), std::slice::from_ref(&first_run));

        let job_pod = |name: &str, job: &'static str| {
            pod(
                name,
                &[("job", job)],
                Some(owner_ref(OwnerKind::Job, job)),
                &["sync"],
            )
        };
        let out = list_pods(
            &mut state,
            &first_run,
            vec![job_pod("nightly-sync-001-abcde", "nightly-sync-001")],
        );
        assert_eq!(changes(&out), ["+nightly-sync-001-abcde/sync!"]);
        assert_eq!(settled(&out), Some(vec![0]));

        // The next run: a new job, a new pod watch, a new pod.
        let out = apply_owner(&mut state, &jobs, run_of_nightly_sync("nightly-sync-002"));
        let second_run = pods(Some("job=nightly-sync-002"));
        assert_eq!(starts(&out), std::slice::from_ref(&second_run));
        let out = list_pods(
            &mut state,
            &second_run,
            vec![job_pod("nightly-sync-002-fghij", "nightly-sync-002")],
        );
        assert_eq!(changes(&out), ["+nightly-sync-002-fghij/sync"]);

        // The first run is cleaned up: its watch stops and its pod goes.
        let out = state.handle(Input::Owner(
            jobs,
            Event::Delete(run_of_nightly_sync("nightly-sync-001")),
        ));
        assert_eq!(stops(&out), [first_run]);
        assert_eq!(changes(&out), ["-nightly-sync-001-abcde/sync"]);
    }

    #[test]
    fn a_plain_name_shared_by_two_kinds_follows_both_and_says_so() {
        let (mut state, _) = state(&["flowcpt/redis"]);
        list_owners(
            &mut state,
            &owners(OwnerKind::StatefulSet, Some("redis")),
            vec![workload(
                OwnerKind::StatefulSet,
                "redis",
                &[("app", "redis")],
                None,
            )],
        );
        let out = list_owners(
            &mut state,
            &owners(OwnerKind::DaemonSet, Some("redis")),
            vec![workload(
                OwnerKind::DaemonSet,
                "redis",
                &[("app", "redis-agent")],
                None,
            )],
        );
        assert_eq!(starts(&out), [pods(Some("app=redis-agent"))]);
        assert_eq!(
            notices(&out),
            [
                "redis in the namespace flowcpt is the name of a StatefulSet and a DaemonSet; all \
              of them are followed. A prefix picks one, as in flowcpt/statefulset:redis"
            ]
        );

        // It is said once, not with every event.
        let out = apply_owner(
            &mut state,
            &owners(OwnerKind::DaemonSet, Some("redis")),
            workload(
                OwnerKind::DaemonSet,
                "redis",
                &[("app", "redis-agent")],
                None,
            ),
        );
        assert_eq!(notices(&out), Vec::<String>::new());

        let out = list_pods(
            &mut state,
            &pods(Some("app=redis")),
            vec![pod("redis-0", &[("app", "redis")], None, &["redis"])],
        );
        assert_eq!(changes(&out), ["+redis-0/redis!"]);
        let out = list_pods(
            &mut state,
            &pods(Some("app=redis-agent")),
            vec![pod(
                "redis-x7k2p",
                &[("app", "redis-agent")],
                None,
                &["agent"],
            )],
        );
        assert_eq!(changes(&out), ["+redis-x7k2p/agent!"]);
    }

    #[test]
    fn a_glob_follows_every_workload_it_matches() {
        let (mut state, _) = state(&["flowcpt/statefulset:redis-*"]);
        let sets = owners(OwnerKind::StatefulSet, None);
        let out = list_owners(
            &mut state,
            &sets,
            vec![
                workload(
                    OwnerKind::StatefulSet,
                    "redis-cache",
                    &[("app", "cache")],
                    None,
                ),
                workload(
                    OwnerKind::StatefulSet,
                    "redis-queue",
                    &[("app", "queue")],
                    None,
                ),
                workload(
                    OwnerKind::StatefulSet,
                    "postgres",
                    &[("app", "postgres")],
                    None,
                ),
            ],
        );
        assert_eq!(
            starts(&out),
            [pods(Some("app=cache")), pods(Some("app=queue"))]
        );
        // One that is created later is picked up too.
        let out = apply_owner(
            &mut state,
            &sets,
            workload(OwnerKind::StatefulSet, "redis-new", &[("app", "new")], None),
        );
        assert_eq!(starts(&out), [pods(Some("app=new"))]);
    }

    // ---- changes while following ---------------------------------------

    #[test]
    fn a_new_state_of_a_container_is_passed_on() {
        let (mut state, _) = state(&["flowcpt"]);
        let all = pods(None);
        let before = pod("a", &[], None, &["api", "envoy"]);
        list_pods(&mut state, &all, vec![before.clone()]);

        // The same again changes nothing.
        assert_eq!(
            changes(&apply_pod(&mut state, &all, before.clone())),
            Vec::<String>::new()
        );

        let mut after = before;
        after.containers[0].restart_count = 1;
        after.containers[0].state = ContainerState::Waiting {
            reason: Some("CrashLoopBackOff".into()),
            message: None,
        };
        let out = apply_pod(&mut state, &all, after.clone());
        assert_eq!(changes(&out), ["~a/api"]);
        let Output::Container(ContainerEvent::Changed { view, .. }) = &out[0] else {
            panic!("expected a change, got {out:?}");
        };
        assert_eq!(view, &after.containers[0]);
    }

    #[test]
    fn a_container_added_to_a_pod_is_followed() {
        let (mut state, _) = state(&["flowcpt"]);
        let all = pods(None);
        list_pods(&mut state, &all, vec![pod("a", &[], None, &["api"])]);
        // An ephemeral container, say.
        let out = apply_pod(&mut state, &all, pod("a", &[], None, &["api", "debugger"]));
        assert_eq!(changes(&out), ["+a/debugger"]);
        // The pod was followed already, so it does not "appear".
        assert_eq!(notices(&out), Vec::<String>::new());
    }

    #[test]
    fn a_pod_made_again_under_the_same_name_is_another_pod() {
        let (mut state, _) = state(&["flowcpt"]);
        let all = pods(None);
        let first = pod("redis-0", &[], None, &["redis"]);
        list_pods(&mut state, &all, vec![first.clone()]);

        let mut second = first.clone();
        second.uid = "uid-redis-0-again".to_string();
        let out = delete_pod(&mut state, &all, first);
        assert_eq!(changes(&out), ["-redis-0/redis"]);
        let out = apply_pod(&mut state, &all, second);
        assert_eq!(changes(&out), ["+redis-0/redis"]);
        let Output::Container(ContainerEvent::Added { id, .. }) = &out[0] else {
            panic!("expected an addition, got {out:?}");
        };
        assert_eq!(id.pod_uid, "uid-redis-0-again");
        assert_eq!(id.namespace, "flowcpt");
    }

    #[test]
    fn a_watch_that_lists_again_drops_what_is_gone_and_keeps_what_is_there() {
        let (mut state, _) = state(&["flowcpt"]);
        let all = pods(None);
        let stays = pod("stays", &[], None, &["api"]);
        let goes = pod("goes", &[], None, &["api"]);
        list_pods(&mut state, &all, vec![stays.clone(), goes]);

        // The watch lost its place and lists again; one pod is gone, one is new.
        let out = list_pods(
            &mut state,
            &all,
            vec![stays, pod("new", &[], None, &["api"])],
        );
        assert_eq!(changes(&out), ["+new/api", "-goes/api"]);
        assert_eq!(
            notices(&out),
            ["pod new appeared; following api", "pod goes went away"]
        );
        assert_eq!(settled(&out), None);
    }

    #[test]
    fn a_pod_seen_by_two_watches_goes_when_both_let_it_go() {
        let (mut state, _) = state(&["flowcpt/statefulset:a", "flowcpt/statefulset:b"]);
        let sets = owners(OwnerKind::StatefulSet, None);
        list_owners(
            &mut state,
            &sets,
            vec![
                workload(OwnerKind::StatefulSet, "a", &[("a", "1")], None),
                workload(OwnerKind::StatefulSet, "b", &[("b", "1")], None),
            ],
        );
        let (watch_a, watch_b) = (pods(Some("a=1")), pods(Some("b=1")));
        let both = pod("both", &[("a", "1"), ("b", "1")], None, &["main"]);
        let out = list_pods(&mut state, &watch_a, vec![both.clone()]);
        assert_eq!(changes(&out), ["+both/main!"]);
        let out = list_pods(&mut state, &watch_b, vec![both.clone()]);
        assert_eq!(changes(&out), Vec::<String>::new());

        // It loses the label of `a`: that watch lets it go, the other keeps it.
        let relabelled = pod("both", &[("b", "1")], None, &["main"]);
        let out = delete_pod(&mut state, &watch_a, both);
        assert_eq!(changes(&out), Vec::<String>::new());
        apply_pod(&mut state, &watch_b, relabelled.clone());
        let out = delete_pod(&mut state, &watch_b, relabelled);
        assert_eq!(changes(&out), ["-both/main"]);
    }

    #[test]
    fn a_namespace_that_came_later_has_nothing_from_the_start() {
        let (mut state, _) = NamespaceState::new(NS, targets(&["flow*"]), None, false);
        let out = list_pods(&mut state, &pods(None), vec![pod("a", &[], None, &["api"])]);
        assert_eq!(changes(&out), ["+a/api"]);
        assert_eq!(notices(&out), ["pod a appeared; following api"]);
    }

    // ---- failures ------------------------------------------------------

    #[test]
    fn a_failing_watch_is_noticed_once_and_does_not_hold_up_the_rest() {
        let (mut state, _) = state(&["flowcpt/flowcpt-api"]);
        let cron = owners(OwnerKind::CronJob, Some("flowcpt-api"));
        let failure = || {
            Input::Failed(
                cron.clone(),
                "cronjobs.batch is forbidden (403)".to_string(),
            )
        };

        let out = state.handle(failure());
        assert_eq!(
            notices(&out),
            [
                "cannot watch the CronJob `flowcpt-api` in the namespace flowcpt: cronjobs.batch \
              is forbidden (403); trying again"
            ]
        );
        // The retries fail the same way and stay quiet.
        assert_eq!(notices(&state.handle(failure())), Vec::<String>::new());

        // The other watches settle the namespace without it.
        for (kind, name) in [
            (OwnerKind::Deployment, Some("flowcpt-api")),
            (OwnerKind::ReplicaSet, None),
            (OwnerKind::StatefulSet, Some("flowcpt-api")),
            (OwnerKind::DaemonSet, Some("flowcpt-api")),
        ] {
            assert_eq!(
                settled(&list_owners(&mut state, &owners(kind, name), vec![])),
                None
            );
        }
        let out = list_owners(&mut state, &owners(OwnerKind::Job, None), vec![]);
        assert_eq!(settled(&out), Some(vec![]));

        // Once it works again, a new failure is worth a new notice.
        list_owners(&mut state, &cron, vec![]);
        assert_eq!(notices(&state.handle(failure())).len(), 1);
    }

    #[test]
    fn a_first_list_that_fails_and_is_tried_again_still_finds_what_was_there_at_the_start() {
        let (mut state, _) = state(&["flowcpt/deployment:flowcpt-api"]);
        let deployments = owners(OwnerKind::Deployment, Some("flowcpt-api"));
        let sets = owners(OwnerKind::ReplicaSet, None);
        // The API server hiccups on the first lists.
        state.handle(Input::Failed(
            deployments.clone(),
            "connection reset".to_string(),
        ));
        let out = state.handle(Input::Failed(sets.clone(), "connection reset".to_string()));
        assert_eq!(settled(&out), Some(vec![]));

        // A second later they work. The pods were there all along: they get
        // the history of the start, not their whole log, and no notice.
        list_owners(
            &mut state,
            &deployments,
            vec![deployment("flowcpt-api", &[("app", "flowcpt-api")])],
        );
        list_owners(
            &mut state,
            &sets,
            vec![replica_set("flowcpt-api-7d9f", "flowcpt-api")],
        );
        let pod_watch = pods(Some("app=flowcpt-api"));
        state.handle(Input::Failed(
            pod_watch.clone(),
            "connection reset".to_string(),
        ));
        let out = list_pods(
            &mut state,
            &pod_watch,
            vec![set_pod(
                "flowcpt-api-7d9f-xk2p1",
                "flowcpt-api",
                "flowcpt-api-7d9f",
                &["api"],
            )],
        );
        assert_eq!(changes(&out), ["+flowcpt-api-7d9f-xk2p1/api!"]);
        assert_eq!(notices(&out), Vec::<String>::new());

        // A pod that comes after that is new.
        let out = apply_pod(
            &mut state,
            &pod_watch,
            set_pod(
                "flowcpt-api-7d9f-m4q8z",
                "flowcpt-api",
                "flowcpt-api-7d9f",
                &["api"],
            ),
        );
        assert_eq!(changes(&out), ["+flowcpt-api-7d9f-m4q8z/api"]);
    }

    #[test]
    fn a_pod_that_is_there_at_the_start_stays_so_when_it_is_matched_late() {
        // The pods are listed before the replica sets are.
        let (mut state, _) = state(&["flowcpt/deployment:flowcpt-api"]);
        list_owners(
            &mut state,
            &owners(OwnerKind::Deployment, Some("flowcpt-api")),
            vec![deployment("flowcpt-api", &[("app", "flowcpt-api")])],
        );
        let out = list_pods(
            &mut state,
            &pods(Some("app=flowcpt-api")),
            vec![set_pod(
                "flowcpt-api-7d9f-xk2p1",
                "flowcpt-api",
                "flowcpt-api-7d9f",
                &["api"],
            )],
        );
        assert_eq!(changes(&out), Vec::<String>::new());
        let out = list_owners(
            &mut state,
            &owners(OwnerKind::ReplicaSet, None),
            vec![replica_set("flowcpt-api-7d9f", "flowcpt-api")],
        );
        assert_eq!(changes(&out), ["+flowcpt-api-7d9f-xk2p1/api!"]);
        assert_eq!(notices(&out), Vec::<String>::new());
    }

    #[test]
    fn what_an_unknown_watch_reports_is_ignored() {
        let (mut state, _) = state(&["flowcpt"]);
        let stranger = pods(Some("app=nope"));
        assert_eq!(
            apply_pod(&mut state, &stranger, pod("a", &[], None, &["api"])),
            []
        );
        assert_eq!(
            state.handle(Input::Failed(stranger, "boom".to_string())),
            []
        );
        let unknown_owners = owners(OwnerKind::Deployment, None);
        assert_eq!(
            apply_owner(
                &mut state,
                &unknown_owners,
                deployment("x", &[("app", "x")])
            ),
            []
        );
    }
}
