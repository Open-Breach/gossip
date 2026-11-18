//! Targets: what to follow, written as a path
//! `namespace[/workload[/pod[/container]]]` whose names may be globs.

use std::fmt;
use std::str::FromStr;

use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Why a target or a name pattern was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TargetError {
    #[error("a target cannot be empty")]
    Empty,
    #[error("target `{target}` has an empty level; write `*` for any name")]
    EmptyLevel { target: String },
    #[error(
        "target `{target}` has {levels} levels; the most is four, namespace/workload/pod/container"
    )]
    TooManyLevels { target: String, levels: usize },
    #[error(
        "unknown workload kind `{kind}` in target `{target}`; the kinds are deployment, \
         statefulset, daemonset, job and cronjob"
    )]
    UnknownKind { target: String, kind: String },
    #[error("`{pattern}` is not a valid name or glob: {reason}")]
    BadPattern { pattern: String, reason: String },
}

/// A name or a glob over names, at one level of a target.
///
/// A pattern without glob characters matches that exact name. The glob
/// syntax is the usual one: `*`, `?`, `[a-z]` and `{a,b}`.
#[derive(Debug, Clone)]
pub struct Pattern {
    source: String,
    /// None when the pattern is a plain name.
    matcher: Option<GlobMatcher>,
}

impl Pattern {
    pub fn new(source: &str) -> Result<Self, TargetError> {
        let bad = |reason: &str| TargetError::BadPattern {
            pattern: source.to_string(),
            reason: reason.to_string(),
        };
        if source.is_empty() {
            return Err(bad("it is empty"));
        }
        if source.chars().any(char::is_whitespace) {
            return Err(bad("it holds whitespace"));
        }
        if source.contains('/') {
            return Err(bad("it holds a `/`"));
        }
        if source.contains(':') {
            return Err(bad("it holds a `:`"));
        }
        let matcher = if source.contains(['*', '?', '[', '{']) {
            let glob = GlobBuilder::new(source)
                .backslash_escape(false)
                .build()
                .map_err(|error| bad(&error.kind().to_string()))?;
            Some(glob.compile_matcher())
        } else {
            None
        };
        Ok(Pattern {
            source: source.to_string(),
            matcher,
        })
    }

    /// The pattern `*`, which matches every name.
    pub fn any() -> Self {
        Pattern::new("*").expect("`*` is a valid glob")
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether this is the pattern `*`.
    pub fn is_any(&self) -> bool {
        self.source == "*"
    }

    /// The exact name this pattern stands for, when it is not a glob.
    pub fn literal(&self) -> Option<&str> {
        self.matcher.is_none().then_some(self.source.as_str())
    }

    pub fn matches(&self, name: &str) -> bool {
        match &self.matcher {
            Some(matcher) => matcher.is_match(name),
            None => self.source == name,
        }
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Eq for Pattern {}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl FromStr for Pattern {
    type Err = TargetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Pattern::new(s)
    }
}

impl Serialize for Pattern {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.source)
    }
}

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Pattern::new(&text).map_err(serde::de::Error::custom)
    }
}

/// The kinds of workload a target can name at its second level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkloadKind {
    Deployment,
    StatefulSet,
    DaemonSet,
    Job,
    CronJob,
}

impl WorkloadKind {
    pub const ALL: [WorkloadKind; 5] = [
        WorkloadKind::Deployment,
        WorkloadKind::StatefulSet,
        WorkloadKind::DaemonSet,
        WorkloadKind::Job,
        WorkloadKind::CronJob,
    ];

    /// The prefix that picks this kind in a target, as in `cronjob:nightly`.
    pub fn prefix(self) -> &'static str {
        match self {
            WorkloadKind::Deployment => "deployment",
            WorkloadKind::StatefulSet => "statefulset",
            WorkloadKind::DaemonSet => "daemonset",
            WorkloadKind::Job => "job",
            WorkloadKind::CronJob => "cronjob",
        }
    }

    /// The kind as Kubernetes writes it, as in `CronJob`.
    pub fn kind_name(self) -> &'static str {
        match self {
            WorkloadKind::Deployment => "Deployment",
            WorkloadKind::StatefulSet => "StatefulSet",
            WorkloadKind::DaemonSet => "DaemonSet",
            WorkloadKind::Job => "Job",
            WorkloadKind::CronJob => "CronJob",
        }
    }

    fn from_prefix(prefix: &str) -> Option<Self> {
        WorkloadKind::ALL
            .into_iter()
            .find(|kind| kind.prefix().eq_ignore_ascii_case(prefix))
    }
}

impl fmt::Display for WorkloadKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind_name())
    }
}

/// The second level of a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkloadSelector {
    /// Every pod of the namespace, whatever owns it. Written `*`, `-`, or
    /// left out.
    Any,
    /// The workloads whose name matches, of one kind when a prefix picks it
    /// and of any kind otherwise.
    Named {
        kind: Option<WorkloadKind>,
        name: Pattern,
    },
}

impl WorkloadSelector {
    /// Whether a workload of this kind and name is selected.
    pub fn matches(&self, kind: WorkloadKind, name: &str) -> bool {
        match self {
            WorkloadSelector::Any => true,
            WorkloadSelector::Named {
                kind: wanted,
                name: pattern,
            } => wanted.is_none_or(|wanted| wanted == kind) && pattern.matches(name),
        }
    }
}

impl fmt::Display for WorkloadSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkloadSelector::Any => f.write_str("*"),
            WorkloadSelector::Named {
                kind: Some(kind),
                name,
            } => write!(f, "{}:{name}", kind.prefix()),
            WorkloadSelector::Named { kind: None, name } => write!(f, "{name}"),
        }
    }
}

/// What to follow: a namespace, narrowed optionally to a workload, a pod or
/// a container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub namespace: Pattern,
    pub workload: WorkloadSelector,
    pub pod: Pattern,
    pub container: Pattern,
}

impl Target {
    /// Every container of every pod of a namespace.
    pub fn namespace(namespace: Pattern) -> Self {
        Target {
            namespace,
            workload: WorkloadSelector::Any,
            pod: Pattern::any(),
            container: Pattern::any(),
        }
    }
}

impl FromStr for Target {
    type Err = TargetError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let text = input.trim();
        if text.is_empty() {
            return Err(TargetError::Empty);
        }
        let levels: Vec<&str> = text.split('/').collect();
        if levels.len() > 4 {
            return Err(TargetError::TooManyLevels {
                target: text.to_string(),
                levels: levels.len(),
            });
        }
        if levels.iter().any(|level| level.is_empty()) {
            return Err(TargetError::EmptyLevel {
                target: text.to_string(),
            });
        }

        let namespace = Pattern::new(levels[0])?;
        let workload = match levels.get(1).copied() {
            None | Some("*") | Some("-") => WorkloadSelector::Any,
            Some(level) => match level.split_once(':') {
                Some((prefix, name)) => {
                    let kind = WorkloadKind::from_prefix(prefix).ok_or_else(|| {
                        TargetError::UnknownKind {
                            target: text.to_string(),
                            kind: prefix.to_string(),
                        }
                    })?;
                    WorkloadSelector::Named {
                        kind: Some(kind),
                        name: Pattern::new(name)?,
                    }
                }
                None => WorkloadSelector::Named {
                    kind: None,
                    name: Pattern::new(level)?,
                },
            },
        };
        let level_or_any = |index: usize| match levels.get(index) {
            Some(level) => Pattern::new(level),
            None => Ok(Pattern::any()),
        };
        Ok(Target {
            namespace,
            workload,
            pod: level_or_any(2)?,
            container: level_or_any(3)?,
        })
    }
}

impl fmt::Display for Target {
    /// Writes the shortest path that parses back to this target. A pod given
    /// without a workload is written with `-` at the workload level.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.namespace)?;
        let with_container = !self.container.is_any();
        let with_pod = with_container || !self.pod.is_any();
        match &self.workload {
            WorkloadSelector::Any if with_pod => f.write_str("/-")?,
            WorkloadSelector::Any => {}
            named => write!(f, "/{named}")?,
        }
        if with_pod {
            write!(f, "/{}", self.pod)?;
        }
        if with_container {
            write!(f, "/{}", self.container)?;
        }
        Ok(())
    }
}

impl Serialize for Target {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text: &str) -> Target {
        text.parse()
            .unwrap_or_else(|error| panic!("`{text}` should parse: {error}"))
    }

    fn named(kind: Option<WorkloadKind>, name: &str) -> WorkloadSelector {
        WorkloadSelector::Named {
            kind,
            name: Pattern::new(name).unwrap(),
        }
    }

    #[test]
    fn a_bare_namespace_follows_everything_in_it() {
        let parsed = target("flowcpt");
        assert_eq!(parsed.namespace.as_str(), "flowcpt");
        assert_eq!(parsed.workload, WorkloadSelector::Any);
        assert!(parsed.pod.is_any());
        assert!(parsed.container.is_any());
    }

    #[test]
    fn namespace_star_means_the_same_as_the_bare_namespace() {
        assert_eq!(target("flowcpt/*"), target("flowcpt"));
    }

    #[test]
    fn the_second_level_names_a_workload_of_any_kind() {
        let parsed = target("flowcpt/flowcpt-api");
        assert_eq!(parsed.workload, named(None, "flowcpt-api"));
        assert!(parsed.pod.is_any());
        for kind in WorkloadKind::ALL {
            assert!(parsed.workload.matches(kind, "flowcpt-api"));
            assert!(!parsed.workload.matches(kind, "flowcpt-web-api"));
        }
    }

    #[test]
    fn a_prefix_picks_one_workload_kind() {
        let parsed = target("flowcpt/cronjob:nightly-sync");
        assert_eq!(
            parsed.workload,
            named(Some(WorkloadKind::CronJob), "nightly-sync")
        );
        assert!(
            parsed
                .workload
                .matches(WorkloadKind::CronJob, "nightly-sync")
        );
        assert!(!parsed.workload.matches(WorkloadKind::Job, "nightly-sync"));

        for (prefix, kind) in [
            ("deployment", WorkloadKind::Deployment),
            ("statefulset", WorkloadKind::StatefulSet),
            ("daemonset", WorkloadKind::DaemonSet),
            ("job", WorkloadKind::Job),
            ("CronJob", WorkloadKind::CronJob),
        ] {
            let parsed = target(&format!("ns/{prefix}:name"));
            assert_eq!(parsed.workload, named(Some(kind), "name"));
        }
    }

    #[test]
    fn a_prefix_goes_with_a_glob() {
        let parsed = target("flowcpt/cronjob:*");
        assert!(parsed.workload.matches(WorkloadKind::CronJob, "anything"));
        assert!(
            !parsed
                .workload
                .matches(WorkloadKind::Deployment, "anything")
        );
    }

    #[test]
    fn all_four_levels() {
        let parsed = target("monitoring/prometheus/*/exporter");
        assert_eq!(parsed.namespace.as_str(), "monitoring");
        assert_eq!(parsed.workload, named(None, "prometheus"));
        assert!(parsed.pod.is_any());
        assert_eq!(parsed.container.as_str(), "exporter");
        assert!(parsed.container.matches("exporter"));
        assert!(!parsed.container.matches("prometheus"));
    }

    #[test]
    fn a_dash_bypasses_the_workload_level() {
        let parsed = target("flowcpt/-/flowcpt-api-7d9f-xk2p1");
        assert_eq!(parsed.workload, WorkloadSelector::Any);
        assert_eq!(parsed.pod.literal(), Some("flowcpt-api-7d9f-xk2p1"));
        assert_eq!(parsed, target("flowcpt/*/flowcpt-api-7d9f-xk2p1"));
    }

    #[test]
    fn globs_match_at_every_level() {
        let parsed = target("flow*/flowcpt-*/*-xk2p1/{api,envoy}");
        assert!(parsed.namespace.matches("flowcpt"));
        assert!(parsed.namespace.matches("flowcpt-stg"));
        assert!(!parsed.namespace.matches("monitoring"));
        assert!(
            parsed
                .workload
                .matches(WorkloadKind::Deployment, "flowcpt-api")
        );
        assert!(!parsed.workload.matches(WorkloadKind::Deployment, "redis"));
        assert!(parsed.pod.matches("flowcpt-api-7d9f-xk2p1"));
        assert!(!parsed.pod.matches("flowcpt-api-7d9f-aaaaa"));
        assert!(parsed.container.matches("api"));
        assert!(parsed.container.matches("envoy"));
        assert!(!parsed.container.matches("init"));
    }

    #[test]
    fn glob_syntax() {
        let question = Pattern::new("web-?").unwrap();
        assert!(question.matches("web-0"));
        assert!(!question.matches("web-10"));

        let class = Pattern::new("web-[0-2]").unwrap();
        assert!(class.matches("web-1"));
        assert!(!class.matches("web-3"));

        let star = Pattern::new("*").unwrap();
        assert!(star.is_any());
        assert!(star.matches("anything-at-all"));
        assert_eq!(star.literal(), None);

        let plain = Pattern::new("api").unwrap();
        assert_eq!(plain.literal(), Some("api"));
        assert!(plain.matches("api"));
        assert!(!plain.matches("api-2"));
        assert!(!plain.matches("API"));
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            target("  flowcpt/flowcpt-api "),
            target("flowcpt/flowcpt-api")
        );
    }

    #[test]
    fn what_is_not_a_target() {
        assert_eq!("".parse::<Target>(), Err(TargetError::Empty));
        assert_eq!("   ".parse::<Target>(), Err(TargetError::Empty));
        assert!(matches!(
            "flowcpt/".parse::<Target>(),
            Err(TargetError::EmptyLevel { .. })
        ));
        assert!(matches!(
            "flowcpt//pod".parse::<Target>(),
            Err(TargetError::EmptyLevel { .. })
        ));
        assert!(matches!(
            "/flowcpt".parse::<Target>(),
            Err(TargetError::EmptyLevel { .. })
        ));
        assert!(matches!(
            "a/b/c/d/e".parse::<Target>(),
            Err(TargetError::TooManyLevels { levels: 5, .. })
        ));
        assert!(matches!(
            "flowcpt/pod:nightly".parse::<Target>(),
            Err(TargetError::UnknownKind { .. })
        ));
        assert!(matches!(
            "flowcpt/cronjob:".parse::<Target>(),
            Err(TargetError::BadPattern { .. })
        ));
        assert!(matches!(
            "flowcpt/api/job:x".parse::<Target>(),
            Err(TargetError::BadPattern { .. })
        ));
        assert!(matches!(
            "flowcpt/flowcpt api".parse::<Target>(),
            Err(TargetError::BadPattern { .. })
        ));
        assert!(matches!(
            "flowcpt/api-[".parse::<Target>(),
            Err(TargetError::BadPattern { .. })
        ));
    }

    #[test]
    fn display_writes_the_shortest_path_that_parses_back() {
        for (input, shown) in [
            ("flowcpt", "flowcpt"),
            ("flowcpt/*", "flowcpt"),
            ("flowcpt/-", "flowcpt"),
            ("flowcpt/*/*/*", "flowcpt"),
            ("flowcpt/flowcpt-api", "flowcpt/flowcpt-api"),
            ("flowcpt/flowcpt-api/*", "flowcpt/flowcpt-api"),
            (
                "flowcpt/CronJob:nightly-sync",
                "flowcpt/cronjob:nightly-sync",
            ),
            (
                "flowcpt/*/flowcpt-api-7d9f-xk2p1",
                "flowcpt/-/flowcpt-api-7d9f-xk2p1",
            ),
            (
                "monitoring/prometheus/*/exporter",
                "monitoring/prometheus/*/exporter",
            ),
            ("flowcpt/-/*/api", "flowcpt/-/*/api"),
        ] {
            let parsed = target(input);
            assert_eq!(parsed.to_string(), shown, "display of `{input}`");
            assert_eq!(target(shown), parsed, "`{shown}` parses back");
        }
    }

    #[test]
    fn a_target_is_a_string_in_serde() {
        let parsed = target("flowcpt/flowcpt-api");
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            r#""flowcpt/flowcpt-api""#
        );
        assert_eq!(
            serde_json::from_str::<Target>(r#""flowcpt/flowcpt-api""#).unwrap(),
            parsed
        );
        assert!(serde_json::from_str::<Target>(r#""a/b/c/d/e""#).is_err());
    }
}
