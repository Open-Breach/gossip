//! `gossip follow`, run as a program against the stand-in API server of the
//! core. These are the acceptance points that need no cluster, as far as a
//! stand-in can show them; the real thing is checked by hand.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use gossip_core::testing::{
    FakeCluster, deployments, event_object, events, owner, pod_object, pods, replica_sets,
    workload_object,
};
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};
use tokio::time::timeout;

const NS: &str = "flowcpt";
const POD_A: &str = "flowcpt-api-7d9f-xk2p1";
const POD_B: &str = "flowcpt-api-7d9f-m4q8z";

/// A time `millis` after a fixed moment in the past.
fn at(millis: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 18, 14, 0, 0).unwrap() + TimeDelta::milliseconds(millis)
}

/// The deployment `flowcpt-api` with two pods, each with the containers
/// `api` and `envoy`.
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
    for pod in [POD_A, POD_B] {
        let set = owner("ReplicaSet", "flowcpt-api-7d9f");
        fake.apply(
            &pods(NS),
            pod_object(NS, pod, &api, Some(set), &["api", "envoy"]),
        );
    }
}

/// A running `gossip follow`.
struct Follow {
    child: Child,
    stdout: Lines<BufReader<ChildStdout>>,
    kubeconfig: PathBuf,
    config: PathBuf,
}

impl Follow {
    /// Starts `gossip follow` with these arguments against the stand-in,
    /// with an empty config file.
    fn start(fake: &FakeCluster, test: &str, args: &[&str]) -> Follow {
        Follow::start_with_config(fake, test, "", args)
    }

    /// The same with a config file of this content.
    fn start_with_config(
        fake: &FakeCluster,
        test: &str,
        config_text: &str,
        args: &[&str],
    ) -> Follow {
        let kubeconfig =
            std::env::temp_dir().join(format!("gossip-{}-{test}.kubeconfig", std::process::id()));
        std::fs::write(&kubeconfig, fake.kubeconfig()).unwrap();
        // A config file of the test's own, so that the one of whoever runs
        // the tests stays out of them.
        let config =
            std::env::temp_dir().join(format!("gossip-{}-{test}.toml", std::process::id()));
        std::fs::write(&config, config_text).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_gossip"));
        command
            .arg("follow")
            .args(args)
            .env("KUBECONFIG", &kubeconfig)
            // Times are printed in the local zone; pin it down.
            .env("TZ", "UTC")
            .env("GOSSIP_CONFIG", &config)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for variable in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            command.env_remove(variable);
        }
        let mut child = command.spawn().expect("gossip should start");
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        Follow {
            child,
            stdout,
            kubeconfig,
            config,
        }
    }

    async fn line(&mut self) -> String {
        match timeout(Duration::from_secs(15), self.stdout.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => panic!("gossip ended"),
            Ok(Err(error)) => panic!("cannot read from gossip: {error}"),
            Err(_) => panic!("gossip printed nothing in time"),
        }
    }

    async fn lines(&mut self, count: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for _ in 0..count {
            lines.push(self.line().await);
        }
        lines
    }

    async fn expect_quiet(&mut self) {
        if let Ok(line) = timeout(Duration::from_millis(600), self.stdout.next_line()).await {
            panic!("expected nothing more, got {line:?}");
        }
    }
}

impl Drop for Follow {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.kubeconfig);
        let _ = std::fs::remove_file(&self.config);
    }
}

#[tokio::test]
async fn follow_prints_the_last_20_lines_of_every_container_and_then_new_lines() {
    let fake = FakeCluster::start().await;
    seed(&fake);
    // 25 lines in every container, of which the last 20 are history.
    let mut offset = 0;
    for pod in [POD_A, POD_B] {
        for container in ["api", "envoy"] {
            for number in 1..=25 {
                let time = at(number * 1_000 + offset);
                fake.log(
                    NS,
                    pod,
                    container,
                    time,
                    &format!("{container} line {number}"),
                );
            }
            offset += 10;
        }
    }

    let mut follow = Follow::start(&fake, "tail", &["flowcpt/flowcpt-api", "--no-color"]);
    let history = follow.lines(80).await;
    // Twenty lines for each of the four containers, none older than line 6.
    for pod in ["7d9f-xk2p1", "7d9f-m4q8z"] {
        for container in ["api", "envoy"] {
            let of_container: Vec<&String> = history
                .iter()
                .filter(|line| line.contains(&format!(" {pod} {container}")))
                .collect();
            assert_eq!(of_container.len(), 20, "{pod} {container}");
            assert!(
                of_container[0].ends_with(&format!("{container} line 6")),
                "{}",
                of_container[0]
            );
            assert!(
                of_container[19].ends_with(&format!("{container} line 25")),
                "{}",
                of_container[19]
            );
        }
    }
    // Ordered by time across the four streams, with a prefix per source.
    // The pod names are shortened: every pod belongs to a deployment.
    assert_eq!(
        history[0],
        // The container column is as wide as the widest name seen so far.
        "14:00:06.000 7d9f-xk2p1 api        api line 6"
    );
    assert_eq!(
        history[1],
        "14:00:06.010 7d9f-xk2p1 envoy        envoy line 6"
    );
    assert_eq!(
        history[2],
        "14:00:06.020 7d9f-m4q8z api          api line 6"
    );
    let mut sorted = history.clone();
    sorted.sort();
    assert_eq!(history, sorted);
    follow.expect_quiet().await;

    // New lines arrive without any action.
    fake.log(NS, POD_B, "api", at(60_000), "WARN something new");
    assert_eq!(
        follow.line().await,
        "14:01:00.000 7d9f-m4q8z api    WARN  WARN something new"
    );
}

const TENANT_LINES: [&str; 4] = [
    r#"{"timestamp":"2026-11-18T14:00:01Z","level":"INFO","message":"request done","tenant_id":"t_123","request_id":"8b1c"}"#,
    r#"{"timestamp":"2026-11-18T14:00:02Z","level":"WARN","message":"payment intent expired","tenant_id":"t_123","request_id":"8b1d"}"#,
    r#"{"timestamp":"2026-11-18T14:00:03Z","level":"ERROR","message":"card declined","tenant_id":"t_999","request_id":"8b1e"}"#,
    "a plain line with no level",
];

/// One pod with one container that wrote the four lines above.
fn seed_tenant_lines(fake: &FakeCluster) {
    fake.apply(&pods(NS), pod_object(NS, "redis-0", &[], None, &["api"]));
    for (index, line) in TENANT_LINES.iter().enumerate() {
        fake.log(NS, "redis-0", "api", at(1_000 + index as i64 * 1_000), line);
    }
}

#[tokio::test]
async fn a_json_line_is_rendered_as_level_fields_and_message() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);
    let mut follow = Follow::start(
        &fake,
        "json-line",
        &["flowcpt", "--no-color", "--fields", "tenant_id,request_id"],
    );
    assert_eq!(
        follow.lines(4).await,
        [
            "14:00:01.000 redis-0 api  INFO  tenant_id=t_123 request_id=8b1c  request done",
            "14:00:02.000 redis-0 api  WARN  tenant_id=t_123 request_id=8b1d  payment intent expired",
            "14:00:03.000 redis-0 api  ERROR tenant_id=t_999 request_id=8b1e  card declined",
            "14:00:04.000 redis-0 api        a plain line with no level",
        ]
    );
}

#[tokio::test]
async fn the_field_flag_keeps_only_that_tenants_lines() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);
    let mut follow = Follow::start(
        &fake,
        "field",
        &[
            "flowcpt",
            "--no-color",
            "--fields",
            "request_id",
            "--field",
            "tenant_id=t_123",
        ],
    );
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:01.000 redis-0 api  INFO  request_id=8b1c  request done",
            "14:00:02.000 redis-0 api  WARN  request_id=8b1d  payment intent expired",
        ]
    );
    follow.expect_quiet().await;
}

/// Two lines as `tracing-subscriber`'s JSON formatter writes them when the
/// span of a request closes: the fields of the span under `span`, the chain
/// of spans around it under `spans`. The first is a line of the real api.
const SPAN_LINES: [&str; 2] = [
    r#"{"timestamp":"2026-11-18T20:39:01.187944Z","level":"INFO","message":"close","time.busy":"5.53ms","time.idle":"36.3ms","target":"flowcpt_telemetry","span":{"http.method":"GET","http.route":"/api/v1/sources","http.status_code":200,"request_id":"32e49c93-4c57-43a0-9439-c554e5fff1f6","tenant_id":"3K6ehNLhnRsPfWhzjugmea","user_id":"auth0|6a86204723ca712d0c11f85a","otel.name":"GET /api/v1/sources"},"spans":[{"http.method":"GET","http.route":"/api/v1/sources","request_id":"32e49c93-4c57-43a0-9439-c554e5fff1f6","tenant_id":"3K6ehNLhnRsPfWhzjugmea"}]}"#,
    r#"{"timestamp":"2026-11-18T20:39:01.412307Z","level":"WARN","message":"close","time.busy":"1.21ms","time.idle":"9.04ms","target":"flowcpt_telemetry","span":{"http.method":"POST","http.route":"/api/v1/flows","http.status_code":404,"request_id":"9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11","tenant_id":"8Hq2mVxWc4TzPb7LdYn3Ka","otel.name":"POST /api/v1/flows"},"spans":[{"http.method":"POST","http.route":"/api/v1/flows","request_id":"9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11","tenant_id":"8Hq2mVxWc4TzPb7LdYn3Ka"}]}"#,
];

/// One pod with one container that wrote the two lines above.
fn seed_span_lines(fake: &FakeCluster) {
    fake.apply(&pods(NS), pod_object(NS, "fc-api-0", &[], None, &["api"]));
    for (index, line) in SPAN_LINES.iter().enumerate() {
        fake.log(
            NS,
            "fc-api-0",
            "api",
            at(1_000 + index as i64 * 1_000),
            line,
        );
    }
}

/// The fields a request is read by, in the order they are read in.
const REQUEST_FIELDS: &str =
    "http.method,http.route,http.status_code,tenant_id,request_id,time.busy";

#[tokio::test]
async fn the_fields_of_a_tracing_span_are_shown_as_fields_of_the_line() {
    let fake = FakeCluster::start().await;
    seed_span_lines(&fake);

    // As it comes: every field of the line and of its span, longest last,
    // without the timestamp and the target, and no `span` or `spans`.
    let mut follow = Follow::start(&fake, "span", &["flowcpt", "--no-color"]);
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:01.000 fc-api-0 api  INFO  http.method=GET time.busy=5.53ms time.idle=36.3ms http.status_code=200 http.route=/api/v1/sources otel.name=\"GET /api/v1/sources\" tenant_id=3K6ehNLhnRsPfWhzjugmea user_id=auth0|6a86204723ca712d0c11f85a request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6  close",
            "14:00:02.000 fc-api-0 api  WARN  http.method=POST time.busy=1.21ms time.idle=9.04ms http.status_code=404 http.route=/api/v1/flows otel.name=\"POST /api/v1/flows\" tenant_id=8Hq2mVxWc4TzPb7LdYn3Ka request_id=9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11  close",
        ]
    );

    // With the fields named: method, route, status, tenant, request id,
    // busy time, then the message.
    let mut follow = Follow::start(
        &fake,
        "span-fields",
        &["flowcpt", "--no-color", "--fields", REQUEST_FIELDS],
    );
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:01.000 fc-api-0 api  INFO  http.method=GET http.route=/api/v1/sources http.status_code=200 tenant_id=3K6ehNLhnRsPfWhzjugmea request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6 time.busy=5.53ms  close",
            "14:00:02.000 fc-api-0 api  WARN  http.method=POST http.route=/api/v1/flows http.status_code=404 tenant_id=8Hq2mVxWc4TzPb7LdYn3Ka request_id=9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11 time.busy=1.21ms  close",
        ]
    );

    // The timestamp and the target are there for whoever names them.
    let mut follow = Follow::start(
        &fake,
        "span-quiet",
        &[
            "flowcpt",
            "--no-color",
            "--tail",
            "1",
            "--fields",
            "target,timestamp",
        ],
    );
    assert_eq!(
        follow.line().await,
        "14:00:02.000 fc-api-0 api  WARN  target=flowcpt_telemetry timestamp=2026-11-18T20:39:01.412307Z  close"
    );
}

#[tokio::test]
async fn the_field_flag_sees_the_fields_of_a_tracing_span() {
    let fake = FakeCluster::start().await;
    seed_span_lines(&fake);

    let mut follow = Follow::start(
        &fake,
        "span-tenant",
        &[
            "flowcpt",
            "--no-color",
            "--fields",
            "http.route",
            "--field",
            "tenant_id=3K6ehNLhnRsPfWhzjugmea",
        ],
    );
    assert_eq!(
        follow.line().await,
        "14:00:01.000 fc-api-0 api  INFO  http.route=/api/v1/sources  close"
    );
    follow.expect_quiet().await;

    // Also one with a dot in its name and a number as its value.
    let mut follow = Follow::start(
        &fake,
        "span-status",
        &[
            "flowcpt",
            "--no-color",
            "--fields",
            "http.route",
            "--field",
            "http.status_code=404",
        ],
    );
    assert_eq!(
        follow.line().await,
        "14:00:02.000 fc-api-0 api  WARN  http.route=/api/v1/flows  close"
    );
    follow.expect_quiet().await;
}

/// What `tracing-subscriber` writes for a line logged in a span inside the
/// span of a request: the fields of the request, its tenant among them, are
/// in the chain under `spans` only.
const NESTED_SPAN_LINE: &str = r#"{"timestamp":"2026-11-18T20:39:01.180112Z","level":"INFO","message":"sources loaded","rows":3,"target":"flowcpt_api::sources","span":{"table":"sources","name":"load_sources"},"spans":[{"http.method":"GET","http.route":"/api/v1/sources","request_id":"32e49c93-4c57-43a0-9439-c554e5fff1f6","tenant_id":"3K6ehNLhnRsPfWhzjugmea","name":"request"},{"table":"sources","name":"load_sources"}]}"#;

#[tokio::test]
async fn the_field_flag_finds_a_line_logged_in_a_span_inside_the_span_of_a_request() {
    let fake = FakeCluster::start().await;
    seed_span_lines(&fake);
    fake.log(NS, "fc-api-0", "api", at(3_000), NESTED_SPAN_LINE);

    let mut follow = Follow::start(
        &fake,
        "span-nested",
        &[
            "flowcpt",
            "--no-color",
            "--fields",
            "name,table,rows,request_id",
            "--field",
            "tenant_id=3K6ehNLhnRsPfWhzjugmea",
        ],
    );
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:01.000 fc-api-0 api  INFO  request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6  close",
            "14:00:03.000 fc-api-0 api  INFO  name=load_sources table=sources rows=3 request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6  sources loaded",
        ]
    );
    follow.expect_quiet().await;
}

#[tokio::test]
async fn a_tracing_span_as_json_is_flat_and_raw_is_the_line_as_it_came() {
    let fake = FakeCluster::start().await;
    seed_span_lines(&fake);

    let mut follow = Follow::start(
        &fake,
        "span-json",
        &["flowcpt", "--output", "json", "--tail", "1"],
    );
    let record: serde_json::Value = serde_json::from_str(&follow.line().await).unwrap();
    // Every field, the timestamp and the target too: hiding them is a matter
    // of the text output only.
    assert_eq!(
        record["fields"],
        serde_json::json!({
            "http.method": "POST",
            "http.route": "/api/v1/flows",
            "http.status_code": 404,
            "otel.name": "POST /api/v1/flows",
            "request_id": "9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11",
            "target": "flowcpt_telemetry",
            "tenant_id": "8Hq2mVxWc4TzPb7LdYn3Ka",
            "time.busy": "1.21ms",
            "time.idle": "9.04ms",
            "timestamp": "2026-11-18T20:39:01.412307Z"
        })
    );
    assert_eq!(record["raw"], SPAN_LINES[1]);

    let mut follow = Follow::start(&fake, "span-raw", &["flowcpt", "--raw"]);
    assert_eq!(follow.lines(2).await, SPAN_LINES);
}

#[tokio::test]
async fn the_level_flag_hides_info_lines_and_keeps_lines_with_no_level() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);
    let mut follow = Follow::start(
        &fake,
        "level",
        &[
            "flowcpt",
            "--no-color",
            "--fields",
            "request_id",
            "--level",
            "warn",
        ],
    );
    // The line with no level stays: unknown is not the same as unimportant.
    assert_eq!(
        follow.lines(3).await,
        [
            "14:00:02.000 redis-0 api  WARN  request_id=8b1d  payment intent expired",
            "14:00:03.000 redis-0 api  ERROR request_id=8b1e  card declined",
            "14:00:04.000 redis-0 api        a plain line with no level",
        ]
    );
    follow.expect_quiet().await;
}

/// A panic as Rust prints it, in the container `api` of the pod `redis-0`,
/// after a JSON line and before another.
fn seed_panic(fake: &FakeCluster) {
    fake.apply(&pods(NS), pod_object(NS, "redis-0", &[], None, &["api"]));
    let lines = [
        r#"{"level":"INFO","message":"charging","tenant_id":"t_123"}"#,
        "",
        "thread 'tokio-runtime-worker' (241) panicked at src/payments.rs:42:19:",
        "tenant t_123 not found",
        "stack backtrace:",
        "   0: __rustc::rust_begin_unwind",
        "             at /rustc/0ef1bc386/library/std/src/panicking.rs:676:5",
        "   1: flowcpt_api::payments::charge",
        "note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.",
        r#"{"level":"INFO","message":"worker restarted"}"#,
    ];
    for (index, line) in lines.iter().enumerate() {
        fake.log(NS, "redis-0", "api", at(1_000 + index as i64), line);
    }
}

#[tokio::test]
async fn a_rust_panic_with_its_backtrace_arrives_as_one_record() {
    let fake = FakeCluster::start().await;
    seed_panic(&fake);
    let mut follow = Follow::start(&fake, "panic", &["flowcpt", "--output", "json"]);
    let records: Vec<serde_json::Value> = follow
        .lines(4)
        .await
        .iter()
        .map(|line| serde_json::from_str(line).expect("every line is JSON"))
        .collect();
    assert_eq!(records[0]["message"], "charging");
    // The blank line Rust prints before a panic.
    assert_eq!(records[1]["message"], "");
    // One record whose message holds every line of the panic.
    assert_eq!(
        records[2]["message"],
        "thread 'tokio-runtime-worker' (241) panicked at src/payments.rs:42:19:\n\
         tenant t_123 not found\n\
         stack backtrace:\n\
         \x20  0: __rustc::rust_begin_unwind\n\
         \x20            at /rustc/0ef1bc386/library/std/src/panicking.rs:676:5\n\
         \x20  1: flowcpt_api::payments::charge\n\
         note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace."
    );
    assert_eq!(records[2]["format"], "text");
    assert_eq!(records[2]["level"], serde_json::Value::Null);
    // The JSON line after it ended the block and is a record of its own.
    assert_eq!(records[3]["message"], "worker restarted");
    follow.expect_quiet().await;
}

#[tokio::test]
async fn a_panic_does_not_vanish_under_a_level_filter() {
    let fake = FakeCluster::start().await;
    seed_panic(&fake);
    let mut follow = Follow::start(
        &fake,
        "panic-level",
        &["flowcpt", "--no-color", "--level", "error"],
    );
    // The two info lines are hidden. The panic has no level and stays, and
    // so does the blank line before it.
    let lines = follow.lines(8).await;
    assert_eq!(lines[0], "14:00:01.001 redis-0 api");
    assert_eq!(
        lines[1],
        "14:00:01.002 redis-0 api        thread 'tokio-runtime-worker' (241) panicked at src/payments.rs:42:19:"
    );
    assert_eq!(lines[2], "tenant t_123 not found");
    assert_eq!(
        lines[7],
        "note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace."
    );
    follow.expect_quiet().await;
}

#[tokio::test]
async fn grep_and_match_filter_on_the_message() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);
    let mut follow = Follow::start(
        &fake,
        "grep",
        &[
            "flowcpt",
            "--no-color",
            "--fields",
            "request_id",
            "--grep",
            "e",
            "--match",
            "^(card|a plain)",
        ],
    );
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:03.000 redis-0 api  ERROR request_id=8b1e  card declined",
            "14:00:04.000 redis-0 api        a plain line with no level",
        ]
    );
    follow.expect_quiet().await;
}

#[tokio::test]
async fn raw_prints_lines_byte_for_byte() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);
    fake.log(
        NS,
        "redis-0",
        "api",
        at(9_000),
        "  indented, with a tab\tand trailing spaces  ",
    );
    let mut follow = Follow::start(&fake, "raw", &["flowcpt", "--raw"]);
    let mut expected: Vec<String> = TENANT_LINES.iter().map(|line| line.to_string()).collect();
    expected.push("  indented, with a tab\tand trailing spaces  ".to_string());
    assert_eq!(follow.lines(5).await, expected);
}

#[tokio::test]
async fn output_json_prints_one_record_per_line_with_every_field() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);
    let mut follow = Follow::start(&fake, "json", &["flowcpt", "--output", "json"]);
    let lines = follow.lines(4).await;
    let records: Vec<serde_json::Value> = lines
        .iter()
        .map(|line| serde_json::from_str(line).expect("every line is JSON"))
        .collect();
    assert_eq!(
        records[1],
        serde_json::json!({
            "cluster": "fake",
            "namespace": "flowcpt",
            "pod": "redis-0",
            "container": "api",
            "time": "2026-11-18T14:00:02Z",
            "kind": "line",
            "format": "json",
            "level": "warn",
            "message": "payment intent expired",
            "fields": {
                "tenant_id": "t_123",
                "request_id": "8b1d",
                "timestamp": "2026-11-18T14:00:02Z"
            },
            "raw": TENANT_LINES[1],
            "previous": false,
            "truncated": false
        })
    );
    assert_eq!(records[3]["format"], "text");
    assert_eq!(records[3]["level"], serde_json::Value::Null);
    assert_eq!(records[3]["message"], "a plain line with no level");
}

#[tokio::test]
async fn a_notice_is_printed_with_two_dashes() {
    let fake = FakeCluster::start().await;
    let mut follow = Follow::start(&fake, "notice", &["flowcpt/flowcpt-api", "--no-color"]);
    let line = follow.line().await;
    assert!(
        line.ends_with(" -- target flowcpt/flowcpt-api matches nothing yet; still watching"),
        "{line}"
    );
}

#[tokio::test]
async fn ctrl_c_stops_the_session_and_exits_0() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);
    let mut follow = Follow::start(&fake, "ctrl-c", &["flowcpt", "--no-color"]);
    follow.lines(4).await;

    let pid = follow.child.id().expect("gossip is running").to_string();
    let killed = std::process::Command::new("kill")
        .args(["-INT", &pid])
        .status()
        .expect("kill should run");
    assert!(killed.success());
    let status = timeout(Duration::from_secs(10), follow.child.wait())
        .await
        .expect("gossip should stop in time")
        .unwrap();
    assert_eq!(status.code(), Some(0));
}

#[tokio::test]
async fn a_cluster_that_cannot_be_reached_at_start_exits_1_with_the_error() {
    let kubeconfig = std::env::temp_dir().join(format!(
        "gossip-{}-follow-nowhere.kubeconfig",
        std::process::id()
    ));
    let text = "apiVersion: v1\nkind: Config\ncurrent-context: nowhere\nclusters:\n- name: nowhere\n  cluster:\n    server: https://127.0.0.1:1\ncontexts:\n- name: nowhere\n  context:\n    cluster: nowhere\n    user: nobody\nusers:\n- name: nobody\n  user:\n    token: x\n";
    std::fs::write(&kubeconfig, text).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_gossip"));
    command
        .args(["follow", "flowcpt"])
        .env("KUBECONFIG", &kubeconfig);
    for variable in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env_remove(variable);
    }
    let output = timeout(Duration::from_secs(30), command.output())
        .await
        .expect("gossip should give up in time")
        .unwrap();
    std::fs::remove_file(&kubeconfig).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("gossip: cannot reach the cluster `nowhere`: "),
        "{stderr}"
    );
}

/// Puts an event about the container `api` of the pod `redis-0` into the
/// namespace, happening now.
fn add_event(fake: &FakeCluster, name: &str, reason: &str, note: &str, kind: &str) {
    let event = event_object(
        NS,
        name,
        ("Pod", "redis-0"),
        Some("api"),
        reason,
        note,
        kind,
        1,
        Utc::now(),
    );
    fake.apply(&events(NS), event);
}

#[tokio::test]
async fn events_are_merged_into_the_stream() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);
    let mut follow = Follow::start(
        &fake,
        "events",
        &["flowcpt", "--no-color", "--fields", "request_id"],
    );
    follow.lines(4).await;

    add_event(
        &fake,
        "killing",
        "Killing",
        "Container api failed liveness probe, will be restarted",
        "Normal",
    );
    let line = follow.line().await;
    // Stdout is not a terminal here, so the event is marked `EVNT`, in the
    // column of the container.
    assert!(
        line.ends_with(
            " redis-0 EVNT        Killing: Container api failed liveness probe, will be restarted"
        ),
        "{line}"
    );

    // A line after it is aligned to the wider column.
    fake.log(NS, "redis-0", "api", Utc::now(), "INFO back again");
    let line = follow.line().await;
    assert!(
        line.ends_with(" redis-0 api   INFO  INFO back again"),
        "{line}"
    );
}

#[tokio::test]
async fn the_filter_flags_hide_lines_and_events_still_show() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);
    let mut follow = Follow::start(
        &fake,
        "events-filter",
        &[
            "flowcpt",
            "--no-color",
            "--fields",
            "request_id",
            "--level",
            "warn",
            "--field",
            "tenant_id=t_123",
        ],
    );
    // Of the four lines only the warning of that tenant is left.
    assert_eq!(
        follow.line().await,
        "14:00:02.000 redis-0 api  WARN  request_id=8b1d  payment intent expired"
    );

    add_event(
        &fake,
        "started",
        "Started",
        "Started container api",
        "Normal",
    );
    let line = follow.line().await;
    assert!(line.ends_with("Started: Started container api"), "{line}");
    follow.expect_quiet().await;
}

#[tokio::test]
async fn no_events_shows_no_event_records() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);
    let mut follow = Follow::start(
        &fake,
        "no-events",
        &["flowcpt", "--no-color", "--no-events"],
    );
    follow.lines(4).await;

    add_event(
        &fake,
        "started",
        "Started",
        "Started container api",
        "Normal",
    );
    follow.expect_quiet().await;
    // A line still comes through.
    fake.log(NS, "redis-0", "api", Utc::now(), "still following");
    assert!(follow.line().await.ends_with("still following"));
}

#[tokio::test]
async fn an_event_as_json_carries_its_reason_type_and_count() {
    let fake = FakeCluster::start().await;
    fake.apply(&pods(NS), pod_object(NS, "redis-0", &[], None, &["api"]));
    let mut follow = Follow::start(&fake, "events-json", &["flowcpt", "--output", "json"]);
    // Give the session a moment to find the pod, then let the event happen.
    tokio::time::sleep(Duration::from_millis(500)).await;
    add_event(
        &fake,
        "backoff",
        "BackOff",
        "Back-off restarting failed container",
        "Warning",
    );
    let record: serde_json::Value = serde_json::from_str(&follow.line().await).unwrap();
    assert_eq!(record["kind"], "event");
    assert_eq!(record["format"], "text");
    assert_eq!(record["level"], "warn");
    assert_eq!(record["pod"], "redis-0");
    assert_eq!(record["container"], "api");
    assert_eq!(record["message"], "Back-off restarting failed container");
    assert_eq!(
        record["fields"],
        serde_json::json!({
            "reason": "BackOff", "type": "Warning", "count": 1, "regarding": "Pod/redis-0"
        })
    );
}

const TEMPLATES: &str = r#"
[defaults]
tail = 2
events = false

[templates.tenant]
targets = ["flowcpt"]
container = "api"
tail = 10
fields = ["request_id"]
filter = { all = [
  { field = "level", op = ">=", value = "warn" },
  { field = "f.tenant_id", op = "==", value = "t_123" },
] }

[templates.plain]
targets = ["flowcpt"]
fields = []

[templates.requests]
targets = ["flowcpt"]
fields = ["http.method", "http.route", "http.status_code", "tenant_id", "request_id", "time.busy"]
"#;

#[tokio::test]
async fn a_template_behaves_as_the_flags_spelt_out_in_it() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);

    let mut by_template = Follow::start_with_config(
        &fake,
        "template",
        TEMPLATES,
        &["--template", "tenant", "--no-color"],
    );
    let from_template = by_template.line().await;
    by_template.expect_quiet().await;

    let mut by_flags = Follow::start(
        &fake,
        "template-flags",
        &[
            "flowcpt",
            "--no-color",
            "--no-events",
            "--container",
            "api",
            "--tail",
            "10",
            "--fields",
            "request_id",
            "--level",
            "warn",
            "--field",
            "tenant_id=t_123",
        ],
    );
    let from_flags = by_flags.line().await;
    by_flags.expect_quiet().await;

    assert_eq!(
        from_template,
        "14:00:02.000 redis-0 api  WARN  request_id=8b1d  payment intent expired"
    );
    assert_eq!(from_template, from_flags);
}

#[tokio::test]
async fn the_fields_of_a_template_are_the_fields_shown_in_that_order() {
    let fake = FakeCluster::start().await;
    seed_span_lines(&fake);

    let mut follow = Follow::start_with_config(
        &fake,
        "template-fields",
        TEMPLATES,
        &["--template", "requests", "--no-color"],
    );
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:01.000 fc-api-0 api  INFO  http.method=GET http.route=/api/v1/sources http.status_code=200 tenant_id=3K6ehNLhnRsPfWhzjugmea request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6 time.busy=5.53ms  close",
            "14:00:02.000 fc-api-0 api  WARN  http.method=POST http.route=/api/v1/flows http.status_code=404 tenant_id=8Hq2mVxWc4TzPb7LdYn3Ka request_id=9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11 time.busy=1.21ms  close",
        ]
    );

    // An empty list is the message only.
    let mut follow = Follow::start_with_config(
        &fake,
        "template-no-fields",
        TEMPLATES,
        &["--template", "plain", "--no-color"],
    );
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:01.000 fc-api-0 api  INFO  close",
            "14:00:02.000 fc-api-0 api  WARN  close",
        ]
    );

    // And --fields beside a template takes the place of its list.
    let mut follow = Follow::start_with_config(
        &fake,
        "template-fields-flag",
        TEMPLATES,
        &[
            "--template",
            "requests",
            "--no-color",
            "--fields",
            "time.idle",
        ],
    );
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:01.000 fc-api-0 api  INFO  time.idle=36.3ms  close",
            "14:00:02.000 fc-api-0 api  WARN  time.idle=9.04ms  close",
        ]
    );
}

#[tokio::test]
async fn the_defaults_fill_in_what_a_template_leaves_out_and_a_flag_overrides() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);

    // The template says nothing about the history: the defaults give 2 lines.
    let mut follow = Follow::start_with_config(
        &fake,
        "template-defaults",
        TEMPLATES,
        &["--template", "plain", "--no-color"],
    );
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:03.000 redis-0 api  ERROR card declined",
            "14:00:04.000 redis-0 api        a plain line with no level",
        ]
    );
    follow.expect_quiet().await;

    // A flag beside the template overrides: three lines of history.
    let mut follow = Follow::start_with_config(
        &fake,
        "template-override",
        TEMPLATES,
        &["--template", "plain", "--no-color", "--tail", "3"],
    );
    let lines = follow.lines(3).await;
    assert!(
        lines[0].ends_with("WARN  payment intent expired"),
        "{}",
        lines[0]
    );
    follow.expect_quiet().await;

    // A filter flag beside a template narrows its filter: a line must pass
    // both. No line of that tenant is an error, so nothing is left.
    let mut follow = Follow::start_with_config(
        &fake,
        "template-narrow",
        TEMPLATES,
        &["--template", "tenant", "--no-color", "--level", "error"],
    );
    follow.expect_quiet().await;

    // With --no-filter the filter of the template is dropped first, and the
    // flag is the whole filter. The line with no level passes it.
    let mut follow = Follow::start_with_config(
        &fake,
        "template-no-filter",
        TEMPLATES,
        &[
            "--template",
            "tenant",
            "--no-color",
            "--no-filter",
            "--level",
            "error",
        ],
    );
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:03.000 redis-0 api  ERROR request_id=8b1e  card declined",
            "14:00:04.000 redis-0 api        a plain line with no level",
        ]
    );
    follow.expect_quiet().await;
}
