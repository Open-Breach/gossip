//! The resolver against the stand-in API server: the watches it really
//! starts, and what it reports when the objects behind them change.

use std::time::Duration;

use tokio::time::timeout;

use super::*;
use crate::testing::{
    FakeCluster, cron_job_object, cron_jobs, deployments, jobs, namespace_object, namespaces,
    owner, pod_object, pods, replica_sets, stateful_sets, workload_object,
};

const NS: &str = "flowcpt";

fn target(text: &str) -> Target {
    text.parse().unwrap()
}

/// An event as a short text: `+pod/container` for a container to follow
/// (with `!` when it was there from the start), `-pod/container` for one
/// that is gone, `~pod/container` for a new state, and the message of a
/// notice.
fn short(event: &ResolverEvent) -> String {
    match event {
        ResolverEvent::Container(ContainerEvent::Added { id, initial, .. }) => {
            format!(
                "+{}/{}{}",
                id.pod,
                id.container,
                if *initial { "!" } else { "" }
            )
        }
        ResolverEvent::Container(ContainerEvent::Changed { id, .. }) => {
            format!("~{}/{}", id.pod, id.container)
        }
        ResolverEvent::Container(ContainerEvent::Removed { id }) => {
            format!("-{}/{}", id.pod, id.container)
        }
        ResolverEvent::Cluster(event) => format!("event {}: {}", event.reason, event.message),
        ResolverEvent::Notice(notice) => notice.message.clone(),
        ResolverEvent::Matched { namespace, pods } => {
            let counts: Vec<String> = pods
                .iter()
                .map(|(target, pods)| format!("{target}:{pods}"))
                .collect();
            format!("matched {namespace} {}", counts.join(" "))
        }
        ResolverEvent::Settled => "settled".to_string(),
    }
}

/// Whether an event is one of the reports about the resolution itself: how
/// many pods each target follows, and that the first picture is complete.
/// The tests of what is followed leave them out; they have tests of their own.
fn is_report(event: &ResolverEvent) -> bool {
    matches!(
        event,
        ResolverEvent::Matched { .. } | ResolverEvent::Settled
    )
}

/// Waits for exactly these events, in any order.
async fn expect(resolver: &mut Resolver, expected: &[&str]) {
    let mut seen = Vec::new();
    while seen.len() < expected.len() {
        match timeout(Duration::from_secs(10), resolver.next()).await {
            Ok(Some(event)) if is_report(&event) => {}
            Ok(Some(event)) => seen.push(short(&event)),
            Ok(None) => panic!("the resolver stopped after {seen:?}"),
            Err(_) => panic!("waited for {expected:?} and got only {seen:?}"),
        }
    }
    let mut expected: Vec<String> = expected.iter().map(|text| text.to_string()).collect();
    expected.sort();
    seen.sort();
    assert_eq!(seen, expected);
}

/// Checks that the resolver has nothing more to report for a moment.
async fn expect_quiet(resolver: &mut Resolver) {
    let quiet = tokio::time::Instant::now() + Duration::from_millis(300);
    while let Ok(event) = tokio::time::timeout_at(quiet, resolver.next()).await {
        match event {
            Some(event) if is_report(&event) => {}
            other => panic!("expected nothing more, got {:?}", other.as_ref().map(short)),
        }
    }
}

/// Waits for the next report about the resolution itself, leaving out
/// everything else.
async fn next_report(resolver: &mut Resolver) -> String {
    loop {
        match timeout(Duration::from_secs(10), resolver.next()).await {
            Ok(Some(event)) if is_report(&event) => return short(&event),
            Ok(Some(_)) => {}
            Ok(None) => panic!("the resolver stopped"),
            Err(_) => panic!("no report came in time"),
        }
    }
}

/// The deployment `flowcpt-api` with one replica set and two pods, next to
/// a stateful set with one pod.
fn seed(fake: &FakeCluster) {
    let api = [("app", "flowcpt-api")];
    fake.apply(
        &deployments(NS),
        workload_object(NS, "flowcpt-api", &api, None),
    );
    fake.apply(
        &replica_sets(NS),
        workload_object(
            NS,
            "flowcpt-api-7d9f",
            &api,
            Some(owner("Deployment", "flowcpt-api")),
        ),
    );
    for name in ["flowcpt-api-7d9f-xk2p1", "flowcpt-api-7d9f-m4q8z"] {
        let set = owner("ReplicaSet", "flowcpt-api-7d9f");
        fake.apply(
            &pods(NS),
            pod_object(NS, name, &api, Some(set), &["api", "envoy"]),
        );
    }
    let redis = [("app", "redis")];
    fake.apply(
        &stateful_sets(NS),
        workload_object(NS, "redis", &redis, None),
    );
    fake.apply(
        &pods(NS),
        pod_object(
            NS,
            "redis-0",
            &redis,
            Some(owner("StatefulSet", "redis")),
            &["redis"],
        ),
    );
}

#[tokio::test]
async fn a_deployment_is_followed_through_a_rollout() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    let mut resolver = Resolver::start(
        &fake.cluster(),
        vec![target("flowcpt/flowcpt-api")],
        None,
        None,
    );

    expect(
        &mut resolver,
        &[
            "+flowcpt-api-7d9f-xk2p1/api!",
            "+flowcpt-api-7d9f-xk2p1/envoy!",
            "+flowcpt-api-7d9f-m4q8z/api!",
            "+flowcpt-api-7d9f-m4q8z/envoy!",
        ],
    )
    .await;
    expect_quiet(&mut resolver).await;

    // The pod watch asks the API server for the labels of the deployment,
    // and the deployment itself is watched by its name.
    let requests = fake.requests();
    assert!(
        requests.iter().any(|request| request.starts_with(&pods(NS))
            && request.contains("labelSelector=app=flowcpt-api")),
        "{requests:#?}"
    );
    assert!(
        requests
            .iter()
            .any(|request| request.starts_with(&deployments(NS))
                && request.contains("fieldSelector=metadata.name=flowcpt-api")),
        "{requests:#?}"
    );
    assert!(
        !requests
            .iter()
            .any(|request| request.starts_with(&pods(NS)) && !request.contains("labelSelector")),
        "all pods were asked for: {requests:#?}"
    );

    // A rollout: a new replica set, a new pod, then an old pod goes.
    let api = [("app", "flowcpt-api")];
    fake.apply(
        &replica_sets(NS),
        workload_object(
            NS,
            "flowcpt-api-5c8b",
            &api,
            Some(owner("Deployment", "flowcpt-api")),
        ),
    );
    fake.apply(
        &pods(NS),
        pod_object(
            NS,
            "flowcpt-api-5c8b-abcde",
            &api,
            Some(owner("ReplicaSet", "flowcpt-api-5c8b")),
            &["api", "envoy"],
        ),
    );
    expect(
        &mut resolver,
        &[
            "+flowcpt-api-5c8b-abcde/api",
            "+flowcpt-api-5c8b-abcde/envoy",
            "pod flowcpt-api-5c8b-abcde appeared; following api, envoy",
        ],
    )
    .await;

    fake.delete(&pods(NS), "flowcpt-api-7d9f-xk2p1");
    expect(
        &mut resolver,
        &[
            "-flowcpt-api-7d9f-xk2p1/api",
            "-flowcpt-api-7d9f-xk2p1/envoy",
            "pod flowcpt-api-7d9f-xk2p1 went away",
        ],
    )
    .await;
    expect_quiet(&mut resolver).await;
}

#[tokio::test]
async fn a_target_that_matches_nothing_says_so_and_keeps_watching() {
    let fake = FakeCluster::start().await;
    // The deployment is there, scaled to zero.
    let api = [("app", "flowcpt-api")];
    fake.apply(
        &deployments(NS),
        workload_object(NS, "flowcpt-api", &api, None),
    );
    fake.apply(
        &replica_sets(NS),
        workload_object(
            NS,
            "flowcpt-api-7d9f",
            &api,
            Some(owner("Deployment", "flowcpt-api")),
        ),
    );
    let mut resolver = Resolver::start(
        &fake.cluster(),
        vec![target("flowcpt/flowcpt-api")],
        None,
        None,
    );
    expect(
        &mut resolver,
        &["target flowcpt/flowcpt-api matches nothing yet; still watching"],
    )
    .await;

    // It is scaled up.
    fake.apply(
        &pods(NS),
        pod_object(
            NS,
            "flowcpt-api-7d9f-xk2p1",
            &api,
            Some(owner("ReplicaSet", "flowcpt-api-7d9f")),
            &["api"],
        ),
    );
    expect(
        &mut resolver,
        &[
            "+flowcpt-api-7d9f-xk2p1/api",
            "pod flowcpt-api-7d9f-xk2p1 appeared; following api",
        ],
    )
    .await;
}

#[tokio::test]
async fn a_bare_namespace_follows_every_pod_and_the_container_option_narrows_it() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    let containers = Pattern::new("{api,redis}").unwrap();
    let mut resolver = Resolver::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        Some(containers),
        None,
    );
    expect(
        &mut resolver,
        &[
            "+flowcpt-api-7d9f-xk2p1/api!",
            "+flowcpt-api-7d9f-m4q8z/api!",
            "+redis-0/redis!",
        ],
    )
    .await;
    expect_quiet(&mut resolver).await;

    // One watch on all pods, and none on any workload.
    let requests = fake.requests();
    assert!(
        requests
            .iter()
            .all(|request| request.starts_with(&pods(NS))),
        "{requests:#?}"
    );
    assert!(
        requests
            .iter()
            .all(|request| !request.contains("labelSelector")),
        "{requests:#?}"
    );
}

#[tokio::test]
async fn a_namespace_glob_follows_the_namespaces_it_matches_as_they_come() {
    let fake = FakeCluster::start().await;
    for name in ["flowcpt", "flowcpt-stg", "monitoring"] {
        fake.apply(&namespaces(), namespace_object(name));
        fake.apply(
            &pods(name),
            pod_object(name, &format!("{name}-pod"), &[], None, &["main"]),
        );
    }
    let mut resolver = Resolver::start(&fake.cluster(), vec![target("flowcpt*")], None, None);
    expect(
        &mut resolver,
        &["+flowcpt-pod/main!", "+flowcpt-stg-pod/main!"],
    )
    .await;
    expect_quiet(&mut resolver).await;

    // A namespace that is made while the session runs.
    fake.apply(
        &pods("flowcpt-dev"),
        pod_object("flowcpt-dev", "late", &[], None, &["main"]),
    );
    fake.apply(&namespaces(), namespace_object("flowcpt-dev"));
    expect(
        &mut resolver,
        &["+late/main", "pod late appeared; following main"],
    )
    .await;
}

#[tokio::test]
async fn a_namespace_glob_that_matches_no_namespace_says_so() {
    let fake = FakeCluster::start().await;
    fake.apply(&namespaces(), namespace_object("monitoring"));
    let mut resolver = Resolver::start(&fake.cluster(), vec![target("flowcpt*/api")], None, None);
    expect(
        &mut resolver,
        &["target flowcpt*/api matches nothing yet; still watching"],
    )
    .await;
}

#[tokio::test]
async fn every_run_of_a_cron_job_is_picked_up() {
    let fake = FakeCluster::start().await;
    fake.apply(&cron_jobs(NS), cron_job_object(NS, "nightly-sync"));
    let mut resolver = Resolver::start(
        &fake.cluster(),
        vec![target("flowcpt/cronjob:nightly-sync")],
        None,
        None,
    );
    expect(
        &mut resolver,
        &["target flowcpt/cronjob:nightly-sync matches nothing yet; still watching"],
    )
    .await;

    for run in ["nightly-sync-001", "nightly-sync-002"] {
        let labels = [("job-name", run)];
        fake.apply(
            &jobs(NS),
            workload_object(NS, run, &labels, Some(owner("CronJob", "nightly-sync"))),
        );
        let pod = format!("{run}-x7k2p");
        fake.apply(
            &pods(NS),
            pod_object(NS, &pod, &labels, Some(owner("Job", run)), &["sync"]),
        );
        expect(
            &mut resolver,
            &[
                &format!("+{pod}/sync"),
                &format!("pod {pod} appeared; following sync"),
            ],
        )
        .await;
    }
}

#[tokio::test]
async fn a_watch_that_is_refused_is_noticed_and_the_rest_goes_on() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    fake.forbid(&cron_jobs(NS));
    let mut resolver = Resolver::start(&fake.cluster(), vec![target("flowcpt/redis")], None, None);
    expect(
        &mut resolver,
        &[
            "+redis-0/redis!",
            "cannot watch the CronJob `redis` in the namespace flowcpt: cronjobs is forbidden: \
             User \"fake\" cannot list resource \"cronjobs\" (403); trying again",
        ],
    )
    .await;
}

#[tokio::test]
async fn dropping_the_resolver_stops_its_watches() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    let mut resolver = Resolver::start(&fake.cluster(), vec![target("flowcpt")], None, None);
    expect(
        &mut resolver,
        &[
            "+flowcpt-api-7d9f-xk2p1/api!",
            "+flowcpt-api-7d9f-xk2p1/envoy!",
            "+flowcpt-api-7d9f-m4q8z/api!",
            "+flowcpt-api-7d9f-m4q8z/envoy!",
            "+redis-0/redis!",
        ],
    )
    .await;
    drop(resolver);

    // With the watch gone, nobody asks the API server for anything any more.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let before = fake.requests().len();
    fake.apply(&pods(NS), pod_object(NS, "late", &[], None, &["main"]));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fake.requests().len(), before);
}

#[tokio::test]
async fn the_resolver_says_how_many_pods_each_target_follows_and_when_it_has_listed_them() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    let targets = vec![
        target("flowcpt/flowcpt-api"),
        target("flowcpt/nope"),
        target("flowcpt/redis"),
    ];
    let mut resolver = Resolver::start(&fake.cluster(), targets, None, None);

    // The counts change while the watches list; the last one before the
    // resolver has settled is the first full picture. The target in the
    // middle matches nothing and is left out.
    let mut reports = Vec::new();
    loop {
        let report = next_report(&mut resolver).await;
        if report == "settled" {
            break;
        }
        reports.push(report);
    }
    assert_eq!(
        reports.last().map(String::as_str),
        Some("matched flowcpt 0:2 2:1")
    );

    // A pod that comes and one that goes change the count; settling is said
    // once.
    let api = [("app", "flowcpt-api")];
    let set = owner("ReplicaSet", "flowcpt-api-7d9f");
    fake.apply(
        &pods(NS),
        pod_object(NS, "flowcpt-api-7d9f-q7w2x", &api, Some(set), &["api"]),
    );
    assert_eq!(next_report(&mut resolver).await, "matched flowcpt 0:3 2:1");
    fake.delete(&pods(NS), "redis-0");
    assert_eq!(next_report(&mut resolver).await, "matched flowcpt 0:3");
}

#[tokio::test]
async fn a_resolver_with_nothing_to_resolve_settles_at_once() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    let mut resolver = Resolver::start(&fake.cluster(), Vec::new(), None, None);
    assert_eq!(next_report(&mut resolver).await, "settled");
    expect_quiet(&mut resolver).await;
}
