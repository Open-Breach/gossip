//! The handle of a session against the stand-in API server: what comes out
//! of the stream when the filter, the targets, the events and the container
//! glob are changed while the session runs.

use std::time::Duration;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use tokio::time::timeout;

use super::*;
use crate::record::{Kind, Level};
use crate::testing::{
    FakeCluster, deployments, event_object, events, owner, pod_object, pods, replica_sets,
    workload_object,
};

const NS: &str = "fc-stg";
const API: &str = "fc-stg/fc-api";
const WEB: &str = "fc-stg/fc-web-api";
const API_POD_A: &str = "fc-api-7f698fb95f-vxfgs";
const API_POD_B: &str = "fc-api-7f698fb95f-k8m2p";
const WEB_POD: &str = "fc-web-api-5c8b6d7f9-m4q8z";

/// A time `millis` after a fixed moment in the past.
fn at(millis: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 18, 14, 0, 0).unwrap() + TimeDelta::milliseconds(millis)
}

fn target(text: &str) -> Target {
    text.parse().unwrap()
}

/// Options with a short hold, so that the tests need not wait for it, and
/// without events unless a test turns them on.
fn options() -> SessionOptions {
    SessionOptions {
        hold: Duration::from_millis(20),
        events: false,
        ..SessionOptions::default()
    }
}

/// The deployment `fc-api` with two pods of one container, and the
/// deployment `fc-web-api` with one pod of two.
fn seed(fake: &FakeCluster) {
    for (deployment, set, pod_names, containers) in [
        (
            "fc-api",
            "fc-api-7f698fb95f",
            &[API_POD_A, API_POD_B][..],
            &["api"][..],
        ),
        (
            "fc-web-api",
            "fc-web-api-5c8b6d7f9",
            &[WEB_POD],
            &["api", "envoy"],
        ),
    ] {
        let labels = [("app", deployment)];
        fake.apply(
            &deployments(NS),
            workload_object(NS, deployment, &labels, None),
        );
        fake.apply(
            &replica_sets(NS),
            workload_object(NS, set, &labels, Some(owner("Deployment", deployment))),
        );
        for pod in pod_names {
            fake.apply(
                &pods(NS),
                pod_object(NS, pod, &labels, Some(owner("ReplicaSet", set)), containers),
            );
        }
    }
}

fn start(fake: &FakeCluster, targets: &[&str], options: SessionOptions) -> Session {
    let targets = targets.iter().map(|text| target(text)).collect();
    Session::start(&fake.cluster(), targets, None, options)
}

async fn next(session: &mut Session) -> Record {
    match timeout(Duration::from_secs(10), session.next()).await {
        Ok(Some(record)) => record,
        Ok(None) => panic!("the session ended"),
        Err(_) => panic!("no record came in time"),
    }
}

/// A record as a short text: the message of a line, `-- ` and the message
/// of a notice, `event ` and the reason of an event.
fn text(record: &Record) -> String {
    match record.kind {
        Kind::Notice => format!("-- {}", record.message),
        Kind::Event => format!("event {}", record.raw),
        Kind::Line => record.message.clone(),
    }
}

/// The next records as short texts, in the order they came.
async fn take(session: &mut Session, count: usize) -> Vec<String> {
    let mut texts = Vec::new();
    for _ in 0..count {
        texts.push(text(&next(session).await));
    }
    texts
}

/// The next records as short texts, sorted: for records that come from
/// different sources at about the same time.
async fn take_sorted(session: &mut Session, count: usize) -> Vec<String> {
    let mut texts = take(session, count).await;
    texts.sort();
    texts
}

async fn expect_quiet(session: &mut Session) {
    if let Ok(record) = timeout(Duration::from_millis(400), session.next()).await {
        panic!("expected nothing more, got {:?}", record.as_ref().map(text));
    }
}

/// What the handle says is followed: every target with its pods.
fn followed(handle: &SessionHandle) -> Vec<(String, usize)> {
    handle
        .targets()
        .into_iter()
        .map(|status| (status.target.to_string(), status.pods))
        .collect()
}

#[tokio::test]
async fn a_new_filter_takes_effect_on_the_next_record_and_is_answered_with_a_notice() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    fake.log(NS, API_POD_A, "api", at(0), "INFO starting");
    let mut session = start(&fake, &[API], options());
    let handle = session.handle();
    assert_eq!(take(&mut session, 1).await, ["INFO starting"]);
    assert_eq!(handle.spec().filter, None);

    // Acceptance 2: one notice, and the info lines are hidden from the next
    // record on.
    handle.set_filter(Some(Filter::level_at_least(Level::Warn)));
    assert_eq!(take(&mut session, 1).await, ["-- filter: level >= warn"]);
    assert_eq!(
        handle.spec().filter,
        Some(Filter::level_at_least(Level::Warn))
    );
    fake.log(NS, API_POD_A, "api", at(1_000), "INFO request done");
    fake.log(NS, API_POD_A, "api", at(1_100), "WARN payment expired");
    assert_eq!(take(&mut session, 1).await, ["WARN payment expired"]);
    expect_quiet(&mut session).await;

    // Without a filter they are back.
    handle.set_filter(None);
    assert_eq!(take(&mut session, 1).await, ["-- filter off"]);
    assert_eq!(handle.spec().filter, None);
    fake.log(NS, API_POD_A, "api", at(2_000), "INFO request done");
    assert_eq!(take(&mut session, 1).await, ["INFO request done"]);
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn a_target_that_is_added_brings_its_lines_in_and_nothing_is_read_twice() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    fake.log(NS, API_POD_A, "api", at(0), "api one");
    fake.log(NS, API_POD_B, "api", at(100), "api two");
    for (index, line) in ["web one", "web two", "web three"].iter().enumerate() {
        fake.log(NS, WEB_POD, "api", at(200 + index as i64 * 100), line);
    }
    let mut session = start(&fake, &[API], options());
    let handle = session.handle();
    assert_eq!(take(&mut session, 2).await, ["api one", "api two"]);
    assert_eq!(followed(&handle), [(API.to_string(), 2)]);

    // Acceptance 3: the second deployment's lines come in without a restart,
    // with their history, and the notice says what is followed now. The
    // lines of the first are not read again.
    handle.add_target(target(WEB));
    assert_eq!(
        take_sorted(&mut session, 4).await,
        [
            "-- added fc-stg/fc-web-api; following fc-stg/fc-api (2 pods), fc-stg/fc-web-api (1 pod)",
            "web one",
            "web three",
            "web two",
        ]
    );
    expect_quiet(&mut session).await;
    assert_eq!(
        followed(&handle),
        [(API.to_string(), 2), (WEB.to_string(), 1)]
    );
    assert_eq!(handle.spec().targets, [target(API), target(WEB)]);

    // Both are followed from here on, each line once.
    fake.log(NS, API_POD_A, "api", at(5_000), "api three");
    fake.log(NS, WEB_POD, "envoy", at(5_100), "envoy one");
    assert_eq!(
        take_sorted(&mut session, 2).await,
        ["api three", "envoy one"]
    );
    expect_quiet(&mut session).await;

    // A target that is followed already is left as it is.
    handle.add_target(target(WEB));
    assert_eq!(
        take(&mut session, 1).await,
        ["-- fc-stg/fc-web-api is followed already"]
    );
    expect_quiet(&mut session).await;
    assert_eq!(handle.spec().targets.len(), 2);
}

#[tokio::test]
async fn a_target_that_is_dropped_closes_only_the_streams_no_other_target_still_matches() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    // The first pod of the deployment is also followed by a target of its
    // own.
    let pod_target = format!("fc-stg/-/{API_POD_A}");
    let mut session = start(&fake, &[API, &pod_target, WEB], options());
    let handle = session.handle();
    expect_quiet(&mut session).await;
    assert_eq!(
        followed(&handle),
        [
            (API.to_string(), 2),
            (pod_target.clone(), 1),
            (WEB.to_string(), 1)
        ]
    );

    handle.remove_target(target(API));
    // The pod that only the deployment selected is let go with a notice;
    // then the change is answered.
    assert_eq!(
        take(&mut session, 2).await,
        [
            format!("-- pod {API_POD_B} is no longer selected by a target"),
            format!(
                "-- dropped fc-stg/fc-api; following {pod_target} (1 pod), fc-stg/fc-web-api (1 pod)"
            ),
        ]
    );
    assert_eq!(
        followed(&handle),
        [(pod_target.clone(), 1), (WEB.to_string(), 1)]
    );

    // The pod another target still matches kept its stream; the other one
    // is closed.
    fake.log(
        NS,
        API_POD_B,
        "api",
        at(1_000),
        "from the pod that was let go",
    );
    fake.log(NS, API_POD_A, "api", at(1_100), "from the pod that stayed");
    fake.log(NS, WEB_POD, "api", at(1_200), "from the other deployment");
    assert_eq!(
        take_sorted(&mut session, 2).await,
        ["from the other deployment", "from the pod that stayed"]
    );
    expect_quiet(&mut session).await;

    // A target that is not followed changes nothing.
    handle.remove_target(target("fc-stg/fc-app"));
    assert_eq!(
        take(&mut session, 1).await,
        [format!(
            "-- fc-stg/fc-app is not followed; following {pod_target} (1 pod), fc-stg/fc-web-api (1 pod)"
        )]
    );

    // The last targets can go too; then nothing is followed.
    handle.remove_target(target(&pod_target));
    handle.remove_target(target(WEB));
    let mut answers = take(&mut session, 4).await;
    answers.sort();
    assert_eq!(
        answers,
        [
            format!("-- dropped {pod_target}; following nothing"),
            "-- dropped fc-stg/fc-web-api; following nothing".to_string(),
            format!("-- pod {API_POD_A} is no longer selected by a target"),
            format!("-- pod {WEB_POD} is no longer selected by a target"),
        ]
    );
    assert!(followed(&handle).is_empty());
    fake.log(NS, API_POD_A, "api", at(2_000), "nobody listens");
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn lines_that_were_received_before_a_target_was_dropped_are_still_emitted() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    // A hold long enough for the line to wait in the buffer while the
    // target goes.
    let held = SessionOptions {
        hold: Duration::from_millis(700),
        ..options()
    };
    fake.log(NS, API_POD_A, "api", at(0), "the first line");
    let mut session = start(&fake, &[API, WEB], held);
    let handle = session.handle();
    assert_eq!(take(&mut session, 1).await, ["the first line"]);

    // The line is received and waits in the buffer when its target goes.
    fake.log(NS, API_POD_A, "api", Utc::now(), "the last line");
    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.remove_target(target(API));
    let texts = take(&mut session, 4).await;
    assert_eq!(texts[0], "the last line");
    assert!(
        texts
            .contains(&"-- dropped fc-stg/fc-api; following fc-stg/fc-web-api (1 pod)".to_string()),
        "{texts:?}"
    );
}

/// Puts an event about a pod into the namespace.
fn add_event(fake: &FakeCluster, name: &str, pod: &str, reason: &str, time: DateTime<Utc>) {
    let event = event_object(
        NS,
        name,
        ("Pod", pod),
        Some("api"),
        reason,
        "as the kubelet says",
        "Normal",
        1,
        time,
    );
    fake.apply(&events(NS), event);
}

#[tokio::test]
async fn the_event_switch_starts_and_stops_the_event_watcher() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    let mut session = start(&fake, &[API], options());
    let handle = session.handle();
    assert_eq!(handle.spec().events, Some(false));
    // An event from before the switch, older than the moment around it.
    add_event(
        &fake,
        "pulled",
        API_POD_A,
        "Pulled",
        Utc::now() - TimeDelta::minutes(2),
    );
    expect_quiet(&mut session).await;

    handle.set_events(true);
    assert_eq!(take(&mut session, 1).await, ["-- events on"]);
    assert_eq!(handle.spec().events, Some(true));
    add_event(&fake, "started", API_POD_A, "Started", Utc::now());
    assert_eq!(
        take(&mut session, 1).await,
        ["event Started: as the kubelet says"]
    );
    // The streams stayed open through both changes.
    fake.log(NS, API_POD_A, "api", Utc::now(), "still here");
    assert_eq!(take(&mut session, 1).await, ["still here"]);

    handle.set_events(true);
    assert_eq!(take(&mut session, 1).await, ["-- events are on already"]);

    handle.set_events(false);
    assert_eq!(take(&mut session, 1).await, ["-- events off"]);
    assert_eq!(handle.spec().events, Some(false));
    add_event(&fake, "killing", API_POD_A, "Killing", Utc::now());
    expect_quiet(&mut session).await;
    fake.log(NS, API_POD_A, "api", Utc::now(), "and still here");
    assert_eq!(take(&mut session, 1).await, ["and still here"]);
}

#[tokio::test]
async fn the_events_of_a_dropped_target_stop_and_no_event_is_told_twice() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    // With a history of an hour, the events that are there are told at the
    // start, and listed again by every resolver that takes over.
    let with_history = SessionOptions {
        events: true,
        since: Some(Duration::from_secs(3600)),
        tail: None,
        ..options()
    };
    add_event(
        &fake,
        "started-a",
        API_POD_A,
        "Started",
        Utc::now() - TimeDelta::minutes(10),
    );
    let mut session = start(&fake, &[API], with_history);
    let handle = session.handle();
    assert_eq!(
        take(&mut session, 1).await,
        ["event Started: as the kubelet says"]
    );

    // The event of the first target is not told again; the one of the
    // target that is new is told, as far back as the history reaches.
    add_event(
        &fake,
        "pulled-web",
        WEB_POD,
        "Pulled",
        Utc::now() - TimeDelta::minutes(5),
    );
    handle.add_target(target(WEB));
    assert_eq!(
        take_sorted(&mut session, 2).await,
        [
            "-- added fc-stg/fc-web-api; following fc-stg/fc-api (2 pods), fc-stg/fc-web-api (1 pod)",
            "event Pulled: as the kubelet says",
        ]
    );
    expect_quiet(&mut session).await;

    // Acceptance 3: dropping the first target stops its pods' events.
    handle.remove_target(target(API));
    let mut answers = take(&mut session, 3).await;
    answers.sort();
    assert_eq!(
        answers,
        [
            "-- dropped fc-stg/fc-api; following fc-stg/fc-web-api (1 pod)".to_string(),
            format!("-- pod {API_POD_B} is no longer selected by a target"),
            format!("-- pod {API_POD_A} is no longer selected by a target"),
        ]
    );
    add_event(&fake, "killing-a", API_POD_A, "Killing", Utc::now());
    expect_quiet(&mut session).await;
    add_event(&fake, "killing-web", WEB_POD, "Killing", Utc::now());
    assert_eq!(
        take(&mut session, 1).await,
        ["event Killing: as the kubelet says"]
    );
}

#[tokio::test]
async fn the_container_glob_is_applied_again_to_every_followed_pod() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    fake.log(NS, WEB_POD, "envoy", at(0), "envoy one");
    let mut session = start(&fake, &[WEB], options());
    let handle = session.handle();
    assert_eq!(take(&mut session, 1).await, ["envoy one"]);
    assert_eq!(handle.spec().container, None);

    // The sidecar goes; its pod is still followed, so no pod is let go.
    let api = Pattern::new("api").unwrap();
    handle.set_container(Some(api.clone()));
    assert_eq!(
        take(&mut session, 1).await,
        ["-- containers matching api; following fc-stg/fc-web-api (1 pod)"]
    );
    assert_eq!(handle.spec().container, Some(api.clone()));
    fake.log(NS, WEB_POD, "envoy", at(1_000), "envoy two");
    fake.log(NS, WEB_POD, "api", at(1_100), "api one");
    assert_eq!(take(&mut session, 1).await, ["api one"]);
    expect_quiet(&mut session).await;

    handle.set_container(Some(api));
    assert_eq!(
        take(&mut session, 1).await,
        ["-- containers matching api, as before"]
    );

    // Without a glob the sidecar is followed again, from its history: its
    // stream was closed, and is opened like any new one.
    handle.set_container(None);
    assert_eq!(
        take_sorted(&mut session, 3).await,
        [
            "-- every container; following fc-stg/fc-web-api (1 pod)",
            "envoy one",
            "envoy two",
        ]
    );
    assert_eq!(handle.spec().container, None);
    // The container that stayed was not read again.
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn loading_a_spec_replaces_the_session_in_one_step() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    fake.log(NS, WEB_POD, "api", at(0), "INFO web one");
    fake.log(NS, WEB_POD, "api", at(100), "WARN web two");
    let mut session = start(&fake, &[API], options());
    let handle = session.handle();
    expect_quiet(&mut session).await;

    // Acceptance 9: the old pods' streams close with a notice each, the new
    // ones open, and the filter of the spec applies to what they bring.
    let spec = SessionSpec {
        targets: vec![target(WEB)],
        container: Some(Pattern::new("api").unwrap()),
        filter: Some(Filter::level_at_least(Level::Warn)),
        events: Some(false),
        tail: Some(5),
        ..SessionSpec::default()
    };
    handle.load(spec.clone());
    assert_eq!(
        take_sorted(&mut session, 4).await,
        [
            format!("-- pod {API_POD_B} is no longer selected by a target"),
            format!("-- pod {API_POD_A} is no longer selected by a target"),
            "-- session loaded; following fc-stg/fc-web-api (1 pod)".to_string(),
            "WARN web two".to_string(),
        ]
    );
    expect_quiet(&mut session).await;
    assert_eq!(followed(&handle), [(WEB.to_string(), 1)]);

    // What the session follows now is what was loaded, with its cluster.
    let now = handle.spec();
    assert_eq!(
        now,
        SessionSpec {
            context: Some("fake".to_string()),
            ..spec.clone()
        }
    );
    fake.log(NS, API_POD_A, "api", at(1_000), "ERROR from the old target");
    fake.log(NS, WEB_POD, "envoy", at(1_100), "ERROR from the sidecar");
    fake.log(NS, WEB_POD, "api", at(1_200), "ERROR web three");
    assert_eq!(take(&mut session, 1).await, ["ERROR web three"]);
    expect_quiet(&mut session).await;

    // A spec that follows the same and differs in its filter resolves
    // nothing again.
    let requests = fake.requests().len();
    handle.load(SessionSpec {
        filter: None,
        ..spec.clone()
    });
    assert_eq!(
        take(&mut session, 1).await,
        ["-- session loaded; following fc-stg/fc-web-api (1 pod)"]
    );
    assert_eq!(handle.spec().filter, None);
    fake.log(NS, WEB_POD, "api", at(2_000), "INFO web four");
    assert_eq!(take(&mut session, 1).await, ["INFO web four"]);
    assert_eq!(fake.requests().len(), requests);

    // A spec of another cluster is refused, and nothing changes.
    handle.load(SessionSpec {
        context: Some("aks-vulcan-01".to_string()),
        targets: vec![target(API)],
        ..SessionSpec::default()
    });
    assert_eq!(
        take(&mut session, 1).await,
        [
            "-- cannot load a session of the cluster aks-vulcan-01: this one follows the cluster \
             fake; nothing changed"
        ]
    );
    assert_eq!(handle.spec().targets, [target(WEB)]);
    // One of this cluster is taken, and what it leaves out is the built-in
    // default: no filter, every container, the last 20 lines, events on.
    handle.load(SessionSpec {
        context: Some("fake".to_string()),
        targets: vec![target(WEB)],
        ..SessionSpec::default()
    });
    let texts = take_sorted(&mut session, 2).await;
    assert_eq!(
        texts[0],
        "-- session loaded; following fc-stg/fc-web-api (1 pod)"
    );
    assert_eq!(texts[1], "ERROR from the sidecar");
    let now = handle.spec();
    assert_eq!(now.container, None);
    assert_eq!(now.events, Some(true));
    assert_eq!((now.since, now.tail), (None, Some(20)));
}

#[tokio::test]
async fn the_spec_of_a_session_is_what_a_template_holds() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    let filter = Filter::field_equals("tenant_id", "t_123").unwrap();
    let session = Session::start(
        &fake.cluster(),
        vec![target(API), target(WEB)],
        Some(filter.clone()),
        SessionOptions {
            since: Some(Duration::from_secs(300)),
            tail: None,
            events: true,
            containers: Some(Pattern::new("api").unwrap()),
            ..options()
        },
    );
    let spec = session.handle().spec();
    assert_eq!(
        spec,
        SessionSpec {
            context: Some("fake".to_string()),
            targets: vec![target(API), target(WEB)],
            container: Some(Pattern::new("api").unwrap()),
            since: Some(Duration::from_secs(300)),
            tail: None,
            events: Some(true),
            filter: Some(filter),
            ..SessionSpec::default()
        }
    );

    // It goes into a config file as a template and comes back the same.
    let config = crate::Config {
        templates: BTreeMap::from([("saved".to_string(), spec.clone())]),
        ..crate::Config::default()
    };
    let text = config.to_toml().unwrap();
    let read_back = crate::Config::from_toml(&text).unwrap();
    assert_eq!(read_back.templates["saved"], spec, "{text}");
}

#[tokio::test]
async fn stopping_through_the_handle_ends_the_stream_and_closes_everything() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    fake.log(NS, API_POD_A, "api", at(0), "one");
    let mut session = start(&fake, &[API], options());
    let handle = session.handle();
    assert_eq!(take(&mut session, 1).await, ["one"]);
    assert!(!handle.is_stopped());

    handle.stop();
    let end = timeout(Duration::from_secs(10), session.next()).await;
    assert!(matches!(end, Ok(None)), "the stream should have ended");

    // With the session gone, nobody asks the API server for anything any
    // more, and a change is dropped without a word.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(handle.is_stopped());
    let before = fake.requests().len();
    handle.add_target(target(WEB));
    fake.apply(&pods(NS), pod_object(NS, "late", &[], None, &["main"]));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fake.requests().len(), before);
}

#[tokio::test]
async fn changes_from_another_task_are_applied_in_the_order_they_were_asked() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    let mut session = start(&fake, &[API], options());
    let handle = session.handle();
    expect_quiet(&mut session).await;

    let from_elsewhere = handle.clone();
    tokio::spawn(async move {
        from_elsewhere.add_target(target(WEB));
        from_elsewhere.add_target(target("fc-stg/fc-app"));
        from_elsewhere.remove_target(target(API));
        from_elsewhere.set_filter(Some(Filter::message_contains("payment")));
    })
    .await
    .unwrap();

    // The filter is answered at once; the three changes of what is followed
    // are answered when the last of them is done, in their order.
    let texts = take(&mut session, 7).await;
    let answers: Vec<&String> = texts
        .iter()
        .filter(|text| !text.starts_with("-- pod ") && !text.starts_with("-- target "))
        .collect();
    let following = "following fc-stg/fc-web-api (1 pod), fc-stg/fc-app (0 pods)";
    assert_eq!(
        answers,
        [
            "-- filter: message contains payment",
            &format!("-- added fc-stg/fc-web-api; {following}"),
            &format!("-- added fc-stg/fc-app; {following}"),
            &format!("-- dropped fc-stg/fc-api; {following}"),
        ]
    );
    // The pods of the dropped target went, and the target that is new and
    // matches nothing says so, as at the start.
    assert!(
        texts.contains(&"-- target fc-stg/fc-app matches nothing yet; still watching".to_string())
    );
    expect_quiet(&mut session).await;
    assert_eq!(
        handle.spec().targets,
        [target(WEB), target("fc-stg/fc-app")]
    );
}

#[tokio::test]
async fn a_resolver_that_takes_over_does_not_repeat_what_was_said_about_a_target() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    let mut session = start(&fake, &["fc-stg/fc-app"], options());
    let handle = session.handle();
    assert_eq!(
        take(&mut session, 1).await,
        ["-- target fc-stg/fc-app matches nothing yet; still watching"]
    );

    // The target still matches nothing, and that was said.
    handle.add_target(target(API));
    assert_eq!(
        take(&mut session, 1).await,
        ["-- added fc-stg/fc-api; following fc-stg/fc-app (0 pods), fc-stg/fc-api (2 pods)"]
    );
    expect_quiet(&mut session).await;

    // When its pod comes up, it is followed like any pod that comes late.
    let labels = [("app", "fc-app")];
    fake.apply(
        &deployments(NS),
        workload_object(NS, "fc-app", &labels, None),
    );
    fake.apply(
        &replica_sets(NS),
        workload_object(
            NS,
            "fc-app-6d5c7b8f4",
            &labels,
            Some(owner("Deployment", "fc-app")),
        ),
    );
    fake.apply(
        &pods(NS),
        pod_object(
            NS,
            "fc-app-6d5c7b8f4-q7w2x",
            &labels,
            Some(owner("ReplicaSet", "fc-app-6d5c7b8f4")),
            &["app"],
        ),
    );
    assert_eq!(
        take(&mut session, 1).await,
        ["-- pod fc-app-6d5c7b8f4-q7w2x appeared; following app"]
    );
    // The handle counts it once the resolver has reported it.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        followed(&handle),
        [("fc-stg/fc-app".to_string(), 1), (API.to_string(), 2)]
    );
}

#[tokio::test]
async fn a_session_nobody_touches_says_nothing_about_itself() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    fake.log(NS, API_POD_A, "api", at(0), "one");
    let mut session = start(&fake, &[API, WEB], options());
    let handle = session.handle();
    assert_eq!(take(&mut session, 1).await, ["one"]);
    expect_quiet(&mut session).await;
    // Reading what it follows is not touching it.
    assert_eq!(
        followed(&handle),
        [(API.to_string(), 2), (WEB.to_string(), 1)]
    );
    assert_eq!(handle.spec().targets.len(), 2);
    expect_quiet(&mut session).await;
}
