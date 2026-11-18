//! Core library of Gossip.
//!
//! Gossip follows the logs of Kubernetes workloads across pod restarts and
//! rollouts. This crate holds everything that knows about Kubernetes,
//! records, filters and the config file; front ends such as the `gossip`
//! command line only present what it produces.
//!
//! Two rules hold for everything in here: the core prints nothing, and it
//! only ever reads from the cluster.
//!
//! What talks to a cluster so far:
//!
//! - [`cluster`]: the contexts of the kubeconfig, and a client for one.
//! - [`discovery`]: the namespaces, deployments and pods of a cluster.
//! - [`resolver`]: from targets to the containers to follow, kept up to date
//!   by watching the cluster.
//!
//! And the logic that needs no cluster:
//!
//! - [`target`]: targets, the paths `namespace[/workload[/pod[/container]]]`
//!   whose names may be globs.
//! - [`record`]: the record, the one type for everything a stream carries.
//! - [`parser`]: the line parser, which recognises JSON, logfmt and text per
//!   line and joins continuation lines.
//! - [`filter`]: filters, trees of rules applied to records.
//! - [`config`]: the config file with its defaults and templates.
//! - [`duration`]: durations as the command line and the config write them.

pub mod cluster;
pub mod config;
pub mod discovery;
pub mod duration;
pub mod error;
pub mod filter;
pub mod parser;
pub mod record;
pub mod resolver;
pub mod target;
#[cfg(test)]
pub(crate) mod testing;

pub use cluster::{Cluster, ContextInfo};
pub use config::{Config, ConfigError, Defaults, FormatSetting, Output, Settings, Template};
pub use discovery::{ContainerInfo, DeploymentInfo, NamespaceInfo, PodInfo};
pub use duration::{DurationError, format_duration, parse_duration};
pub use error::Error;
pub use filter::{Field, Filter, FilterError, Op, Rule};
pub use parser::{FormatChoice, LineParser, ParsedLine};
pub use record::{Format, Kind, Level, Record, Value};
pub use resolver::{ContainerEvent, ContainerId, Resolver, ResolverEvent};
pub use target::{Pattern, Target, TargetError, WorkloadKind, WorkloadSelector};
