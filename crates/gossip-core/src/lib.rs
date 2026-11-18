//! Core library of Gossip.
//!
//! Gossip follows the logs of Kubernetes workloads across pod restarts and
//! rollouts. This crate will hold everything that knows about Kubernetes,
//! records, filters and the config file; front ends such as the `gossip`
//! command line only present what it produces.
//!
//! Two rules hold for everything in here: the core prints nothing, and it
//! only ever reads from the cluster.
