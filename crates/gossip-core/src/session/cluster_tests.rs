//! Sessions against the stand-in API server: what comes out of the stream
//! when containers log, restart, lose their connection and go away.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use serde_json::{Value, json};
use tokio::time::timeout;

use super::*;
use crate::parser::FormatChoice;
use crate::record::{Format, Kind, Level};
use crate::testing::{FakeCluster, container_status, pod_with_statuses, pods};

const NS: &str = "flowcpt";
const POD: &str = "flowcpt-api-7d9f-xk2p1";

/// A time `millis` after a fixed moment in the past.
fn at(millis: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 18, 14, 0, 0).unwrap() + TimeDelta::milliseconds(millis)
}

fn target(text: &str) -> Target {
    text.parse().unwrap()
}

/// Options with a short hold, so that the tests need not wait for it.
fn options() -> SessionOptions {
    SessionOptions {
        hold: Duration::from_millis(20),
        ..SessionOptions::default()
    }
}

fn running() -> Value {
    json!({ "running": { "startedAt": "2026-11-18T13:59:00Z" } })
}

fn terminated(exit_code: i32) -> Value {
    json!({ "terminated": { "exitCode": exit_code, "reason": "Error" } })
}

fn crash_loop() -> Value {
    json!({ "waiting": {
        "reason": "CrashLoopBackOff",
        "message": "back-off 40s restarting failed container"
    } })
}

/// Puts a pod with one container `api` in this state into the namespace.
fn set_pod(fake: &FakeCluster, pod: &str, restarts: i32, state: Value, last_state: Option<Value>) {
    let status = container_status("api", restarts, state, last_state);
    fake.apply(&pods(NS), pod_with_statuses(NS, pod, vec![status]));
}

/// Writes lines to the container `api` of a pod, one every 100 ms from `start`.
fn log_lines(fake: &FakeCluster, pod: &str, start: i64, lines: &[&str]) {
    for (index, line) in lines.iter().enumerate() {
        fake.log(NS, pod, "api", at(start + index as i64 * 100), line);
    }
}

async fn next(session: &mut Session) -> Record {
    match timeout(Duration::from_secs(10), session.next()).await {
        Ok(Some(record)) => record,
        Ok(None) => panic!("the session ended"),
        Err(_) => panic!("no record came in time"),
    }
}

async fn take(session: &mut Session, count: usize) -> Vec<Record> {
    let mut records = Vec::new();
    for _ in 0..count {
        records.push(next(session).await);
    }
    records
}

/// Records as short texts: the message of a line, with `(previous) ` in
/// front when it is of a previous run, and `-- ` in front of a notice.
fn texts(records: &[Record]) -> Vec<String> {
    records
        .iter()
        .map(|record| match record.kind {
            Kind::Notice => format!("-- {}", record.message),
            _ if record.previous => format!("(previous) {}", record.message),
            _ => record.message.clone(),
        })
        .collect()
}

async fn expect_quiet(session: &mut Session) {
    if let Ok(record) = timeout(Duration::from_millis(400), session.next()).await {
        panic!(
            "expected nothing more, got {:?}",
            record.map(|record| record.message)
        );
    }
}

#[tokio::test]
async fn a_session_prints_the_tail_and_then_new_lines_as_they_arrive() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    let history: Vec<String> = (1..=30).map(|number| format!("line {number}")).collect();
    let history: Vec<&str> = history.iter().map(String::as_str).collect();
    log_lines(&fake, POD, 0, &history);

    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        None,
        SessionOptions {
            tail: Some(5),
            ..options()
        },
    );
    let records = take(&mut session, 5).await;
    assert_eq!(
        texts(&records),
        ["line 26", "line 27", "line 28", "line 29", "line 30"]
    );
    // Every record knows where it came from and when.
    let first = &records[0];
    assert_eq!(first.cluster, "fake");
    assert_eq!(first.namespace, NS);
    assert_eq!(first.pod, POD);
    assert_eq!(first.container.as_deref(), Some("api"));
    assert_eq!(first.time, at(2_500));
    assert_eq!(first.kind, Kind::Line);
    assert_eq!(first.format, Format::Text);
    assert_eq!(first.raw, "line 26");
    assert!(!first.previous && !first.truncated);
    expect_quiet(&mut session).await;

    log_lines(&fake, POD, 10_000, &["a new line", "and another"]);
    assert_eq!(
        texts(&take(&mut session, 2).await),
        ["a new line", "and another"]
    );

    // The log was asked for with the tail, the timestamps and follow.
    let requests = fake.requests();
    let log_request = requests
        .iter()
        .find(|request| request.contains("/log?"))
        .unwrap();
    for part in [
        "container=api",
        "follow=true",
        "tailLines=5",
        "timestamps=true",
    ] {
        assert!(log_request.contains(part), "{log_request}");
    }
}

#[tokio::test]
async fn since_takes_the_history_of_a_time_span() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    fake.log(
        NS,
        POD,
        "api",
        Utc::now() - TimeDelta::minutes(10),
        "long ago",
    );
    fake.log(
        NS,
        POD,
        "api",
        Utc::now() - TimeDelta::seconds(10),
        "just now",
    );

    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        None,
        SessionOptions {
            since: Some(Duration::from_secs(60)),
            tail: None,
            ..options()
        },
    );
    assert_eq!(texts(&take(&mut session, 1).await), ["just now"]);
    expect_quiet(&mut session).await;
    let requests = fake.requests();
    let log_request = requests
        .iter()
        .find(|request| request.contains("/log?"))
        .unwrap();
    assert!(log_request.contains("sinceSeconds=60"), "{log_request}");
    assert!(!log_request.contains("tailLines"), "{log_request}");
}

#[tokio::test]
async fn a_restart_is_followed_and_no_line_is_printed_twice() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    log_lines(&fake, POD, 0, &["first run, line 1", "first run, line 2"]);
    let mut session = Session::start(&fake.cluster(), vec![target("flowcpt")], None, options());
    assert_eq!(
        texts(&take(&mut session, 2).await),
        ["first run, line 1", "first run, line 2"]
    );

    // The container dies: its stream ends, and the pod says so.
    fake.end_run(NS, POD, "api");
    set_pod(&fake, POD, 0, terminated(137), None);
    expect_quiet(&mut session).await;

    // It is started again: the restart count goes up and a new log begins.
    fake.start_run(NS, POD, "api");
    log_lines(
        &fake,
        POD,
        5_000,
        &["second run, line 1", "second run, line 2"],
    );
    set_pod(&fake, POD, 1, running(), Some(terminated(137)));
    assert_eq!(
        texts(&take(&mut session, 2).await),
        ["second run, line 1", "second run, line 2"]
    );

    log_lines(&fake, POD, 6_000, &["second run, line 3"]);
    assert_eq!(texts(&take(&mut session, 1).await), ["second run, line 3"]);
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn a_dropped_connection_is_reopened_and_nothing_is_lost_or_repeated() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    // All within one second: the API can only be asked for whole seconds,
    // so the reopened stream sends these lines again.
    fake.log(NS, POD, "api", at(100), "before the drop, 1");
    fake.log(NS, POD, "api", at(200), "before the drop, 2");
    fake.log(NS, POD, "api", at(200), "before the drop, 3");
    let mut session = Session::start(&fake.cluster(), vec![target("flowcpt")], None, options());
    assert_eq!(texts(&take(&mut session, 3).await).len(), 3);

    fake.break_log_streams();
    fake.log(NS, POD, "api", at(200), "during the outage, same instant");
    fake.log(NS, POD, "api", at(300), "during the outage, same second");
    fake.log(NS, POD, "api", at(4_000), "during the outage, later");

    let records = take(&mut session, 4).await;
    let notice = &records[0];
    assert_eq!(notice.kind, Kind::Notice);
    assert!(
        notice
            .message
            .starts_with(&format!("lost the log stream of api in pod {POD}: ")),
        "{}",
        notice.message
    );
    assert!(
        notice.message.ends_with("; opening it again in 1 s"),
        "{}",
        notice.message
    );
    assert_eq!(notice.pod, POD);
    assert_eq!(notice.container.as_deref(), Some("api"));
    assert_eq!(
        texts(&records[1..]),
        [
            "during the outage, same instant",
            "during the outage, same second",
            "during the outage, later"
        ]
    );
    expect_quiet(&mut session).await;

    // The stream was opened again from the time of the last line seen.
    let requests = fake.requests();
    let reopened = requests
        .iter()
        .rfind(|request| request.contains("/log?"))
        .unwrap();
    assert!(
        reopened.contains("sinceTime=2026-11-18T14:00:00Z"),
        "{reopened}"
    );
    assert!(!reopened.contains("tailLines"), "{reopened}");
}

#[tokio::test]
async fn a_log_that_cannot_be_opened_is_tried_again_with_a_notice() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    log_lines(&fake, POD, 0, &["there all along"]);
    fake.fail_log_requests(1);
    let mut session = Session::start(&fake.cluster(), vec![target("flowcpt")], None, options());
    assert_eq!(
        texts(&take(&mut session, 2).await),
        [
            format!(
                "-- cannot open the log stream of api in pod {POD}: the kubelet does not answer \
                 (500); trying again in 1 s"
            ),
            "there all along".to_string(),
        ]
    );
}

#[tokio::test]
async fn a_container_that_restarted_gets_the_tail_of_its_previous_run_first() {
    let fake = FakeCluster::start().await;
    log_lines(&fake, POD, 0, &["old 1", "old 2", "old 3, the panic"]);
    fake.start_run(NS, POD, "api");
    log_lines(&fake, POD, 60_000, &["new 1", "new 2", "new 3"]);
    set_pod(&fake, POD, 1, running(), Some(terminated(101)));

    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        None,
        SessionOptions {
            tail: Some(2),
            ..options()
        },
    );
    assert_eq!(
        texts(&take(&mut session, 4).await),
        [
            "(previous) old 2",
            "(previous) old 3, the panic",
            "new 2",
            "new 3"
        ]
    );
    expect_quiet(&mut session).await;

    let requests = fake.requests();
    let previous = requests
        .iter()
        .find(|request| request.contains("previous=true"))
        .unwrap();
    assert!(previous.contains("tailLines=2"), "{previous}");
    assert!(!previous.contains("follow"), "{previous}");
}

#[tokio::test]
async fn a_crash_loop_shows_the_run_that_crashed_and_then_the_next_one() {
    let fake = FakeCluster::start().await;
    log_lines(&fake, POD, 0, &["starting", "thread 'main' panicked"]);
    fake.back_off(NS, POD, "api");
    set_pod(&fake, POD, 3, crash_loop(), Some(terminated(101)));

    let mut session = Session::start(&fake.cluster(), vec![target("flowcpt")], None, options());
    assert_eq!(
        texts(&take(&mut session, 3).await),
        [
            "(previous) starting".to_string(),
            "(previous) thread 'main' panicked".to_string(),
            format!(
                "-- api in pod {POD} is waiting: CrashLoopBackOff (back-off 40s restarting \
                 failed container)"
            ),
        ]
    );
    // A waiting container has no stream.
    expect_quiet(&mut session).await;

    // The back-off is over and the container starts again.
    fake.start_run(NS, POD, "api");
    log_lines(&fake, POD, 90_000, &["starting again"]);
    set_pod(&fake, POD, 4, running(), Some(terminated(101)));
    assert_eq!(texts(&take(&mut session, 1).await), ["starting again"]);
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn a_container_that_waits_to_start_says_so_and_is_read_once_it_runs() {
    let fake = FakeCluster::start().await;
    let pulling = json!({ "waiting": { "reason": "ImagePullBackOff" } });
    set_pod(&fake, POD, 0, pulling, None);
    let mut session = Session::start(&fake.cluster(), vec![target("flowcpt")], None, options());
    assert_eq!(
        texts(&take(&mut session, 1).await),
        [format!("-- api in pod {POD} is waiting: ImagePullBackOff")]
    );

    let creating = json!({ "waiting": { "reason": "ContainerCreating" } });
    set_pod(&fake, POD, 0, creating, None);
    assert_eq!(
        texts(&take(&mut session, 1).await),
        [format!("-- api in pod {POD} is waiting: ContainerCreating")]
    );

    // It starts. It was not running when the session began, so its log is
    // read from the first line, not from the tail.
    let lines: Vec<String> = (1..=25).map(|number| format!("line {number}")).collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    log_lines(&fake, POD, 0, &lines);
    set_pod(&fake, POD, 0, running(), None);
    let records = take(&mut session, 25).await;
    assert_eq!(records[0].message, "line 1");
    assert_eq!(records[24].message, "line 25");
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn a_pod_that_comes_up_later_is_read_from_its_beginning() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    log_lines(&fake, POD, 0, &["old pod, 1", "old pod, 2", "old pod, 3"]);
    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        None,
        SessionOptions {
            tail: Some(1),
            ..options()
        },
    );
    assert_eq!(texts(&take(&mut session, 1).await), ["old pod, 3"]);

    let new_pod = "flowcpt-api-5c8b-abcde";
    log_lines(
        &fake,
        new_pod,
        20_000,
        &["new pod, 1", "new pod, 2", "new pod, 3"],
    );
    set_pod(&fake, new_pod, 0, running(), None);
    // The notice carries the time it was made, the lines the time they were
    // written, so where the notice lands among them is not fixed.
    let mut seen = texts(&take(&mut session, 4).await);
    seen.sort();
    assert_eq!(
        seen,
        [
            format!("-- pod {new_pod} appeared; following api"),
            "new pod, 1".to_string(),
            "new pod, 2".to_string(),
            "new pod, 3".to_string(),
        ]
    );
}

#[tokio::test]
async fn a_run_that_crashed_unseen_is_still_read() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    log_lines(&fake, POD, 0, &["first run"]);
    let mut session = Session::start(&fake.cluster(), vec![target("flowcpt")], None, options());
    assert_eq!(texts(&take(&mut session, 1).await), ["first run"]);
    fake.end_run(NS, POD, "api");
    set_pod(&fake, POD, 0, terminated(1), None);
    expect_quiet(&mut session).await;

    // The next run starts and crashes before the pod reports either. All
    // there is to see afterwards is that the container waits again.
    fake.start_run(NS, POD, "api");
    log_lines(&fake, POD, 5_000, &["second run", "second run panicked"]);
    fake.back_off(NS, POD, "api");
    set_pod(&fake, POD, 1, crash_loop(), Some(terminated(101)));
    let mut seen = texts(&take(&mut session, 3).await);
    seen.sort();
    assert_eq!(
        seen,
        [
            format!(
                "-- api in pod {POD} is waiting: CrashLoopBackOff (back-off 40s restarting \
                 failed container)"
            ),
            "second run".to_string(),
            "second run panicked".to_string(),
        ]
    );
    // It is read once, not again while the container keeps waiting.
    expect_quiet(&mut session).await;

    fake.start_run(NS, POD, "api");
    log_lines(&fake, POD, 10_000, &["third run"]);
    set_pod(&fake, POD, 2, running(), Some(terminated(101)));
    assert_eq!(texts(&take(&mut session, 1).await), ["third run"]);
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn a_quiet_container_that_crashes_during_an_outage_loses_nothing() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    // It runs and has logged nothing yet, so the follower has no last line
    // to go by.
    fake.start_run(NS, POD, "api");
    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        None,
        SessionOptions {
            since: Some(Duration::from_secs(60)),
            tail: None,
            ..options()
        },
    );
    // Long enough for the empty history to count as delivered.
    tokio::time::sleep(Duration::from_millis(2_300)).await;

    // The connection drops. While it is down the container prints its
    // panic and is started again.
    fake.break_log_streams();
    fake.log(NS, POD, "api", Utc::now(), "panicked while nobody listened");
    fake.end_run(NS, POD, "api");
    fake.start_run(NS, POD, "api");
    fake.log(NS, POD, "api", Utc::now(), "second run");
    set_pod(&fake, POD, 1, running(), Some(terminated(101)));

    let records = take(&mut session, 3).await;
    assert!(
        records[0].message.starts_with("lost the log stream of api"),
        "{}",
        records[0].message
    );
    assert_eq!(
        texts(&records[1..]),
        ["panicked while nobody listened", "second run"]
    );
    expect_quiet(&mut session).await;

    // Both were asked for from where the history began, not as a new
    // history counted from the moment of the reconnect.
    let requests = fake.requests();
    let reopened: Vec<&String> = requests
        .iter()
        .filter(|request| request.contains("/log?"))
        .skip(1)
        .collect();
    assert_eq!(reopened.len(), 2, "{requests:#?}");
    for request in reopened {
        assert!(request.contains("sinceTime="), "{request}");
        assert!(!request.contains("sinceSeconds"), "{request}");
    }
}

#[tokio::test]
async fn a_silent_stream_is_opened_again_without_a_word_or_a_repeated_line() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    log_lines(&fake, POD, 0, &["one", "two"]);
    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        None,
        SessionOptions {
            idle: Duration::from_millis(300),
            ..options()
        },
    );
    assert_eq!(texts(&take(&mut session, 2).await), ["one", "two"]);

    // Several times the idle time goes by with nothing logged.
    tokio::time::sleep(Duration::from_secs(1)).await;
    expect_quiet(&mut session).await;
    let opened = fake
        .requests()
        .iter()
        .filter(|request| request.contains("/log?"))
        .count();
    assert!(opened >= 3, "the stream was opened {opened} times");

    // The stream that was opened last still follows.
    fake.log(NS, POD, "api", Utc::now(), "three");
    assert_eq!(texts(&take(&mut session, 1).await), ["three"]);
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn a_stream_that_died_without_a_word_is_opened_again_and_says_so() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    log_lines(&fake, POD, 0, &["before"]);
    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        None,
        SessionOptions {
            idle: Duration::from_millis(300),
            ..options()
        },
    );
    assert_eq!(texts(&take(&mut session, 1).await), ["before"]);

    // The connection stays open and carries nothing any more.
    fake.stall_log_streams();
    log_lines(
        &fake,
        POD,
        1_000,
        &["during the silence, 1", "during the silence, 2"],
    );
    let mut seen = texts(&take(&mut session, 3).await);
    seen.sort();
    assert_eq!(
        seen,
        [
            format!(
                "-- the log stream of api in pod {POD} had gone dead without a word; it was \
                 opened again and the lines since then follow"
            ),
            "during the silence, 1".to_string(),
            "during the silence, 2".to_string(),
        ]
    );
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn a_pod_that_goes_away_is_let_go_with_a_notice() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    log_lines(&fake, POD, 0, &["still here"]);
    let mut session = Session::start(&fake.cluster(), vec![target("flowcpt")], None, options());
    assert_eq!(texts(&take(&mut session, 1).await), ["still here"]);

    fake.end_run(NS, POD, "api");
    fake.delete(&pods(NS), POD);
    assert_eq!(
        texts(&take(&mut session, 1).await),
        [format!("-- pod {POD} went away")]
    );
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn the_records_of_several_pods_come_out_in_the_order_of_their_time() {
    let fake = FakeCluster::start().await;
    for (pod, offset) in [("pod-a", 0), ("pod-b", 50), ("pod-c", 25)] {
        set_pod(&fake, pod, 0, running(), None);
        for step in 0..5 {
            fake.log(
                NS,
                pod,
                "api",
                at(offset + step * 100),
                &format!("{pod} {step}"),
            );
        }
    }
    // The default hold, which is what orders the three streams.
    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        None,
        SessionOptions::default(),
    );
    let records = take(&mut session, 15).await;
    let times: Vec<DateTime<Utc>> = records.iter().map(|record| record.time).collect();
    let mut sorted = times.clone();
    sorted.sort();
    assert_eq!(times, sorted, "{:?}", texts(&records));
    assert_eq!(
        texts(&records[..4]),
        ["pod-a 0", "pod-c 0", "pod-b 0", "pod-a 1"]
    );
}

#[tokio::test]
async fn the_filter_leaves_lines_out_but_never_notices() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    log_lines(
        &fake,
        POD,
        0,
        &[
            r#"{"level":"INFO","message":"request done","tenant_id":"t_123"}"#,
            r#"{"level":"WARN","message":"payment intent expired","tenant_id":"t_123"}"#,
            r#"{"level":"ERROR","message":"other tenant","tenant_id":"t_999"}"#,
            "a plain line",
        ],
    );
    let filter = Filter::All(vec![
        Filter::level_at_least(Level::Warn),
        Filter::field_equals("tenant_id", "t_123").unwrap(),
    ]);
    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt"), target("flowcpt/-/no-such-pod")],
        Some(filter),
        options(),
    );
    let records = take(&mut session, 2).await;
    let line = records
        .iter()
        .find(|record| record.kind == Kind::Line)
        .unwrap();
    assert_eq!(line.message, "payment intent expired");
    assert_eq!(line.level, Some(Level::Warn));
    assert_eq!(line.format, Format::Json);
    assert_eq!(
        line.fields,
        BTreeMap::from([("tenant_id".to_string(), json!("t_123"))])
    );
    let notice = records
        .iter()
        .find(|record| record.kind == Kind::Notice)
        .unwrap();
    assert_eq!(
        notice.message,
        "target flowcpt/-/no-such-pod matches nothing yet; still watching"
    );
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn continuation_lines_arrive_as_one_record() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    // The lines of a backtrace are written at once.
    for line in [
        "ERROR handler failed",
        "   0: rust_begin_unwind",
        "   1: core::panicking",
    ] {
        fake.log(NS, POD, "api", at(0), line);
    }
    fake.log(NS, POD, "api", at(10), "INFO next request");
    let mut session = Session::start(&fake.cluster(), vec![target("flowcpt")], None, options());
    let records = take(&mut session, 2).await;
    assert_eq!(
        texts(&records),
        [
            "ERROR handler failed\n   0: rust_begin_unwind\n   1: core::panicking",
            "INFO next request"
        ]
    );
    assert_eq!(records[0].level, Some(Level::Error));
    assert_eq!(records[0].time, at(0));
}

#[tokio::test]
async fn the_format_option_is_applied_per_container() {
    let fake = FakeCluster::start().await;
    let statuses = vec![
        container_status("api", 0, running(), None),
        container_status("envoy", 0, running(), None),
    ];
    fake.apply(&pods(NS), pod_with_statuses(NS, POD, statuses));
    let line = r#"{"level":"info","message":"hello"}"#;
    fake.log(NS, POD, "api", at(0), line);
    fake.log(NS, POD, "envoy", at(100), line);

    let format =
        FormatSetting::PerContainer(BTreeMap::from([("envoy".to_string(), FormatChoice::Text)]));
    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        None,
        SessionOptions {
            format,
            ..options()
        },
    );
    let records = take(&mut session, 2).await;
    assert_eq!(records[0].container.as_deref(), Some("api"));
    assert_eq!(records[0].format, Format::Json);
    assert_eq!(records[0].message, "hello");
    assert_eq!(records[0].level, Some(Level::Info));
    // The lines of envoy are shown untouched.
    assert_eq!(records[1].container.as_deref(), Some("envoy"));
    assert_eq!(records[1].format, Format::Text);
    assert_eq!(records[1].message, line);
    assert!(records[1].fields.is_empty());
}

#[tokio::test]
async fn the_container_option_keeps_sidecars_out() {
    let fake = FakeCluster::start().await;
    let statuses = vec![
        container_status("api", 0, running(), None),
        container_status("envoy", 0, running(), None),
    ];
    fake.apply(&pods(NS), pod_with_statuses(NS, POD, statuses));
    fake.log(NS, POD, "api", at(0), "from the api");
    fake.log(NS, POD, "envoy", at(0), "from envoy");
    let mut session = Session::start(
        &fake.cluster(),
        vec![target("flowcpt")],
        None,
        SessionOptions {
            containers: Some(Pattern::new("api").unwrap()),
            ..options()
        },
    );
    assert_eq!(texts(&take(&mut session, 1).await), ["from the api"]);
    expect_quiet(&mut session).await;
}

#[tokio::test]
async fn dropping_the_session_closes_its_streams() {
    let fake = FakeCluster::start().await;
    set_pod(&fake, POD, 0, running(), None);
    log_lines(&fake, POD, 0, &["one"]);
    let mut session = Session::start(&fake.cluster(), vec![target("flowcpt")], None, options());
    assert_eq!(texts(&take(&mut session, 1).await), ["one"]);
    drop(session);

    // Nothing asks the API server for anything any more, also not when the
    // stream would have to be opened again.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let before = fake.requests().len();
    fake.break_log_streams();
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(fake.requests().len(), before);
}
