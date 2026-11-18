# Gossip

Gossip is a Rust tool for following Kubernetes pod logs and cluster events
across restarts and rollouts. It is built as a core library plus a command
line, step by step from [SPEC.md](SPEC.md).

This README describes what exists in the repository today, not what the spec
plans.

## Status

Two commands work against a cluster: `gossip list` shows what is in it, and
`gossip follow` follows the logs of a set of targets across restarts and
rollouts. Cluster events in the stream and templates from a config file are
not built yet.

| Crate | Kind | Holds today |
| --- | --- | --- |
| `crates/gossip-core` | library | cluster access, discovery, target resolution, log streams with reconnection, the session that merges them, the line parser, filters; and, not yet used by the command line, the config file |
| `crates/gossip` | binary `gossip` | `gossip list`, `gossip follow` |

Gossip is a client only: it reads from the cluster and never creates, changes
or deletes anything in it. It reaches the cluster through the kubeconfig
(`KUBECONFIG` or `~/.kube/config`) with the same credentials `kubectl` uses,
exec plugins such as `kubelogin` included, and it honours `HTTPS_PROXY` and a
`proxy-url` in the kubeconfig.

## Commands

### `gossip list`

Shows what is in a cluster, so that names need not be typed from memory.

| Form | Shows |
| --- | --- |
| `gossip list contexts` | the contexts of the kubeconfig, the current one marked with `*` |
| `gossip list namespaces` | the namespaces of the cluster, with status and age |
| `gossip list <namespace>` | the deployments of the namespace, with ready / desired replicas and age |
| `gossip list <namespace>/<deployment>` | the pods of the deployment, with phase, restarts, age and containers |

`--context <name>` picks the cluster; without it the current context is used.
`--json` prints the same as JSON for scripts.

```bash
gossip list flowcpt/flowcpt-api
```

```
NAME                     PHASE     RESTARTS   AGE     CONTAINERS
flowcpt-api-7d9f-xk2p1   Running   0          3h12m   migrate (init), api, envoy
```

Names are exact here; globs are not accepted. A cluster that cannot be reached
ends the command with exit code 1 and the error.

### `gossip follow`

```bash
gossip follow flowcpt/flowcpt-api
```

```
14:02:11.482 7d9f-xk2p1 api  WARN  request_id=8b1c tenant_id=t_123  payment intent expired
14:02:12.007 7d9f-m4q8z api  INFO  request_id=8b1d tenant_id=t_456  request done
14:02:13.650 -- pod flowcpt-api-7d9f-xk2p1 went away
14:02:14.120 -- pod flowcpt-api-7d9f-q9w2x appeared; following api
14:02:14.903 7d9f-q9w2x api  INFO  version=0.14.2  starting
```

Follow prints the last lines of every container the targets select, then new
lines as they arrive, merged into one stream ordered by time. It keeps going
when a pod restarts, is replaced or is rolled out: a new pod is picked up
without any action, and a stream that breaks is opened again from the last
line seen, so that no line is lost and none is printed twice. Stop it with
Ctrl-C.

#### Targets

A target is a path `namespace[/workload[/pod[/container]]]` whose names may be
globs. Several targets can be given, across namespaces.

| Target | Follows |
| --- | --- |
| `flowcpt` or `flowcpt/*` | every pod of the namespace, whatever owns it |
| `flowcpt/flowcpt-api` | the pods of the workload of that name: a Deployment, StatefulSet, DaemonSet, Job or CronJob |
| `flowcpt/cronjob:nightly-sync` | the same, of one kind only; the prefixes are `deployment`, `statefulset`, `daemonset`, `job`, `cronjob` |
| `flowcpt/flowcpt-*` | every workload whose name matches the glob |
| `flowcpt/-/flowcpt-api-7d9f-xk2p1` | one pod, given directly |
| `monitoring/prometheus/*/exporter` | one container of every pod of a workload |
| `flowcpt*` | every namespace whose name matches, also ones created later |

A target that matches nothing at start is not an error: a notice says so and
Gossip keeps watching until something matches.

#### Flags

| Flag | Meaning | Default |
| --- | --- | --- |
| `--context <name>` | the kubeconfig context of the cluster | the current context |
| `--since <duration>` | history per container at start: `30s`, `5m`, `1h` | none |
| `--tail <n>` | last n lines per container at start | 20, unless `--since` is given |
| `--container <glob>` | only containers whose name matches | all |
| `--level <level>` | only lines at this level or above: `trace`, `debug`, `info`, `warn`, `error` | all |
| `--grep <text>` | only lines whose message contains the text | none |
| `--match <regex>` | only lines whose message matches the regular expression | none |
| `--field <key=value>` | only lines whose field has the value; repeatable | none |
| `--fields <a,b,c>` | show only these fields of a line, in this order | all, longest last |
| `--long-names` | keep pod names whole | shortened when every pod is a deployment's |
| `--output text\|json` | the rendering | `text` |
| `--raw` | print the raw line, with no prefix, parsing or colour | off |
| `--no-color` | no colours | colours when stdout is a terminal |

The filter flags combine with "and". A line that has no level does not pass
`--level`.

#### Lines

The format of every line is detected on its own:

- A JSON object becomes a level (`level`, `severity` or `lvl`), a message
  (`message` or `msg`) and fields. The application's own timestamp is kept as
  the field `timestamp`.
- A logfmt line (`level=info msg="request done" status=200`) is read the same
  way.
- Anything else is text. Its level is guessed from a leading `ERROR`, `WARN`,
  `INFO`, `DEBUG` or `TRACE`, also after a timestamp or a bracketed module.
- A line that starts with whitespace, with `at ` or with `Caused by:` joins
  the line before it, so that a stack trace is one record.

#### Output

The text output is one line per record: the time in the local zone, the pod
and the container in a colour per pod, the level, the fields, and the message.
Lines of the run before the current one are dimmed. Notices from Gossip itself
(a pod appeared or went away, a stream was reopened, a target matches nothing)
start with `--`.

`--output json` prints one record per line as JSON, for `jq`:

```bash
gossip follow flowcpt/flowcpt-api --output json | jq 'select(.level == "error") | .message'
```

A container that has restarted, or that waits in a crash loop, first gets the
tail of its previous run: the output of the run that crashed.

#### Exit

Ctrl-C stops following and exits 0. A cluster that cannot be reached at start
exits 1 with the error; once following, Gossip retries instead and says so in
a notice.

## Build and install

Gossip needs the latest stable Rust toolchain (edition 2024);
`rust-toolchain.toml` selects it for you when you use `rustup`.

Build and run from a checkout:

```bash
cargo build
```

```bash
cargo run -p gossip -- --help
```

Install the `gossip` binary into `~/.cargo/bin`:

```bash
cargo install --path crates/gossip
```

## Development

The same three checks run in CI on every push and pull request:

```bash
cargo fmt --check
```

```bash
cargo clippy --workspace -- -D warnings
```

```bash
cargo test --workspace
```

## Licence

Apache-2.0, see [LICENSE](LICENSE).
