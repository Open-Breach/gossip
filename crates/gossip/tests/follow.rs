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

/// The command `gossip follow` with these arguments against the stand-in,
/// with a config file of this content, and the two files it reads. Stdout
/// and stderr are pipes, and stdin is closed: no terminal on any side.
fn follow_command(
    fake: &FakeCluster,
    test: &str,
    config_text: &str,
    args: &[&str],
) -> (Command, PathBuf, PathBuf) {
    let kubeconfig =
        std::env::temp_dir().join(format!("gossip-{}-{test}.kubeconfig", std::process::id()));
    std::fs::write(&kubeconfig, fake.kubeconfig()).unwrap();
    // A config file of the test's own, so that the one of whoever runs
    // the tests stays out of them.
    let config = std::env::temp_dir().join(format!("gossip-{}-{test}.toml", std::process::id()));
    std::fs::write(&config, config_text).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_gossip"));
    command
        .arg("follow")
        .args(args)
        .env("KUBECONFIG", &kubeconfig)
        // Times are printed in the local zone; pin it down.
        .env("TZ", "UTC")
        .env("GOSSIP_CONFIG", &config)
        .stdin(Stdio::null())
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
    (command, kubeconfig, config)
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
        let (mut command, kubeconfig, config) = follow_command(fake, test, config_text, args);
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
    // The pod column holds the full name, cut in the middle: the workload
    // and the pod suffix.
    for pod in ["flowcpt-api…xk2p1", "flowcpt-api…m4q8z"] {
        for container in ["api", "envoy"] {
            let of_container: Vec<&String> = history
                .iter()
                .filter(|line| line.contains(&format!(" {pod:<20} {container:<10} ")))
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
    // Every column has its width, whichever names have been seen.
    assert_eq!(
        history[0],
        "14:00:06.000 flowcpt-api…xk2p1    api               api line 6"
    );
    assert_eq!(
        history[1],
        "14:00:06.010 flowcpt-api…xk2p1    envoy             envoy line 6"
    );
    assert_eq!(
        history[2],
        "14:00:06.020 flowcpt-api…m4q8z    api               api line 6"
    );
    let mut sorted = history.clone();
    sorted.sort();
    assert_eq!(history, sorted);
    follow.expect_quiet().await;

    // New lines arrive without any action.
    fake.log(NS, POD_B, "api", at(60_000), "WARN something new");
    assert_eq!(
        follow.line().await,
        "14:01:00.000 flowcpt-api…m4q8z    api         WARN  WARN something new"
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
            "14:00:01.000 redis-0              api         INFO  tenant_id=t_123 request_id=8b1c  request done",
            "14:00:02.000 redis-0              api         WARN  tenant_id=t_123 request_id=8b1d  payment intent expired",
            "14:00:03.000 redis-0              api         ERROR tenant_id=t_999 request_id=8b1e  card declined",
            "14:00:04.000 redis-0              api               a plain line with no level",
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
            "14:00:01.000 redis-0              api         INFO  request_id=8b1c  request done",
            "14:00:02.000 redis-0              api         WARN  request_id=8b1d  payment intent expired",
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

#[tokio::test]
async fn the_fields_of_a_tracing_span_are_shown_as_fields_of_the_line() {
    let fake = FakeCluster::start().await;
    seed_span_lines(&fake);
    fake.log(NS, "fc-api-0", "api", at(3_000), NESTED_SPAN_LINE);

    // As it comes: method, route, status, tenant, user, request id and busy
    // time, each when the line has it, then the other fields by name, then
    // the message. No `span` or `spans`, and none of the hidden fields: the
    // timestamp, the target, `otel.name`, `time.idle`, the name of the span.
    let mut follow = Follow::start(&fake, "span", &["flowcpt", "--no-color"]);
    assert_eq!(
        follow.lines(3).await,
        [
            "14:00:01.000 fc-api-0             api         INFO  http.method=GET http.route=/api/v1/sources http.status_code=200 tenant_id=3K6ehNLhnRsPfWhzjugmea user_id=auth0|6a86204723ca712d0c11f85a request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6 time.busy=5.53ms  close",
            "14:00:02.000 fc-api-0             api         WARN  http.method=POST http.route=/api/v1/flows http.status_code=404 tenant_id=8Hq2mVxWc4TzPb7LdYn3Ka request_id=9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11 time.busy=1.21ms  close",
            "14:00:03.000 fc-api-0             api         INFO  http.method=GET http.route=/api/v1/sources tenant_id=3K6ehNLhnRsPfWhzjugmea request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6 rows=3 table=sources  sources loaded",
        ]
    );

    // A list of names replaces all of that: these and no others, in the
    // order given, a hidden field like any other.
    let mut follow = Follow::start(
        &fake,
        "span-fields",
        &[
            "flowcpt",
            "--no-color",
            "--fields",
            "time.idle,target,http.route,timestamp",
        ],
    );
    assert_eq!(
        follow.lines(3).await,
        [
            "14:00:01.000 fc-api-0             api         INFO  time.idle=36.3ms target=flowcpt_telemetry http.route=/api/v1/sources timestamp=2026-11-18T20:39:01.187944Z  close",
            "14:00:02.000 fc-api-0             api         WARN  time.idle=9.04ms target=flowcpt_telemetry http.route=/api/v1/flows timestamp=2026-11-18T20:39:01.412307Z  close",
            "14:00:03.000 fc-api-0             api         INFO  target=flowcpt_api::sources http.route=/api/v1/sources timestamp=2026-11-18T20:39:01.180112Z  sources loaded",
        ]
    );

    // `all` is every field, the hidden ones too, in the default order.
    let mut follow = Follow::start(
        &fake,
        "span-all",
        &["flowcpt", "--no-color", "--fields", "all"],
    );
    assert_eq!(
        follow.lines(3).await,
        [
            "14:00:01.000 fc-api-0             api         INFO  http.method=GET http.route=/api/v1/sources http.status_code=200 tenant_id=3K6ehNLhnRsPfWhzjugmea user_id=auth0|6a86204723ca712d0c11f85a request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6 time.busy=5.53ms otel.name=\"GET /api/v1/sources\" target=flowcpt_telemetry time.idle=36.3ms timestamp=2026-11-18T20:39:01.187944Z  close",
            "14:00:02.000 fc-api-0             api         WARN  http.method=POST http.route=/api/v1/flows http.status_code=404 tenant_id=8Hq2mVxWc4TzPb7LdYn3Ka request_id=9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11 time.busy=1.21ms otel.name=\"POST /api/v1/flows\" target=flowcpt_telemetry time.idle=9.04ms timestamp=2026-11-18T20:39:01.412307Z  close",
            "14:00:03.000 fc-api-0             api         INFO  http.method=GET http.route=/api/v1/sources tenant_id=3K6ehNLhnRsPfWhzjugmea request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6 name=load_sources rows=3 table=sources target=flowcpt_api::sources timestamp=2026-11-18T20:39:01.180112Z  sources loaded",
        ]
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
        "14:00:01.000 fc-api-0             api         INFO  http.route=/api/v1/sources  close"
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
        "14:00:02.000 fc-api-0             api         WARN  http.route=/api/v1/flows  close"
    );
    follow.expect_quiet().await;

    // A field that is hidden by default is hidden from the eye only: a
    // filter sees it.
    let mut follow = Follow::start(
        &fake,
        "span-hidden",
        &[
            "flowcpt",
            "--no-color",
            "--field",
            "otel.name=POST /api/v1/flows",
        ],
    );
    assert_eq!(
        follow.line().await,
        "14:00:02.000 fc-api-0             api         WARN  http.method=POST http.route=/api/v1/flows http.status_code=404 tenant_id=8Hq2mVxWc4TzPb7LdYn3Ka request_id=9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11 time.busy=1.21ms  close"
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
            "14:00:01.000 fc-api-0             api         INFO  request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6  close",
            "14:00:03.000 fc-api-0             api         INFO  name=load_sources table=sources rows=3 request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6  sources loaded",
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
            "14:00:02.000 redis-0              api         WARN  request_id=8b1d  payment intent expired",
            "14:00:03.000 redis-0              api         ERROR request_id=8b1e  card declined",
            "14:00:04.000 redis-0              api               a plain line with no level",
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
    assert_eq!(lines[0], "14:00:01.001 redis-0              api");
    assert_eq!(
        lines[1],
        "14:00:01.002 redis-0              api               thread 'tokio-runtime-worker' (241) panicked at src/payments.rs:42:19:"
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
            "14:00:03.000 redis-0              api         ERROR request_id=8b1e  card declined",
            "14:00:04.000 redis-0              api               a plain line with no level",
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
            " redis-0              EVNT              Killing: Container api failed liveness probe, will be restarted"
        ),
        "{line}"
    );

    // A line after it stands in the same columns.
    fake.log(NS, "redis-0", "api", Utc::now(), "INFO back again");
    let line = follow.line().await;
    assert!(
        line.ends_with(" redis-0              api         INFO  INFO back again"),
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
        "14:00:02.000 redis-0              api         WARN  request_id=8b1d  payment intent expired"
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
fields = ["request_id", "http.status_code", "time.idle", "http.route"]

[templates.everything]
targets = ["flowcpt"]
fields = ["all"]
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
        "14:00:02.000 redis-0              api         WARN  request_id=8b1d  payment intent expired"
    );
    assert_eq!(from_template, from_flags);
}

#[tokio::test]
async fn the_fields_of_a_template_are_the_fields_shown_in_that_order() {
    let fake = FakeCluster::start().await;
    seed_span_lines(&fake);

    // These and no others, in the order of the list, which is not the
    // default order, and with a field in it that is hidden by default.
    let mut follow = Follow::start_with_config(
        &fake,
        "template-fields",
        TEMPLATES,
        &["--template", "requests", "--no-color"],
    );
    assert_eq!(
        follow.lines(2).await,
        [
            "14:00:01.000 fc-api-0             api         INFO  request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6 http.status_code=200 time.idle=36.3ms http.route=/api/v1/sources  close",
            "14:00:02.000 fc-api-0             api         WARN  request_id=9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11 http.status_code=404 time.idle=9.04ms http.route=/api/v1/flows  close",
        ]
    );

    // The one word `all` is every field, as with the flag.
    let mut follow = Follow::start_with_config(
        &fake,
        "template-all",
        TEMPLATES,
        &["--template", "everything", "--no-color", "--tail", "1"],
    );
    assert_eq!(
        follow.line().await,
        "14:00:02.000 fc-api-0             api         WARN  http.method=POST http.route=/api/v1/flows http.status_code=404 tenant_id=8Hq2mVxWc4TzPb7LdYn3Ka request_id=9d0e7a55-0b0e-4c0c-8a4f-2f1f3f2b6c11 time.busy=1.21ms otel.name=\"POST /api/v1/flows\" target=flowcpt_telemetry time.idle=9.04ms timestamp=2026-11-18T20:39:01.412307Z  close"
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
            "14:00:01.000 fc-api-0             api         INFO  close",
            "14:00:02.000 fc-api-0             api         WARN  close",
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
            "14:00:01.000 fc-api-0             api         INFO  time.idle=36.3ms  close",
            "14:00:02.000 fc-api-0             api         WARN  time.idle=9.04ms  close",
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
            "14:00:03.000 redis-0              api         ERROR card declined",
            "14:00:04.000 redis-0              api               a plain line with no level",
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
            "14:00:03.000 redis-0              api         ERROR request_id=8b1e  card declined",
            "14:00:04.000 redis-0              api               a plain line with no level",
        ]
    );
    follow.expect_quiet().await;
}

// ---- columns ----------------------------------------------------------

const FC: &str = "fc-stg";
const FC_API_POD: &str = "fc-api-7f698fb95f-vxfgs";
const FC_WEB_POD: &str = "fc-web-api-5c8b6d7f9-m4q8z";

/// The deployments `fc-api` and `fc-web-api` of the namespace `fc-stg`, one
/// pod each; the second pod has a sidecar with a long name.
fn seed_fc(fake: &FakeCluster) {
    for (deployment, set, pod, containers) in [
        ("fc-api", "fc-api-7f698fb95f", FC_API_POD, &["api"][..]),
        (
            "fc-web-api",
            "fc-web-api-5c8b6d7f9",
            FC_WEB_POD,
            &["api", "linkerd-proxy"],
        ),
    ] {
        let labels = [("app", deployment)];
        fake.apply(
            &deployments(FC),
            workload_object(FC, deployment, &labels, None),
        );
        fake.apply(
            &replica_sets(FC),
            workload_object(FC, set, &labels, Some(owner("Deployment", deployment))),
        );
        fake.apply(
            &pods(FC),
            pod_object(FC, pod, &labels, Some(owner("ReplicaSet", set)), containers),
        );
    }
}

#[tokio::test]
async fn a_long_pod_name_is_cut_in_the_middle_and_the_columns_flag_changes_that() {
    let fake = FakeCluster::start().await;
    seed_fc(&fake);
    fake.log(FC, FC_API_POD, "api", at(1_000), "INFO ready");

    // Acceptance 6, with no flag at all: in the 20 cells of the pod column
    // the name keeps the workload and the pod suffix. Every pod here is a
    // deployment's, and the name is not shortened to hash and suffix.
    let mut follow = Follow::start(&fake, "columns-cut", &["fc-stg/fc-api"]);
    assert_eq!(
        follow.line().await,
        "14:00:01.000 fc-api…vxfgs         api         INFO  INFO ready"
    );

    // A column that is wide enough shows it whole.
    let mut follow = Follow::start(
        &fake,
        "columns-wide",
        &["fc-stg/fc-api", "--columns", "pod=40"],
    );
    assert_eq!(
        follow.line().await,
        "14:00:01.000 fc-api-7f698fb95f-vxfgs                  api         INFO  INFO ready"
    );

    // A width of zero hides the column, and the message moves left.
    let mut follow = Follow::start(
        &fake,
        "columns-hidden",
        &["fc-stg/fc-api", "--columns", "pod=0"],
    );
    assert_eq!(
        follow.line().await,
        "14:00:01.000 api         INFO  INFO ready"
    );

    // A column that just holds the name shows it whole too, hash and all.
    let mut follow = Follow::start(
        &fake,
        "columns-exact",
        &["fc-stg/fc-api", "--columns", "pod=23"],
    );
    assert_eq!(
        follow.line().await,
        "14:00:01.000 fc-api-7f698fb95f-vxfgs api         INFO  INFO ready"
    );

    // A side to cut on, and several columns at once.
    let mut follow = Follow::start(
        &fake,
        "columns-sides",
        &[
            "fc-stg/fc-api",
            "--columns",
            "time=0,pod=10:start,container=2,level=4",
        ],
    );
    assert_eq!(follow.line().await, "…95f-vxfgs a…  INFO INFO ready");
}

#[tokio::test]
async fn a_columns_flag_that_cannot_be_read_is_refused() {
    let fake = FakeCluster::start().await;
    for (columns, expected) in [
        ("node=24", "`node` is not a column"),
        ("pod=wide", "`wide` is not a width"),
        ("pod=24:left", "`left` is not a side to cut on"),
    ] {
        let (mut command, kubeconfig, config) = follow_command(
            &fake,
            "columns-refused",
            "",
            &["fc-stg", "--columns", columns],
        );
        let output = timeout(Duration::from_secs(15), command.output())
            .await
            .expect("gossip should stop in time")
            .unwrap();
        let _ = std::fs::remove_file(kubeconfig);
        let _ = std::fs::remove_file(config);
        assert_eq!(output.status.code(), Some(2), "{columns}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{columns}: {stderr}");
    }
}

const COLUMNS_CONFIG: &str = r#"
[columns]
pod = { width = 12, cut = "end" }
container = { width = 5 }

[templates.narrow]
targets = ["fc-stg/fc-web-api"]
columns = { pod = { cut = "start" }, level = { width = 0 } }
"#;

#[tokio::test]
async fn the_columns_of_the_config_file_and_of_a_template_apply_and_a_flag_goes_over_them() {
    let fake = FakeCluster::start().await;
    seed_fc(&fake);
    fake.log(FC, FC_WEB_POD, "linkerd-proxy", at(1_000), "WARN refused");

    // The `[columns]` of the config file, for a run without a template.
    let mut follow = Follow::start_with_config(
        &fake,
        "columns-config",
        COLUMNS_CONFIG,
        &["fc-stg/fc-web-api"],
    );
    assert_eq!(
        follow.line().await,
        "14:00:01.000 fc-web-api-… link…  WARN  WARN refused"
    );

    // A template changes what it names, part by part: the side of the pod
    // column, with the width of the config file, and no level column.
    let mut follow = Follow::start_with_config(
        &fake,
        "columns-template",
        COLUMNS_CONFIG,
        &["--template", "narrow"],
    );
    assert_eq!(
        follow.line().await,
        "14:00:01.000 …6d7f9-m4q8z link… WARN refused"
    );

    // And a flag goes over both, again part by part.
    let mut follow = Follow::start_with_config(
        &fake,
        "columns-flag",
        COLUMNS_CONFIG,
        &[
            "--template",
            "narrow",
            "--columns",
            "pod=8,container=13,level=5",
        ],
    );
    assert_eq!(
        follow.line().await,
        "14:00:01.000 …9-m4q8z linkerd-proxy  WARN  WARN refused"
    );
}

// ---- --last -----------------------------------------------------------------

const LAST_CONFIG: &str = r#"# Kept by gossip when it quits.
[templates.last]
targets = ["flowcpt"]
container = "api"
filter = { any = [
  { field = "kind", op = "==", value = "event" },
  { field = "level", op = ">=", value = "warn" },
] }
fields = ["request_id"]
columns = { pod = { width = 0 } }
"#;

#[tokio::test]
async fn last_follows_what_the_template_of_that_name_holds() {
    let fake = FakeCluster::start().await;
    seed_tenant_lines(&fake);

    // Acceptance 8, as far as it goes without a terminal: `--last` resumes
    // the targets, the filter, the fields and the columns that were kept.
    let mut follow = Follow::start_with_config(&fake, "last", LAST_CONFIG, &["--last"]);
    assert_eq!(
        follow.lines(3).await,
        [
            "14:00:02.000 api         WARN  request_id=8b1d  payment intent expired",
            "14:00:03.000 api         ERROR request_id=8b1e  card declined",
            "14:00:04.000 api               a plain line with no level",
        ]
    );
    follow.expect_quiet().await;

    // It is `--template last`, to the letter.
    let mut by_name =
        Follow::start_with_config(&fake, "last-template", LAST_CONFIG, &["--template", "last"]);
    assert_eq!(
        by_name.line().await,
        "14:00:02.000 api         WARN  request_id=8b1d  payment intent expired"
    );

    // A run without a terminal keeps nothing for `--last`: the config file
    // is what it was.
    let pid = follow.child.id().expect("gossip is running").to_string();
    std::process::Command::new("kill")
        .args(["-INT", &pid])
        .status()
        .unwrap();
    timeout(Duration::from_secs(10), follow.child.wait())
        .await
        .expect("gossip should stop in time")
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&follow.config).unwrap(),
        LAST_CONFIG
    );
}

#[tokio::test]
async fn last_without_a_kept_session_is_an_error_that_says_what_it_is_for() {
    let fake = FakeCluster::start().await;
    let (mut command, kubeconfig, config) = follow_command(&fake, "no-last", "", &["--last"]);
    let output = timeout(Duration::from_secs(15), command.output())
        .await
        .expect("gossip should stop in time")
        .unwrap();
    let _ = std::fs::remove_file(kubeconfig);
    let _ = std::fs::remove_file(config);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("gossip: there is no session to resume yet: `--last` follows what"),
        "{stderr}"
    );
}

// ---- the output without a terminal, byte for byte -------------------------

/// What `gossip follow` with these arguments writes to stdout, as bytes:
/// read until `lines` line ends have come, then until it has been quiet for
/// a moment.
async fn stdout_bytes(fake: &FakeCluster, test: &str, args: &[&str], lines: usize) -> String {
    use tokio::io::AsyncReadExt;

    let (mut command, kubeconfig, config) = follow_command(fake, test, "", args);
    let mut child = command.spawn().expect("gossip should start");
    let mut stdout = child.stdout.take().unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let complete = bytes.iter().filter(|byte| **byte == b'\n').count() >= lines;
        let wait = if complete {
            Duration::from_millis(600)
        } else {
            Duration::from_secs(15)
        };
        match timeout(wait, stdout.read(&mut chunk)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(count)) => bytes.extend_from_slice(&chunk[..count]),
            Ok(Err(error)) => panic!("cannot read from gossip: {error}"),
            Err(_) if complete => break,
            Err(_) => panic!(
                "gossip wrote less than {lines} lines in time:\n{}",
                String::from_utf8_lossy(&bytes)
            ),
        }
    }
    let _ = std::fs::remove_file(kubeconfig);
    let _ = std::fs::remove_file(config);
    String::from_utf8(bytes).expect("the output is UTF-8")
}

/// A little of everything a stream carries: events about pods and about a
/// workload, JSON, logfmt and plain lines, and a panic of several lines.
fn seed_pinned(fake: &FakeCluster) {
    seed_fc(fake);
    fake.apply(&pods(FC), pod_object(FC, "redis-0", &[], None, &["redis"]));
    // The events are older than every line, and each has a second of its
    // own, so the order of the stream does not hang on which watch answers
    // first.
    for (name, regarding, container, reason, note, kind, count, time) in [
        (
            "scaled",
            ("Deployment", "fc-web-api"),
            None,
            "ScalingReplicaSet",
            "Scaled up replica set fc-web-api-5c8b6d7f9 to 1",
            "Normal",
            1,
            at(-3_000),
        ),
        (
            "killing",
            ("Pod", FC_API_POD),
            Some("api"),
            "Killing",
            "Container api failed liveness probe, will be restarted",
            "Normal",
            1,
            at(-2_000),
        ),
        (
            "backoff",
            ("Pod", FC_WEB_POD),
            Some("api"),
            "BackOff",
            "Back-off restarting failed container",
            "Warning",
            3,
            at(-1_000),
        ),
    ] {
        let event = event_object(
            FC, name, regarding, container, reason, note, kind, count, time,
        );
        fake.apply(&events(FC), event);
    }
    fake.log(FC, FC_API_POD, "api", at(1_000), SPAN_LINES[0]);
    fake.log(
        FC,
        FC_WEB_POD,
        "linkerd-proxy",
        at(2_000),
        r#"time=2026-11-18T14:00:02Z level=warn msg="connection refused" peer=10.0.0.7:8080"#,
    );
    fake.log(
        FC,
        "redis-0",
        "redis",
        at(3_000),
        "1:M 18 Nov 2026 14:00:03.000 * Ready to accept connections",
    );
    fake.log(FC, FC_WEB_POD, "api", at(4_000), "ERROR upstream is gone");
    // A panic, and the JSON line that ends its block.
    for (index, line) in [
        "thread 'main' (7) panicked at src/main.rs:5:5:",
        "boom",
        "stack backtrace:",
        "   0: fc_api::main",
        r#"{"level":"DEBUG","message":"worker restarted","attempt":2}"#,
    ]
    .iter()
    .enumerate()
    {
        fake.log(FC, FC_API_POD, "api", at(5_000 + index as i64), line);
    }
}

/// The whole history, events included: everything since long before the
/// fixed times of the test.
const PINNED_ARGS: [&str; 3] = ["fc-stg", "--since", "36500d"];

/// THE OUTPUT WITHOUT A TERMINAL IS PINNED HERE, byte for byte. With stdout
/// or stdin not a terminal, nothing of the interactive mode may show: no
/// escape code, no carriage return, no status line, no prompt.
///
/// The JSON and the raw output are what the prototype wrote, to the byte:
/// the two files were written by the prototype's own binary. The text
/// output is what the prototype wrote apart from the prefix columns of
/// SPEC-interactive 5: their fixed widths, and the pod column, which holds
/// the full name of the pod cut in the middle where the prototype shortened
/// it to hash and suffix. None of the three may change without the spec
/// saying so.
const PINNED_TEXT: &str = include_str!("pinned/follow.txt");
const PINNED_JSON: &str = include_str!("pinned/follow.jsonl");
const PINNED_RAW: &str = include_str!("pinned/follow.raw");

#[tokio::test]
async fn the_output_without_a_terminal_is_pinned_byte_for_byte() {
    let fake = FakeCluster::start().await;
    seed_pinned(&fake);

    let lines = |text: &str| text.matches('\n').count();
    let text = stdout_bytes(&fake, "pinned-text", &PINNED_ARGS, lines(PINNED_TEXT)).await;
    let args = [&PINNED_ARGS[..], &["--output", "json"]].concat();
    let json = stdout_bytes(&fake, "pinned-json", &args, lines(PINNED_JSON)).await;
    let args = [&PINNED_ARGS[..], &["--raw"]].concat();
    let raw = stdout_bytes(&fake, "pinned-raw", &args, lines(PINNED_RAW)).await;

    assert_eq!(text, PINNED_TEXT, "the text output:\n{text}");
    assert_eq!(json, PINNED_JSON, "the JSON output:\n{json}");
    assert_eq!(raw, PINNED_RAW, "the raw output:\n{raw}");

    // Nothing in any of them that only a terminal would want.
    for output in [&text, &json, &raw] {
        assert!(!output.contains('\u{1b}'), "an escape code: {output:?}");
        assert!(!output.contains('\r'), "a carriage return: {output:?}");
    }
}
