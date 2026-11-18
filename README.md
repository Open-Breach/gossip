# Gossip

Gossip follows the logs of a set of Kubernetes workloads from the command
line. It keeps following them across pod restarts and rollouts, merges the
cluster's own events about those pods into the same stream, and reads JSON
and logfmt lines as data: a level, a message and fields.

```bash
gossip follow flowcpt/flowcpt-api
```

```
14:02:11.482 7d9f-xk2p1 api  WARN  request_id=8b1c tenant_id=t_123  payment intent expired
14:02:11.490 7d9f-xk2p1 💀         Killing: Container api failed liveness probe, will be restarted
14:02:12.007 7d9f-m4q8z api  INFO  request_id=8b1d tenant_id=t_456  request done
14:02:13.004 7d9f-xk2p1 🚀         Started: Started container api
14:02:13.210 7d9f-xk2p1 api  INFO  version=0.14.2  starting
```

## Status

Gossip is a prototype, built from [SPEC.md](SPEC.md). This README describes
what exists in the repository today.

It is one Cargo workspace with two crates:

| Crate | Kind | Holds |
| --- | --- | --- |
| `crates/gossip-core` | library | cluster access, discovery, target resolution, log streams with reconnection, cluster events, the session that merges them, the line parser, filters, the config file |
| `crates/gossip` | binary `gossip` | the commands `list`, `follow` and `templates`, and the rendering |

The command line only presents what the core produces; nothing in it talks to
the cluster, and the core prints nothing. Gossip follows one cluster at a
time.

Gossip is a client only: it reads from the cluster and never creates, changes
or deletes anything in it. It reaches the cluster through the kubeconfig
(`KUBECONFIG` or `~/.kube/config`) with the same credentials `kubectl` uses,
exec plugins such as `kubelogin` included, and it honours `HTTPS_PROXY` and a
`proxy-url` in the kubeconfig.

The tests run the whole of it against a stand-in API server inside the test
code. Its behaviour against a real cluster is checked by hand; there are no
tests against a cluster in CI.

## Build and install

Gossip needs the latest stable Rust toolchain (edition 2024);
`rust-toolchain.toml` selects it for you when you use `rustup`.

Install the `gossip` binary into `~/.cargo/bin`:

```bash
cargo install --path crates/gossip
```

Or build and run from a checkout:

```bash
cargo run -p gossip -- --help
```

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
gossip follow <target>... [flags]
```

```bash
gossip follow --template <name> [flags]
```

Follow prints the last lines of every container the targets select, then new
lines as they arrive, merged with the cluster's events about those pods into
one stream ordered by time. It keeps going
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
| `--template <name>` | follow what a template of the config file says, in place of targets | none |
| `--no-filter` | drop the filter of the template, or of `[defaults]`, so that only the filter flags apply | off |
| `--context <name>` | the kubeconfig context of the cluster | the current context |
| `--since <duration>` | history per container at start: `30s`, `5m`, `1h` | none |
| `--tail <n>` | last n lines per container at start | 20, unless `--since` is given |
| `--no-events` | leave the cluster's events out | events on |
| `--container <glob>` | only containers whose name matches | all |
| `--level <level>` | only lines at this level or above: `trace`, `debug`, `info`, `warn`, `error` | all |
| `--grep <text>` | only lines whose message contains the text | none |
| `--match <regex>` | only lines whose message matches the regular expression | none |
| `--field <key=value>` | only lines whose field has the value; repeatable | none |
| `--fields <a,b,c>` | show only these fields of a line, in this order | all but `timestamp` and `target`, longest last |
| `--long-names` | keep pod names whole | shortened when every pod is a deployment's |
| `--no-emoji` | mark events with `EVNT` instead of an emoji | emoji when stdout is a terminal and the locale is UTF-8 |
| `--output text\|json` | the rendering | `text` |
| `--raw` | print the raw line, with no prefix, parsing or colour | off |
| `--no-color` | no colours | colours when stdout is a terminal |

The filter flags (`--level`, `--grep`, `--match`, `--field`) combine with
"and", and they are about lines: events pass them, and `--no-events` is what
leaves events out. A line that has no level always passes `--level`: unknown is
not the same as unimportant, and a plain-text panic must not vanish under
`--level warn`.

#### Lines

The format of every line is detected on its own:

- A JSON object becomes a level (`level`, `severity` or `lvl`), a message
  (`message` or `msg`) and fields. The application's own timestamp is kept as
  the field `timestamp`. A nested object is kept as one field, as JSON text.
- The fields of a `tracing` span are fields of the line. `tracing-subscriber`'s
  JSON formatter writes the fields of the current span under `span`, and the
  chain of spans the line was logged in under `spans`. Gossip merges the keys
  of all of them into the fields and keeps neither `span` nor `spans`. A field
  of the line itself wins over a span field of the same name, and an inner
  span over an outer one. So `--field tenant_id=t_123` finds a tenant whose
  id was recorded on the request's span, also for a line logged in a span
  inside that one. The name of the innermost span is the field `name`.
- A logfmt line (`level=info msg="request done" status=200`) is read the same
  way.
- Anything else is text. Its level is guessed from a leading `ERROR`, `WARN`,
  `INFO`, `DEBUG` or `TRACE`, also after a timestamp or a bracketed module.
- A line that starts with whitespace, with `at ` or with `Caused by:` joins
  the line before it, so that a stack trace is one record.
- A Rust panic is one record too. The line `thread '<name>' panicked at ...`
  opens a block, and every line after it joins that record until a blank
  line, a JSON or logfmt line, the next panic, or two seconds later.

#### Events

The cluster's events about the followed pods, and about the workloads above
them (replica set, deployment, stateful set, daemon set, job, cron job), are
merged into the stream at the time they happened. An event carries an emoji in
place of the container, one per reason:

| Emoji | Reasons |
| --- | --- |
| 📍 | `Scheduled` |
| 📦 | `Pulling`, `Pulled` |
| 🚀 | `Started` |
| 💀 | `Killing`, and `Exited`: a container that ended with an exit code other than zero |
| 🔥 | `BackOff`: a crash loop or an image pull back-off |
| ⚠️ | `Unhealthy` and every other warning |
| 🧹 | `Evicted`, `Preempted` |
| ℹ️ | any other reason |

Normal events are dimmed and warnings take the warning colour. Kubernetes has
no event for a container that exits, so Gossip makes `Exited` itself from what
the pod reports. Events from before Gossip started are left out, unless
`--since` reaches back to them.

#### Output

The text output is one line per record: the time in the local zone, the pod
and the container in a colour per pod, the level, the fields, and the message.
The fields are all of them, the longest last, without `timestamp` and
`target`: the time is at the front of the line already, and `target` is rarely
what one reads for. `--fields` names the ones to show and their order, those
two included, and `--output json` always has every field.

```bash
gossip follow fc-stg/fc-api --fields http.method,http.route,http.status_code,tenant_id,request_id,time.busy
```

```text
20:39:01.188 6c9d8f7b5-x2k9p api  INFO  http.method=GET http.route=/api/v1/sources http.status_code=200 tenant_id=3K6ehNLhnRsPfWhzjugmea request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6 time.busy=5.53ms  close
```

Lines of the run before the current one are dimmed. Notices from Gossip itself
(a pod appeared or went away, a stream was reopened, a target matches nothing)
start with `--`.

`--output json` prints one record per line as JSON, for `jq`. An event is a
record of kind `event` with its `reason`, `type` and `count` among the fields:

```bash
gossip follow flowcpt/flowcpt-api --output json | jq 'select(.level == "error") | .message'
```

A container that has restarted, or that waits in a crash loop, first gets the
tail of its previous run: the output of the run that crashed.

#### Exit

Ctrl-C stops following and exits 0. A cluster that cannot be reached at start
exits 1 with the error; once following, Gossip retries instead and says so in
a notice.

### `gossip templates`

| Form | Shows |
| --- | --- |
| `gossip templates` | the templates of the config file, with their targets |
| `gossip templates show <name>` | one template, as it stands in the config file |

```bash
gossip templates
```

```
NAME          TARGETS
dev-api       flowcpt/flowcpt-api, flowcpt/flowcpt-web-api
staging-all   flowcpt-stg
```

## Config file and templates

One TOML file holds defaults and templates: named, saved combinations of
targets, filters and options. It lives at `~/.config/gossip/gossip.toml` on
Linux and in the platform's config directory elsewhere; `$GOSSIP_CONFIG`
overrides the path. Gossip only reads it. Nothing secret goes in it:
credentials stay in the kubeconfig.

```toml
[defaults]
context = "aks-vulcan-01"
tail = 20
events = true

[templates.dev-api]
context = "aks-vulcan-01"
targets = ["flowcpt/flowcpt-api", "flowcpt/flowcpt-web-api"]
container = "api"
filter = { all = [
  { field = "level", op = ">=", value = "info" },
  { any = [
    { field = "f.tenant_id", op = "==", value = "t_123" },
    { field = "kind", op = "==", value = "event" },
  ] },
] }
fields = ["tenant_id", "request_id"]

[templates.staging-all]
context = "aks-vulcan-01"
targets = ["flowcpt-stg/*"]
since = "5m"
format = { api = "json", envoy = "text" }
```

```bash
gossip follow --template dev-api
```

A template holds what `follow` takes:

| Key | Value |
| --- | --- |
| `context` | the kubeconfig context of the cluster |
| `targets` | a list of targets |
| `container` | a glob: only containers whose name matches |
| `since` | history at start as a duration, `"5m"` |
| `tail` | history at start as a number of lines |
| `events` | `false` leaves the cluster's events out |
| `format` | `"auto"`, `"json"`, `"logfmt"` or `"text"` for every container, or a table by container name; `"text"` shows lines untouched and does not join continuation lines |
| `filter` | a filter, see below |
| `fields` | the list of fields of a line to show, in this order; `[]` shows the message only; left out, all but `timestamp` and `target` |
| `output` | `"text"` or `"json"` |

What is used is decided in layers: a flag on the command line, then the
template, then `[defaults]`, then the built-in defaults. `[defaults]` takes
every key of a template except `targets`, and applies to `gossip follow` with
or without a template. `since` and `tail` are one setting: the layer that
gives either one decides both.

The filter is the one thing a flag does not override. A filter flag (`--level`,
`--grep`, `--match`, `--field`) beside a template narrows the template's
filter: a line must pass both. `--no-filter` drops the template's filter
first, so that the flags are the whole filter:

```bash
gossip follow --template dev-api --grep timeout
```

```bash
gossip follow --template dev-api --no-filter --level error
```

An unknown key is an error that names the line, so that a typo in a template
does not silently follow the wrong thing.

### Filters

A filter is a rule, or `all`, `any` or `not` over filters, nested as deep as
needed:

```toml
filter = { field = "message", op = "contains", value = "expired" }
filter = { not = { field = "f.debug", op = "exists" } }
```

A rule compares a field of a record with a value.

| Fields | |
| --- | --- |
| `level` | `trace`, `debug`, `info`, `warn`, `error` |
| `message` | the message of a line or an event |
| `pod`, `container`, `namespace` | where the record came from |
| `kind` | `line`, `event` or `notice` |
| `f.<key>` | a field of the line, such as `f.tenant_id` |

| Operators | |
| --- | --- |
| `==`, `!=` | equal, not equal |
| `>=` | at this level or above; on `level` only |
| `contains` | the value is part of the field |
| `matches` | the regular expression finds a match in the field |
| `exists` | the record has the field; takes no value |

A rule on a field the record does not have is false, with two exceptions:
`!=` is the opposite of `==`, and a record with no level passes `level >=`.
In the config file a filter is exactly what is written: unlike the filter
flags, it applies to events too, which is why the example above names
`kind == event`. Notices from Gossip always pass.

## Known limits

- **Open files.** Every followed container holds a connection of its own, and
  so does every watch. Following a few hundred containers at once can reach
  the limit on open files, which is 256 by default on macOS. Opening a stream
  then fails with "too many open files" and is tried again, with a notice each
  time. Raise the limit in the shell before starting:

  ```bash
  ulimit -n 4096
  ```

## Development

The tests need no cluster. The same three checks run in CI on every push and
pull request:

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
