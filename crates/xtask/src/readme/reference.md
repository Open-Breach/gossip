### Commands

{{commands}}

`gossip --version` prints the version, and `--help` after any command prints its help. Gossip reaches the cluster through the kubeconfig (`KUBECONFIG` or `~/.kube/config`) with the credentials `kubectl` uses, exec plugins included, and honours `HTTPS_PROXY` and a `proxy-url` in the kubeconfig.

`gossip follow` prints the last lines of every container its targets select, then new lines as they arrive, merged with the cluster's events about those pods into one stream ordered by time. It keeps going when a pod restarts, is replaced or is rolled out: a new pod is picked up without any action, and a stream that breaks is opened again from the last line seen, so no line is lost and none is printed twice. A container that has restarted first gets the tail of its previous run, dimmed. Notices from Gossip itself start with `--`.

Exit codes: `q` or `Ctrl-C` ends `follow` with 0. A cluster that cannot be reached at start ends any command with 1 and the error; once following, Gossip retries instead and says so in a notice. `gossip replay` ends with 0 at the end of the recording.

### Targets

A target is a path `namespace[/workload[/pod[/container]]]` whose names may be globs. Several targets can be given, across namespaces.

| Target | Follows |
| --- | --- |
| `shop` or `shop/*` | every pod of the namespace, whatever owns it |
| `shop/api` | the pods of the workload of that name: a Deployment, StatefulSet, DaemonSet, Job or CronJob |
| `shop/deployment:api` | the same, of one kind only; the prefixes are `deployment`, `statefulset`, `daemonset`, `job`, `cronjob` |
| `shop/a*` | every workload whose name matches the glob |
| `shop/-/api-6c9f7b8d54-x7k2p` | one pod, given directly |
| `shop/api/*/api` | one container of every pod of a workload |
| `shop*` | every namespace whose name matches, also ones created later |

A target that matches nothing at start is not an error: a notice says so and Gossip keeps watching until something matches. `gossip list` takes exact names, not globs.

### Flags

A flag that is a switch is off unless it is given. Where the Default column is empty, nothing is set.

#### `gossip follow`

{{flags-follow}}

The filter flags (`--level`, `--grep`, `--match`, `--field`) combine with "and", and they are about lines: events pass them, and `--no-events` is what leaves events out. A line that has no level always passes `--level`: unknown is not the same as unimportant, and a plain-text panic must not vanish under `--level warn`.

`--since` and `--tail` are one setting: giving either one decides both.

`--output json` prints one record per line as JSON, for `jq`, always with every field. An event is a record of kind `event` with its `reason`, `type` and `count` among the fields. With stdin or stdout not a terminal, in a pipe or into a file, no key is read and nothing but the records is written.

`--record` writes every record to the file as one JSON object per line, as `--output json` prints it, before the filter: the filter still applies to the screen, and the recording holds what it hid as well. The file is appended to. Nothing rotates or compresses a recording: left running, it grows at the rate of the stream.

#### `gossip replay`

{{flags-replay}}

Replay plays a recording through the same output as `follow`: every line renders as it did live. A time for `--from` and `--to` is written as the time column shows it, `14:02` or `14:02:11.482`, in the local zone and on the day the recording begins; or in full, as `2026-11-18 14:02` or `2026-11-18T14:02:11Z`.

Since a recording holds what the filter hid, a replay can be filtered in another way than the live run was. On a terminal the keys and the filter, field and column commands work on it as well, and while a replay is paused the file is not read on. The commands that ask something of a cluster (`:add`, `:drop`, `:targets`, `:events`, `:container`, `:save`, `:load`, `:record`) are refused with a notice: the source is the file.

#### `gossip list`

{{flags-list}}

| Form | Shows |
| --- | --- |
| `gossip list contexts` | the contexts of the kubeconfig, the current one marked with `*` |
| `gossip list namespaces` | the namespaces of the cluster, with status and age |
| `gossip list <namespace>` | the deployments of the namespace, with ready / desired replicas and age |
| `gossip list <namespace>/<deployment>` | the pods of the deployment, with phase, restarts, age and containers |

### Keys

With a terminal on both sides, Gossip reads keys while the stream runs. The stream scrolls as it always does: there are no panes and no second screen, and the scrollback of the terminal stays what it is. A status line is the last line only while the command line is open, the output is paused, or a question waits.

{{keys}}

Typing never pauses the stream: lines keep arriving above the command line. The terminal is in raw mode for the keys only, and out of it again on every way out: `q`, `Ctrl-C`, an error, `kill -TERM`, a panic.

`Space` pauses the output, not the session: the streams stay open and the filter keeps running. The status line reads `paused · 3 lines waiting` and counts. The buffer holds {{pause-buffer}} records, or what `pause_buffer` in the config file says, `0` for no limit; beyond that the oldest are dropped and one notice says how many when the output resumes. Commands work while paused, and a new filter applies to what waits, so one can pause, narrow, and resume to see only what matters. `Ctrl-L` and `:clear` while paused also empty the buffer. While a recording runs, the status line of the pause shows its size.

### `:` commands

One command per line; `Enter` applies it and a notice answers it. A command that cannot be read prints its error and changes nothing. The arguments are written as for the flags, without quotes: the rest of the line is the argument.

{{colon-commands}}

A saved session holds the context, the targets, the container glob, the history, the events switch, the filter as it is in force, the fields, and the columns that differ from those of the config file. A template name is made of letters, digits, `-` and `_`. Writing keeps everything else in the config file as it is, comments and order included, and is atomic. A config file that cannot be read is never written over; `:save` then says what is wrong with it.

On every quit of an interactive session Gossip keeps the session as the template `last`, and `gossip follow --last` starts from it. A run without a terminal keeps nothing.

### Filters

A flag and a command build the same filter the same way: a flag is the first rule, and a command replaces it or adds to it. `gossip follow shop/api --level warn` followed by `:level error` and `:grep timeout` is the filter of `gossip follow shop/api --level error --grep timeout`. Beside the filter of a template or of `[defaults]` they narrow: a line must pass both. `--no-filter` and `:filter off` drop that filter first.

In the config file a filter is a rule, or `all`, `any` or `not` over filters, nested as deep as needed:

```toml
filter = { field = "message", op = "contains", value = "expired" }
filter = { not = { field = "f.debug", op = "exists" } }
filter = { any = [
  { field = "kind", op = "==", value = "event" },
  { all = [
    { field = "level", op = ">=", value = "warn" },
    { field = "f.tenant_id", op = "==", value = "acme" },
  ] },
] }
```

A rule compares a field of a record with a value.

| Field | Holds |
| --- | --- |
| `level` | `trace`, `debug`, `info`, `warn`, `error` |
| `message` | the message of a line or an event |
| `pod`, `container`, `namespace` | where the record came from |
| `kind` | `line`, `event` or `notice` |
| `f.<key>` | a field of the line, such as `f.tenant_id` |

| Operator | True when |
| --- | --- |
| `==` | the field is equal to the value |
| `!=` | it is not; also when the record has no such field |
| `>=` | the level is this one or above; on `level` only |
| `contains` | the value is part of the field |
| `matches` | the regular expression finds a match in the field |
| `exists` | the record has the field; takes no value |

A rule on a field the record does not have is false, with two exceptions: `!=` is the opposite of `==`, and a record with no level passes `level >=`. In the config file a filter is exactly what is written: unlike the filter flags and commands, it applies to events too, which is why the example above names `kind == event`. Notices from Gossip always pass.

### Fields

The text output is one line per record: the time in the local zone, the pod and the container in a colour per pod, the level, the fields, and the message. Which fields, and in which order:

1. First the fields a request is read by, always in this order, each only when the line has it: {{fields-first}}.
2. Then every other field in alphabetical order.
3. Hidden, because they repeat the time, the route or the status, or are noise on every line: {{fields-hidden}}.

`--fields a,b,c` and `:fields a,b,c` replace all of this with the fields they name, in the order they name them, a hidden one like any other. `{{fields-all}}` in place of the list shows every field, the hidden ones too, in the order above. `--output json` always has every field.

### Columns

The time, the pod, the container and the level each stand in a column of a fixed width, measured in terminal cells, so the message starts in the same place on every line. An emoji counts two cells.

{{columns}}

A value shorter than its column is padded. A longer one is cut, and `…` marks the cut: `start` keeps the end (`…b8d54-x7k2p`), `end` keeps the start (`api-6c9f7b8…`), `middle` keeps both ends. With `none` a value is never cut and pushes the rest of the line to the right. A width of `0` hides the column, and the message moves left.

The pod column always gets the full name of the pod. Cut in the middle, the name of a deployment's pod keeps the workload and the pod suffix and loses the hash of the replica set: `api-6c9f7b8d54-x7k2p` is `api…x7k2p` in 12 cells, and whole in the 20 of the default.

`--columns` and `:columns` take a list such as `pod=24:middle,container=8`; `pod=:start` changes the side and leaves the width. The default is `{{columns-default}}`. In the config file, `[columns]` holds the columns of every run and a template may hold its own:

```toml
[columns]
pod = { width = 24, cut = "middle" }
container = { width = 8 }
```

The columns come, part by part, from the `--columns` flag, then the `columns` of a template, then `[columns]`, then the defaults above.

### Config file

One TOML file holds defaults and templates: named, saved combinations of targets, filters and options. It is `~/.config/gossip/gossip.toml` on Linux and `gossip.toml` in the platform's config directory elsewhere; `$GOSSIP_CONFIG` overrides the path. Gossip reads it, and writes to it in two cases: `:save` in a running session, and the template `last` when an interactive session quits. Nothing secret goes in it: credentials stay in the kubeconfig.

```toml
pause_buffer = 10000

[defaults]
context = "demo"
tail = 20
events = true

[columns]
pod = { width = 20, cut = "middle" }
container = { width = 10, cut = "end" }
level = { width = 5 }

[templates.shop]
context = "demo"
targets = ["shop/api", "shop/web"]
filter = { all = [
  { field = "level", op = ">=", value = "info" },
  { any = [
    { field = "f.tenant_id", op = "==", value = "acme" },
    { field = "kind", op = "==", value = "event" },
  ] },
] }
fields = ["tenant_id", "request_id"]

[templates.api-json]
targets = ["shop/api"]
container = "api"
since = "5m"
events = false
format = { api = "json" }
output = "json"
columns = { pod = { width = 40 } }
```

The keys of a template, which `[defaults]` takes too, except `targets` and `columns`:

| Key | Value |
| --- | --- |
| `context` | the kubeconfig context of the cluster |
| `targets` | a list of targets |
| `container` | a glob: only containers whose name matches |
| `since` | history at start as a duration, `"5m"` |
| `tail` | history at start as a number of lines; {{tail}} when neither `since` nor `tail` is given anywhere |
| `events` | `false` leaves the cluster's events out |
| `format` | how lines are read, see [Line formats](#line-formats) |
| `filter` | a filter, see [Filters](#filters) |
| `fields` | the list of fields of a line to show, in this order; `[]` shows the message only and `["all"]` every field |
| `output` | `"text"` or `"json"` |
| `columns` | a table of columns, each with a `width`, a `cut`, or both; what it leaves out comes from `[columns]` |

Beside `[defaults]` and the templates, the file has two things of its own: `[columns]`, the columns of every run, and `pause_buffer`, at the top of the file: how many records wait while the output is paused, `0` for no limit.

What is used is decided in layers: a flag on the command line, then the template, then `[defaults]`, then the built-in defaults. `since` and `tail` are one setting: the layer that gives either one decides both. The filter is the one thing a flag does not override: a filter flag narrows it. An unknown key is an error that names the line, so a typo in a template does not silently follow the wrong thing.

### Line formats

The format of every line is detected on its own:

- A JSON object becomes a level (`level`, `severity` or `lvl`), a message (`message` or `msg`) and fields. The application's own timestamp is kept as the field `timestamp`. A nested object is kept as one field, as JSON text.
- The fields of a `tracing` span are fields of the line. `tracing-subscriber`'s JSON formatter writes the fields of the current span under `span`, and the chain of spans the line was logged in under `spans`. Gossip merges the keys of all of them into the fields and keeps neither `span` nor `spans`. A field of the line itself wins over a span field of the same name, and an inner span over an outer one. The name of the innermost span is the field `name`.
- A logfmt line (`level=info msg="page rendered" status=200`) is read the same way.
- Anything else is text. Its level is guessed from a leading `ERROR`, `WARN`, `INFO`, `DEBUG` or `TRACE`, also after a timestamp or a bracketed module.
- A line that starts with whitespace, with `at ` or with `Caused by:` joins the line before it, so a stack trace is one record.
- A Rust panic is one record too. The line `thread '<name>' panicked at ...` opens a block, and every line after it joins that record until a blank line, a JSON or logfmt line, the next panic, or two seconds later.

The `format` key of a template or of `[defaults]` takes the detection away: {{formats}} for every container, or a table by container name, as in `format = { api = "json" }`. `"text"` shows lines untouched and does not join continuation lines.

### Events

The cluster's events about the followed pods, and about the workloads above them (replica set, deployment, stateful set, daemon set, job, cron job), are merged into the stream at the time they happened. An event carries an emoji in place of the container, one per reason:

{{events}}

Normal events are dimmed and warnings take the warning colour. Kubernetes has no event for a container that exits, so Gossip makes `Exited` itself from what the pod reports. Events from before Gossip started are left out, unless `--since` reaches back to them.

`--no-emoji` marks events with `{{event-tag}}` instead; that is also what a pipe gets, and a terminal whose locale is not UTF-8. `--no-events` and `:events off` leave events out of the stream.
