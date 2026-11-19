# Gossip

Gossip follows the logs of a set of Kubernetes workloads from the command
line. It keeps following them across pod restarts and rollouts, merges the
cluster's own events about those pods into the same stream, and reads JSON
and logfmt lines as data: a level, a message and fields.

```bash
gossip follow flowcpt/flowcpt-api
```

```
14:02:11.482 7d9f-xk2p1           api         WARN  tenant_id=t_123 request_id=8b1c  payment intent expired
14:02:11.490 7d9f-xk2p1           💀                Killing: Container api failed liveness probe, will be restarted
14:02:12.007 7d9f-m4q8z           api         INFO  tenant_id=t_456 request_id=8b1d  request done
14:02:13.004 7d9f-xk2p1           🚀                Started: Started container api
14:02:13.210 7d9f-xk2p1           api         INFO  version=0.14.2  starting
```

On a terminal the stream can be changed while it runs: `Space` pauses it,
`:level warn` narrows it, `:add flowcpt/flowcpt-web-api` brings another
workload in, `:save dev-api` keeps the session as a template. And what was
followed can be recorded and played back later.

## Status

Gossip is a prototype, built from [SPEC.md](SPEC.md) and, for the keyboard,
the columns, saving and replay, from [SPEC-interactive.md](SPEC-interactive.md).
This README describes what exists in the repository today.

It is one Cargo workspace with two crates:

| Crate | Kind | Holds |
| --- | --- | --- |
| `crates/gossip-core` | library | cluster access, discovery, target resolution, log streams with reconnection, cluster events, the session that merges them and can be changed while it runs, the line parser, filters, the config file and its writer |
| `crates/gossip` | binary `gossip` | the commands `list`, `follow`, `replay` and `templates`, the rendering, the interactive mode, recording |

The command line only presents what the core produces; nothing in it talks to
the cluster, and the core prints nothing and knows nothing of keys or of a
terminal. Gossip follows one cluster at a time.

Gossip is a client only: it reads from the cluster and never creates, changes
or deletes anything in it. It reaches the cluster through the kubeconfig
(`KUBECONFIG` or `~/.kube/config`) with the same credentials `kubectl` uses,
exec plugins such as `kubelogin` included, and it honours `HTTPS_PROXY` and a
`proxy-url` in the kubeconfig.

The tests run the whole of it against a stand-in API server inside the test
code, and the interactive mode against a terminal in memory. Its behaviour
against a real cluster and in a real terminal is checked by hand; there are
no tests against a cluster in CI.

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

```bash
gossip follow --last [flags]
```

Follow prints the last lines of every container the targets select, then new
lines as they arrive, merged with the cluster's events about those pods into
one stream ordered by time. It keeps going
when a pod restarts, is replaced or is rolled out: a new pod is picked up
without any action, and a stream that breaks is opened again from the last
line seen, so that no line is lost and none is printed twice. Stop it with
`q` or Ctrl-C.

On a terminal, keys change the stream while it runs; see
[Keys and commands](#keys-and-commands). With stdin or stdout not a terminal,
in a pipe or into a file, no key is read and nothing but the lines is
written.

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
| `--last` | follow what an interactive `gossip follow` was following when it last quit; the same as `--template last` | off |
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
| `--fields <a,b,c>` | show only these fields of a line, in this order; `all` shows every field | the request's fields first, the rest by name, the noisy ones hidden: see [Output](#output) |
| `--long-names` | keep pod names whole | shortened when every pod is a deployment's |
| `--columns <columns>` | the widths of the columns and the side a longer value is cut on: `pod=24:middle,container=8`; see [Columns](#columns) | `time=12:none,pod=20:middle,container=10:end,level=5:end` |
| `--no-emoji` | mark events with `EVNT` instead of an emoji | emoji when stdout is a terminal and the locale is UTF-8 |
| `--output text\|json` | the rendering | `text` |
| `--raw` | print the raw line, with no prefix, parsing or colour | off |
| `--no-color` | no colours | colours when stdout is a terminal |
| `--record <file>` | write every record to the file as well, before the filter; see [Recording](#recording) | off |
| `--interactive` | read keys also when stdin is not a terminal; stdout must be one | on when stdin and stdout are both a terminal |
| `--no-interactive` | read no keys | |

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
The first four stand in [columns](#columns) of a fixed width, so the message
starts in the same place on every line. Which fields, and in which order:

1. First the fields a request is read by, always in this order, each only
   when the line has it: `http.method`, `http.route`, `http.status_code`,
   `tenant_id`, `user_id`, `request_id`, `time.busy`.
2. Then every other field in alphabetical order.
3. Hidden: `timestamp` and `target`, since the time is at the front of the
   line already and where in the code a line was logged is rarely what one
   reads for; and `otel.name`, `otel.kind`, `otel.status_code`, `http.target`,
   `http.scheme`, `http.flavor`, `http.host`, `http.user_agent`,
   `http.client_ip`, `time.idle`, `name` and `spans`, which repeat the route or
   the status or are noise on every line.

```bash
gossip follow fc-stg/fc-api
```

```text
14:00:01.000 fc-api-0             api         INFO  http.method=GET http.route=/api/v1/sources http.status_code=200 tenant_id=3K6ehNLhnRsPfWhzjugmea user_id=auth0|6a86204723ca712d0c11f85a request_id=32e49c93-4c57-43a0-9439-c554e5fff1f6 time.busy=5.53ms  close
```

`--fields` replaces all of this with the fields it names, in the order it
names them, a hidden one like any other. `--fields all` shows every field,
the hidden ones too, in the order above. `--output json` always has every
field.

```bash
gossip follow fc-stg/fc-api --fields tenant_id,http.route,time.idle
```

```bash
gossip follow fc-stg/fc-api --fields all
```

The names live in one table, `KNOWN_FIELDS` in `crates/gossip/src/fields.rs`,
which is the only place to change them.

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

#### Columns

The time, the pod, the container and the level each stand in a column of a
fixed width, measured in terminal cells; an emoji counts two.

| Column | Width | Cut | Holds |
| --- | --- | --- | --- |
| `time` | 12 | `none` | `HH:MM:SS.mmm`, local zone |
| `pod` | 20 | `middle` | the pod name, or what an event is about, as in `deployment/fc-api` |
| `container` | 10 | `end` | the container name; the emoji or `EVNT` for an event |
| `level` | 5 | `end` | `TRACE` to `ERROR`; empty for events |

A value shorter than its column is padded. A longer one is cut, and `…`
marks the cut: `start` keeps the end (`…9c7-vxfgs`), `end` keeps the start
(`fc-api-7f698…`), `middle` keeps both ends. Cut in the middle, the name of a
deployment's pod keeps the workload and the pod suffix and loses the hash of
the replica set: `fc-api-7f698fb95f-vxfgs` is `fc-api…vxfgs`. With `none` a
value is never cut and pushes the rest of the line to the right. A width of
`0` hides the column, and the message moves left.

```bash
gossip follow fc-stg/fc-api --long-names --columns pod=40
```

```bash
gossip follow fc-stg/fc-api --columns pod=0,container=6
```

`pod=:start` changes the side and leaves the width. The columns come, part by
part, from the `--columns` flag, then the `columns` of a template, then the
`[columns]` table of the [config file](#config-file-and-templates), then the
widths above. Pod names are still shortened to the hash of the replica set
and the pod suffix while every pod followed is a deployment's, so such a name
fits its column as it is; `--long-names` keeps names whole, and then the cut
applies.

#### Keys and commands

With a terminal on both sides, Gossip reads keys while the stream runs. The
stream scrolls as it always does: there are no panes and no second screen,
and the scrollback of the terminal stays what it is. A status line is the
last line only while the command line is open or the output is paused.

| Key | Does |
| --- | --- |
| `Space` | pause the output, and resume it |
| `Ctrl-L` | clear the screen |
| `:` | open the command line |
| `/` | open the command line with `grep ` typed, so `/payment` is `:grep payment` |
| `Esc` | close the command line without applying |
| `q`, `Ctrl-C` | quit; `Ctrl-C` also while the command line is open |
| `?` | print the keys and the commands |

Any other key is ignored. Typing never pauses the stream: lines keep arriving
above the command line. One command per line; `Enter` applies it and a notice
answers it. A command that cannot be read prints its error and changes
nothing.

| Command | Does |
| --- | --- |
| `:level <level>` | only lines at this level or above; `:level off` takes the rule away |
| `:grep <text>`, `:match <regex>` | a rule on the message; the same command again replaces its own rule, and alone it takes its rule away |
| `:field <key=value>` | the rule on that field; `:field <key>` takes it away |
| `:filter` | print the filter as the TOML of a template; `:filter off` removes every rule |
| `:add <target>...` | follow more targets; their containers start with the history of `--tail` or `--since` |
| `:drop <target>...` | stop following them; a pod another target still matches stays |
| `:targets` | what is followed, and how many pods each target has |
| `:events on`, `:events off` | the cluster's events |
| `:container <glob>`, `:container off` | only containers whose name matches |
| `:fields <a,b,c>`, `:fields all`, `:fields default` | the fields shown, as `--fields` |
| `:columns <columns>`, `:columns`, `:columns default` | the columns, as `--columns`; alone it prints them |
| `:save [name]`, `:load <name>`, `:templates` | see [Templates from inside](#templates-from-inside) |
| `:record <file>`, `:record off`, `:record` | see [Recording](#recording) |
| `:clear` | as `Ctrl-L` |
| `:q` | quit |

The arguments are written as for the flags, without quotes: the rest of the
line is the argument. The command line has a history for the session (`Up`,
`Down`) and the usual editing: `Backspace`, `Delete`, `Left`, `Right`, `Home`,
`End`, `Ctrl-U` to empty the line and `Ctrl-W` to take the last word away.

A flag and a command build the same filter the same way: a flag is the first
rule, and a command replaces it or adds to it. `gossip follow x --level warn`
followed by `:level error` and `:grep timeout` is the filter of
`gossip follow x --level error --grep timeout`. Beside the filter of a
template they narrow, as the flags do.

**Pause.** `Space` pauses the output, not the session: the streams stay open
and the filter keeps running. The status line reads
`paused · 142 lines waiting` and counts. `Space` again prints what waited, in
order, and goes on. The buffer holds 10,000 records, or what `pause_buffer`
in the config file says, `0` for no limit; beyond that the oldest are dropped
and one notice says how many when the output resumes. Commands work while
paused, and a new filter applies to what waits, so one can pause, narrow, and
resume to see only what matters. `Ctrl-L` and `:clear` while paused also
empty the buffer.

The terminal is in raw mode for the keys only, and out of it again on every
way out: `q`, `Ctrl-C`, an error, `kill -TERM`, a panic.

#### Templates from inside

| Command | Does |
| --- | --- |
| `:save <name>` | write the session as it is at that moment as the template `<name>`; one that is there is written over only after `y` on the status line |
| `:save` | write back to the template the session was started from, loaded from or saved as last; without one, the command line opens again to take a name |
| `:load <name>` | replace the running session with a template: streams not in it close, with a notice per pod, new ones open, and its filter, fields and columns apply |
| `:templates` | the templates, as `gossip templates` prints them |
| `:templates show <name>` | one template as TOML |

A saved session holds the context, the targets, the container glob, the
history, the events switch, the filter as it is in force, the fields, and the
columns that differ from those of the config file. A name is made of letters,
digits, `-` and `_`.

Writing keeps everything else in the config file as it is, comments and
order included, and is atomic. A config file that cannot be read is never
written over; `:save` then says what is wrong with it.

On every quit of an interactive session Gossip keeps the session as the
template `last`, and `gossip follow --last` starts from it. A run without a
terminal keeps nothing, and `:save last` is refused.

#### Recording

The cluster keeps little history: a container's log until it rotates or the
pod goes, events for an hour. For anything longer, Gossip records what it
followed.

```bash
gossip follow fc-stg/fc-api --level warn --record api.jsonl
```

Every record goes to the file as one JSON object per line, as `--output json`
prints it, before the filter: the filter still applies to the screen, and the
recording holds what it hid as well. The file is appended to. From inside,
`:record <file>` starts a recording, `:record off` stops it, and `:record`
alone says whether one runs and how much it holds. While one runs, the status
line of the pause shows its size. Nothing rotates or compresses a recording:
left running, it grows at the rate of the stream.

[`gossip replay`](#gossip-replay) plays a recording back.

#### Exit

`q` or Ctrl-C stops following and exits 0. A cluster that cannot be reached at
start exits 1 with the error; once following, Gossip retries instead and says
so in a notice.

### `gossip replay`

```bash
gossip replay <file> [flags]
```

Replay plays a recording through the same output as `follow`: every line
renders exactly as it did live. It takes the filter flags of `follow`
(`--level`, `--grep`, `--match`, `--field`, `--no-filter`) and its output
flags (`--fields`, `--columns`, `--output`, `--raw`, `--no-color`,
`--long-names`, `--no-emoji`, `--interactive`, `--no-interactive`), and
these of its own:

| Flag | Meaning | Default |
| --- | --- | --- |
| `--speed <factor>` | `1` plays at the recorded pace, `10` ten times faster, `0` without waiting | `0` |
| `--from <time>` | leave out what is older | the start of the recording |
| `--to <time>` | leave out what is newer | its end |

A time is written as the time column shows it, `14:02` or `14:02:11.482`, in
the local zone and on the day the recording begins; or in full, as
`2026-11-18 14:02` or `2026-11-18T14:02:11Z`.

```bash
gossip replay api.jsonl --level error --from 14:02 --to 14:10
```

```bash
gossip replay api.jsonl --speed 1
```

Since a recording holds what the filter hid, a replay can be filtered in
another way than the live run was. On a terminal the keys and the commands
work on it as well: `Space`, `/`, `:level`, `:grep`, `:match`, `:field`,
`:filter`, `:fields`, `:columns`. While a replay is paused the file is not
read on. The commands that ask something of a cluster (`:add`, `:drop`,
`:targets`, `:events`, `:container`, `:save`, `:load`, `:record`) are refused
with a notice: the source is the file. A replay ends at the end of the
recording.

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
overrides the path. Gossip reads it, and writes to it in two cases: `:save`
in a running session, and the template `last` when an interactive session
quits (see [Templates from inside](#templates-from-inside)). A write keeps
everything else in the file as it is, comments included. Nothing secret goes
in it: credentials stay in the kubeconfig.

```toml
pause_buffer = 10000

[defaults]
context = "aks-vulcan-01"
tail = 20
events = true

[columns]
pod = { width = 20, cut = "middle" }
container = { width = 10, cut = "end" }
level = { width = 5 }

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
| `fields` | the list of fields of a line to show, in this order; `[]` shows the message only and `["all"]` every field; left out, the default of [Output](#output) |
| `output` | `"text"` or `"json"` |
| `columns` | a table of [columns](#columns), each with a `width`, a `cut`, or both; what it leaves out comes from `[columns]` |

What is used is decided in layers: a flag on the command line, then the
template, then `[defaults]`, then the built-in defaults. `[defaults]` takes
every key of a template except `targets` and `columns`, and applies to
`gossip follow` with or without a template. `since` and `tail` are one
setting: the layer that gives either one decides both.

Beside `[defaults]` and the templates, the file has two things of its own:

- `[columns]`, the columns of every run: `time`, `pod`, `container` and
  `level`, each a table with a `width` in terminal cells, a `cut` (`"start"`,
  `"end"`, `"middle"` or `"none"`), or both.
- `pause_buffer`, at the top of the file: how many records wait while the
  output is paused, `0` for no limit. Without it, 10,000.

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

The tests need no cluster and no terminal. The files in
`crates/gossip/tests/pinned/` hold the output of one scenario without a
terminal, byte for byte, as text, as JSON and as raw lines; a test compares
them, so that nothing reaches a pipe or a file that was not there before.

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
