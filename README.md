# Gossip

*Everything your cluster says behind your back.*

Gossip is a terminal tool that follows the logs of Kubernetes workloads. It keeps following them across pod restarts and rollouts, merges the cluster's own events into the same stream, reads JSON and logfmt lines as data, and can be changed while it runs: filter, pause, add a service, save the session.

<!-- generated:status -->
v0.1.0 · Gossip is young and has been tried by hand against one cluster so far, so expect bugs. Please tell us what you see.
<!-- /generated:status -->

<!-- generated:sample -->
```text
14:02:11.482 api-6c9f7b8d54-x7k2p api         INFO  http.method=GET http.route=/cart http.status_code=200  request finished
14:02:14.000 api-6c9f7b8d54-x7k2p 💀                Killing: Stopping container api
14:02:15.000 -- pod api-6c9f7b8d54-r9t2v appeared; following api
14:02:15.000 api-6c9f7b8d54-r9t2v 🚀                Started: Started container api
14:02:15.310 api-6c9f7b8d54-r9t2v api         INFO  address=0.0.0.0:8080  listening
```
<!-- /generated:sample -->

> **Use with caution.** Gossip is written largely by an AI coding agent working from [SPEC.md](SPEC.md), with human review, a strict test gate and CI. That makes it fast to build and easy to audit, but it is still young software. Gossip only reads from your cluster and never creates, changes or deletes anything in it, but it runs with the credentials of your kubeconfig. Until 1.0, treat every release as a preview. If it does something surprising, that's a bug: please report it.

## Install

<!-- generated:install -->
**Linux**

- x64: [gossip-0.1.0-linux-x64.tar.gz](https://github.com/Open-Breach/gossip/releases/download/v0.1.0/gossip-0.1.0-linux-x64.tar.gz)
- arm64: [gossip-0.1.0-linux-arm64.tar.gz](https://github.com/Open-Breach/gossip/releases/download/v0.1.0/gossip-0.1.0-linux-arm64.tar.gz)

The binary is static and runs on any distribution. Unpack the archive and put `gossip` on the `PATH`; on arm64, take the other archive:

```bash
curl -LO https://github.com/Open-Breach/gossip/releases/download/v0.1.0/gossip-0.1.0-linux-x64.tar.gz
tar xzf gossip-0.1.0-linux-x64.tar.gz
sudo install gossip-0.1.0-linux-x64/gossip /usr/local/bin/
```

**macOS**

- Apple silicon: [gossip-0.1.0-macos-arm64.tar.gz](https://github.com/Open-Breach/gossip/releases/download/v0.1.0/gossip-0.1.0-macos-arm64.tar.gz)

```bash
curl -LO https://github.com/Open-Breach/gossip/releases/download/v0.1.0/gossip-0.1.0-macos-arm64.tar.gz
tar xzf gossip-0.1.0-macos-arm64.tar.gz
sudo install gossip-0.1.0-macos-arm64/gossip /usr/local/bin/
```

The binary is not signed or notarised: if macOS refuses to start it, `xattr -d com.apple.quarantine /usr/local/bin/gossip` lifts the quarantine a download carries.

**Windows**

- x64: [gossip-0.1.0-windows-x64.zip](https://github.com/Open-Breach/gossip/releases/download/v0.1.0/gossip-0.1.0-windows-x64.zip)

In PowerShell, unpack the archive and put its folder on the `PATH` of the session; to keep it, move `gossip.exe` to a folder that is on your `PATH`:

```powershell
Expand-Archive gossip-0.1.0-windows-x64.zip -DestinationPath .
$env:Path += ";$PWD\gossip-0.1.0-windows-x64"
```

The binary is not signed: if Windows refuses to start it, `Unblock-File gossip-0.1.0-windows-x64\gossip.exe` lifts the mark a download carries.

**From source**

With the latest stable Rust toolchain, which `rust-toolchain.toml` selects when you use `rustup`:

```bash
git clone https://github.com/Open-Breach/gossip
cargo install --path gossip/crates/gossip
```

Every archive holds the binary, this README, the licence and the licences of the crates it is built from. `SHA256SUMS.txt` on the [release page](https://github.com/Open-Breach/gossip/releases/tag/v0.1.0) has the checksums.
<!-- /generated:install -->

## Using Gossip

One session, read top to bottom. The cluster in it is made up: a namespace `shop` with the deployments `api` and `web`. Every command below was run by this release's binary against a stand-in API server, and every line under it is what the binary printed.

### 1. Find your way

`gossip list` shows what is there, so no name is typed from memory: the contexts of the kubeconfig, the namespaces of the cluster, the deployments of a namespace.

<!-- generated:walk-find -->
```text
$ gossip list contexts
CURRENT   NAME        CLUSTER     AUTHINFO   NAMESPACE
*         demo        demo        dev
          demo-test   demo-test   dev        shop

$ gossip list namespaces
NAME          STATUS   AGE
default       Active   96d
kube-system   Active   96d
shop          Active   41d

$ gossip list shop
NAME   READY   AGE
api    2/2     41d
web    1/1     12d
```
<!-- /generated:walk-find -->

### 2. Follow one deployment

A target is `namespace/workload`. Gossip prints the last lines of every container of the deployment's pods, then each new line as it is logged. A JSON line is read as data: the level, the fields, then the message.

<!-- generated:walk-follow -->
```text
$ gossip follow shop/api
14:01:51.000 api-6c9f7b8d54-x7k2p api         INFO  http.method=GET http.route=/products http.status_code=200  request finished
14:01:52.500 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/cart http.status_code=200  request finished
14:01:55.800 api-6c9f7b8d54-x7k2p api         DEBUG key=products:42  cache hit
14:02:02.482 api-6c9f7b8d54-x7k2p api         INFO  http.method=POST http.route=/cart http.status_code=201  request finished
```
<!-- /generated:walk-follow -->

### 3. What a restart looks like

One of the two pods is killed while Gossip follows. No key is pressed: the cluster's events arrive in the stream, each with an emoji, Gossip's own notices start with `--`, and the first lines of the new pod follow.

<!-- generated:walk-restart -->
```text
14:02:05.000 api-6c9f7b8d54-x7k2p 💀                Killing: Stopping container api
14:02:05.000 -- pod api-6c9f7b8d54-r9t2v appeared; following api
14:02:05.000 -- api in pod api-6c9f7b8d54-r9t2v is waiting: ContainerCreating
14:02:05.000 api-6c9f7b8d54-r9t2v 📍                Scheduled: Successfully assigned shop/api-6c9f7b8d54-r9t2v to node-2
14:02:05.210 api-6c9f7b8d54-x7k2p api         INFO  shutting down
14:02:06.000 api-6c9f7b8d54-r9t2v 📦                Pulled: Container image "registry.example/shop/api:1.8.2" already present on machine
14:02:06.000 api-6c9f7b8d54-r9t2v ℹ️                Created: Created container api
14:02:06.000 api-6c9f7b8d54-r9t2v 🚀                Started: Started container api
14:02:06.340 api-6c9f7b8d54-r9t2v api         INFO  address=0.0.0.0:8080  listening
14:02:08.000 -- pod api-6c9f7b8d54-x7k2p went away
```
<!-- /generated:walk-restart -->

### 4. Narrow it

`:` opens a command line on the last line of the terminal. `:level info` hides the debug lines from here on, and `:filter` prints the filter that is in force, as a template would hold it.

<!-- generated:walk-narrow -->
```text
:level info
14:02:20.000 -- filter: (kind == event or level >= info)
14:02:21.100 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/cart/7 http.status_code=200  request finished
:filter
14:02:24.000 -- filter = { any = [
  { field = "kind", op = "==", value = "event" },
  { field = "level", op = ">=", value = "info" },
] }
```
<!-- /generated:walk-narrow -->

### 5. Stop to read

`Space` holds the output, and the status line counts the lines that wait. `Space` again prints them in order and goes on.

<!-- generated:walk-pause -->
```text
paused · 0 lines waiting
paused · 3 lines waiting
14:02:31.000 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/products http.status_code=200  request finished
14:02:31.200 api-6c9f7b8d54-r9t2v api         INFO  http.method=POST http.route=/checkout http.status_code=502  request finished
14:02:31.400 api-6c9f7b8d54-r9t2v api         WARN  elapsed_ms=2140  payment provider slow
```
<!-- /generated:walk-pause -->

### 6. Add a second service

`:add shop/web` brings another deployment into the same stream, without a restart. `:targets` says what is followed.

<!-- generated:walk-add -->
```text
:add shop/web
14:02:40.000 -- added shop/web; following shop/api (2 pods), shop/web (1 pod)
14:02:41.150 web-5d8c7f6b9-tq6wn  web         INFO  path=/ status=200  page rendered
14:02:41.900 web-5d8c7f6b9-tq6wn  web         WARN  elapsed=2.1s upstream=api  slow upstream
:targets
14:02:44.000 -- shop/api: 2 pods
14:02:44.000 -- shop/web: 1 pod
```
<!-- /generated:walk-add -->

### 7. Keep it

`:save shop` writes the session as it is now, with its targets and its filter, to the config file as a template. Gossip says where; this session ran with a config file in a scratch directory.

<!-- generated:walk-save -->
```text
:save shop
14:02:50.000 -- saved as the template shop in /tmp/gossip-readme/.config/gossip/gossip.toml
```
<!-- /generated:walk-save -->

### 8. Come back

`q` quits. `gossip follow --last` picks up what was followed when Gossip last quit, and `gossip follow --template shop` starts the saved session by its name. Both follow the two deployments again, with the filter.

<!-- generated:walk-back -->
```text
$ gossip follow --last
14:01:52.500 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/cart http.status_code=200  request finished
14:02:06.340 api-6c9f7b8d54-r9t2v api         INFO  address=0.0.0.0:8080  listening
14:02:21.100 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/cart/7 http.status_code=200  request finished
14:02:31.000 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/products http.status_code=200  request finished
14:02:31.200 api-6c9f7b8d54-r9t2v api         INFO  http.method=POST http.route=/checkout http.status_code=502  request finished
14:02:31.400 api-6c9f7b8d54-r9t2v api         WARN  elapsed_ms=2140  payment provider slow
14:02:41.150 web-5d8c7f6b9-tq6wn  web         INFO  path=/ status=200  page rendered
14:02:41.900 web-5d8c7f6b9-tq6wn  web         WARN  elapsed=2.1s upstream=api  slow upstream

$ gossip follow --template shop
14:01:52.500 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/cart http.status_code=200  request finished
14:02:06.340 api-6c9f7b8d54-r9t2v api         INFO  address=0.0.0.0:8080  listening
14:02:21.100 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/cart/7 http.status_code=200  request finished
14:02:31.000 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/products http.status_code=200  request finished
14:02:31.200 api-6c9f7b8d54-r9t2v api         INFO  http.method=POST http.route=/checkout http.status_code=502  request finished
14:02:31.400 api-6c9f7b8d54-r9t2v api         WARN  elapsed_ms=2140  payment provider slow
14:02:41.150 web-5d8c7f6b9-tq6wn  web         INFO  path=/ status=200  page rendered
14:02:41.900 web-5d8c7f6b9-tq6wn  web         WARN  elapsed=2.1s upstream=api  slow upstream
```
<!-- /generated:walk-back -->

### 9. Keep a record

The cluster keeps little history. `--record` writes every record to a file, also what the filter hides from the screen, and `gossip replay` plays the file back through the same output.

<!-- generated:walk-record -->
```text
$ gossip follow shop/api --level warn --record api.jsonl
14:02:31.400 api-6c9f7b8d54-r9t2v api         WARN  elapsed_ms=2140  payment provider slow

$ gossip replay api.jsonl
14:01:52.500 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/cart http.status_code=200  request finished
14:02:06.340 api-6c9f7b8d54-r9t2v api         INFO  address=0.0.0.0:8080  listening
14:02:21.000 api-6c9f7b8d54-m4q8z api         DEBUG key=cart:7  cache miss
14:02:21.100 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/cart/7 http.status_code=200  request finished
14:02:31.000 api-6c9f7b8d54-m4q8z api         INFO  http.method=GET http.route=/products http.status_code=200  request finished
14:02:31.200 api-6c9f7b8d54-r9t2v api         INFO  http.method=POST http.route=/checkout http.status_code=502  request finished
14:02:31.400 api-6c9f7b8d54-r9t2v api         WARN  elapsed_ms=2140  payment provider slow
```
<!-- /generated:walk-record -->

## Reference

<!-- generated:reference -->
### Commands

| Command | Does |
| --- | --- |
| `gossip list <what>` | List the contexts of the kubeconfig, or the namespaces, deployments and pods of a cluster. |
| `gossip follow <target>...` | Follow the logs of a set of targets, across restarts and rollouts. |
| `gossip replay <file>` | Play a recording back, through the same output and with the same keys as `follow`. |
| `gossip templates` | List the templates of the config file, or show one. |
| `gossip templates show <name>` | Print one template as it stands in the config file. |
| `gossip help [command]` | Print this message or the help of the given subcommand(s). |

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

| Flag | Meaning | Default |
| --- | --- | --- |
| `<target>...` | What to follow; see [Targets](#targets). |  |
| `--template <name>` | Follow what a template of the config file says. Flags given beside it override its values; the filter flags narrow its filter. |  |
| `--last` | Follow what was followed when an interactive `gossip follow` last quit: its targets, filter, fields and columns. The same as `--template last`. | off |
| `--context <name>` | The kubeconfig context of the cluster. | the current context |
| `--since <duration>` | History per container at start, as a duration: 30s, 5m, 1h. |  |
| `--tail <n>` | History per container at start, as a number of lines. | 20, unless --since is given |
| `--no-events` | Leave the cluster's events out of the stream. | off |
| `--container <glob>` | Only containers whose name matches the glob. |  |
| `--no-filter` | Drop the filter of the template, or of the defaults, so that only the filter flags apply. | off |
| `--level <level>` | Only lines at this level or above: trace, debug, info, warn, error. |  |
| `--grep <text>` | Only lines whose message contains the text. |  |
| `--match <regex>` | Only lines whose message matches the regular expression. |  |
| `--field <key=value>` | Only lines whose field has the value; can be given several times. |  |
| `--raw` | Print the raw line, with no prefix, parsing or colour. | off |
| `--output <text\|json>` | The rendering: text, or json with one record per line. | text |
| `--no-color` | No colours. | colours when stdout is a terminal |
| `--fields <keys>` | Show only these fields of a line, in this order: a,b,c. `all` shows every field. Without it: the fields a request is read by first, the rest in alphabetical order, and the noisy ones hidden. |  |
| `--columns <columns>` | The widths of the columns time, pod, container and level, in terminal cells, and the side a longer value is cut on (start, end, middle or none): pod=24:middle,container=8. A width of 0 hides the column. | time=12:none,pod=20:middle,container=10:end,level=5:end |
| `--no-emoji` | Mark events with `EVNT` instead of an emoji. | emoji when stdout is a terminal and the locale is UTF-8 |
| `--interactive` | Read keys while following, also when stdin is not a terminal: Space pauses, `:` opens a command line, `?` lists the keys and commands. Stdout must be a terminal. | on when stdin and stdout are both a terminal |
| `--no-interactive` | Read no keys: follow and print, and nothing else. | off |
| `--record <file>` | Write every record to this file as well, one JSON object per line as `--output json` prints them, before the filter: the recording holds what the filter hides too. The file is appended to. `gossip replay` plays it back. |  |

The filter flags (`--level`, `--grep`, `--match`, `--field`) combine with "and", and they are about lines: events pass them, and `--no-events` is what leaves events out. A line that has no level always passes `--level`: unknown is not the same as unimportant, and a plain-text panic must not vanish under `--level warn`.

`--since` and `--tail` are one setting: giving either one decides both.

`--output json` prints one record per line as JSON, for `jq`, always with every field. An event is a record of kind `event` with its `reason`, `type` and `count` among the fields. With stdin or stdout not a terminal, in a pipe or into a file, no key is read and nothing but the records is written.

`--record` writes every record to the file as one JSON object per line, as `--output json` prints it, before the filter: the filter still applies to the screen, and the recording holds what it hid as well. The file is appended to. Nothing rotates or compresses a recording: left running, it grows at the rate of the stream.

#### `gossip replay`

| Flag | Meaning | Default |
| --- | --- | --- |
| `<file>` | The recording to play: a file that `gossip follow --record` or the `:record` command wrote. |  |
| `--speed <factor>` | How fast to play: 1 is the pace it was recorded at, 10 is ten times faster, and 0 is as fast as the terminal takes it. | 0 |
| `--from <time>` | Leave out what is older than this time: 14:02 or 14:02:11 on the day the recording begins, in the local zone as the time column shows it, or a full time such as 2026-11-18T14:02:11Z. |  |
| `--to <time>` | Leave out what is newer than this time; written like --from. |  |
| `--no-filter` | Drop the filter of the template, or of the defaults, so that only the filter flags apply. | off |
| `--level <level>` | Only lines at this level or above: trace, debug, info, warn, error. |  |
| `--grep <text>` | Only lines whose message contains the text. |  |
| `--match <regex>` | Only lines whose message matches the regular expression. |  |
| `--field <key=value>` | Only lines whose field has the value; can be given several times. |  |
| `--raw` | Print the raw line, with no prefix, parsing or colour. | off |
| `--output <text\|json>` | The rendering: text, or json with one record per line. | text |
| `--no-color` | No colours. | colours when stdout is a terminal |
| `--fields <keys>` | Show only these fields of a line, in this order: a,b,c. `all` shows every field. Without it: the fields a request is read by first, the rest in alphabetical order, and the noisy ones hidden. |  |
| `--columns <columns>` | The widths of the columns time, pod, container and level, in terminal cells, and the side a longer value is cut on (start, end, middle or none): pod=24:middle,container=8. A width of 0 hides the column. | time=12:none,pod=20:middle,container=10:end,level=5:end |
| `--no-emoji` | Mark events with `EVNT` instead of an emoji. | emoji when stdout is a terminal and the locale is UTF-8 |
| `--interactive` | Read keys while following, also when stdin is not a terminal: Space pauses, `:` opens a command line, `?` lists the keys and commands. Stdout must be a terminal. | on when stdin and stdout are both a terminal |
| `--no-interactive` | Read no keys: follow and print, and nothing else. | off |

Replay plays a recording through the same output as `follow`: every line renders as it did live. A time for `--from` and `--to` is written as the time column shows it, `14:02` or `14:02:11.482`, in the local zone and on the day the recording begins; or in full, as `2026-11-18 14:02` or `2026-11-18T14:02:11Z`.

Since a recording holds what the filter hid, a replay can be filtered in another way than the live run was. On a terminal the keys and the filter, field and column commands work on it as well, and while a replay is paused the file is not read on. The commands that ask something of a cluster (`:add`, `:drop`, `:targets`, `:events`, `:container`, `:save`, `:load`, `:record`) are refused with a notice: the source is the file.

#### `gossip list`

| Flag | Meaning | Default |
| --- | --- | --- |
| `<what>` | `contexts`, `namespaces`, a namespace, or `namespace/deployment`. |  |
| `--context <name>` | The kubeconfig context of the cluster. | the current context |
| `--json` | Print JSON instead of a table. | off |

| Form | Shows |
| --- | --- |
| `gossip list contexts` | the contexts of the kubeconfig, the current one marked with `*` |
| `gossip list namespaces` | the namespaces of the cluster, with status and age |
| `gossip list <namespace>` | the deployments of the namespace, with ready / desired replicas and age |
| `gossip list <namespace>/<deployment>` | the pods of the deployment, with phase, restarts, age and containers |

### Keys

With a terminal on both sides, Gossip reads keys while the stream runs. The stream scrolls as it always does: there are no panes and no second screen, and the scrollback of the terminal stays what it is. A status line is the last line only while the command line is open, the output is paused, or a question waits.

While the stream scrolls:

| Key | Does |
| --- | --- |
| `Space` | Pause the output, and resume it. While it is paused the status line counts the lines that wait; they are printed, in order, when it resumes. |
| `Ctrl-L` | Clear the screen. |
| `:` | Open the command line. |
| `/` | Open the command line with `grep ` typed. |
| `?` | List the keys and the commands. |
| `q`, `Ctrl-C` | Quit. |

While the command line is open:

| Key | Does |
| --- | --- |
| `Enter` | Apply the line. An empty line closes the command line. |
| `Esc` | Close the command line without applying. |
| any character | Typed into the line at the cursor. |
| `Backspace`, `Ctrl-H` | Erase the character before the cursor. On an empty line: close the command line. |
| `Delete`, `Ctrl-D` | Erase the character at the cursor. |
| `Left`, `Ctrl-B` | Move the cursor one character to the left. |
| `Right`, `Ctrl-F` | Move the cursor one character to the right. |
| `Home`, `Ctrl-A` | Move the cursor to the start of the line. |
| `End`, `Ctrl-E` | Move the cursor to the end of the line. |
| `Ctrl-U` | Erase from the start of the line to the cursor. |
| `Ctrl-W` | Erase the word before the cursor. |
| `Up`, `Ctrl-P` | The line entered before this one. |
| `Down`, `Ctrl-N` | The line entered after it. |
| `Ctrl-L` | Clear the screen. The line stays as it is. |
| `Ctrl-C` | Quit. |

While a question waits on the status line, as `:save` asks one:

| Key | Does |
| --- | --- |
| `y` | Yes. |
| `Ctrl-C` | Quit. |
| any other key | No: nothing is done by accident. |

Typing never pauses the stream: lines keep arriving above the command line. The terminal is in raw mode for the keys only, and out of it again on every way out: `q`, `Ctrl-C`, an error, `kill -TERM`, a panic.

`Space` pauses the output, not the session: the streams stay open and the filter keeps running. The status line reads `paused · 3 lines waiting` and counts. The buffer holds 10,000 records, or what `pause_buffer` in the config file says, `0` for no limit; beyond that the oldest are dropped and one notice says how many when the output resumes. Commands work while paused, and a new filter applies to what waits, so one can pause, narrow, and resume to see only what matters. `Ctrl-L` and `:clear` while paused also empty the buffer. While a recording runs, the status line of the pause shows its size.

### `:` commands

One command per line; `Enter` applies it and a notice answers it. A command that cannot be read prints its error and changes nothing. The arguments are written as for the flags, without quotes: the rest of the line is the argument.

| Command | Does |
| --- | --- |
| `:level <level>` | Only lines at this level or above: `trace`, `debug`, `info`, `warn`, `error`. |
| `:level off` | Take the level rule away. |
| `:grep <text>` | Only lines whose message contains the text. The text is the rest of the line, spaces included. |
| `:grep` | Take that rule away. |
| `:match <regex>` | Only lines whose message matches the regular expression. |
| `:match` | Take that rule away. |
| `:field <key>=<value>` | Only lines whose field has the value. It is the one rule on that key: a second `:field` for the key replaces the first. |
| `:field <key>` | Take the rules on that key away. |
| `:filter` | Print the filter. |
| `:filter off` | Remove every rule, the filter of the config file included. |
| `:add <target>...` | Follow more targets. Each starts with the history a target has at start. |
| `:drop <target>...` | Stop following them. A target is named as it was given. |
| `:targets` | Print what is followed, and how many pods each target has. |
| `:events on` | Show the cluster's events. |
| `:events off` | Leave them out. |
| `:container <glob>` | Only containers whose name matches the glob. |
| `:container off` | Every container. |
| `:fields <a,b,c>` | Show only these fields of a line, in this order. |
| `:fields all` | Show every field, the hidden ones too. |
| `:fields default` | The default choice of fields. |
| `:columns <columns>` | Set widths and cut sides, written as for `--columns`: `pod=24:middle,container=8`. |
| `:columns` | Print the columns. |
| `:columns default` | The columns the run started with. |
| `:save <name>` | Save the session as the template of that name in the config file: what it follows, with its filter, fields and columns. Asks `y/n` before it replaces a template that is there. The name `last` is refused: it is Gossip's own. |
| `:save` | Save to the template the session is: the one it was started from, loaded, or last saved as. Without one, the command line opens again with `save ` typed, to take a name. |
| `:load <name>` | Replace the session with a template. |
| `:templates` | List the templates. |
| `:templates show <name>` | Print one template as it stands in the config file. |
| `:record <file>` | Append every record to the file from now on, one JSON object per line, also what the filter hides. |
| `:record` | Say whether a recording runs, and how much it holds. |
| `:record off` | Stop recording. |
| `:clear` | Clear the screen. |
| `:q` | Quit. `:quit` does the same. |
| `:help` | List the keys and the commands, as `?` does. |

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

1. First the fields a request is read by, always in this order, each only when the line has it: `http.method`, `http.route`, `http.status_code`, `tenant_id`, `user_id`, `request_id`, `time.busy`.
2. Then every other field in alphabetical order.
3. Hidden, because they repeat the time, the route or the status, or are noise on every line: `timestamp`, `target`, `otel.name`, `otel.kind`, `otel.status_code`, `http.target`, `http.scheme`, `http.flavor`, `http.host`, `http.user_agent`, `http.client_ip`, `time.idle`, `name`, `spans`.

`--fields a,b,c` and `:fields a,b,c` replace all of this with the fields they name, in the order they name them, a hidden one like any other. `all` in place of the list shows every field, the hidden ones too, in the order above. `--output json` always has every field.

### Columns

The time, the pod, the container and the level each stand in a column of a fixed width, measured in terminal cells, so the message starts in the same place on every line. An emoji counts two cells.

| Column | Width | Cut | Holds |
| --- | --- | --- | --- |
| `time` | 12 | `none` | `HH:MM:SS.mmm`, local zone |
| `pod` | 20 | `middle` | the full pod name, or what an event is about, as in `deployment/api` |
| `container` | 10 | `end` | the container name; the emoji or `EVNT` for an event |
| `level` | 5 | `end` | `TRACE` to `ERROR`; empty for events |

A value shorter than its column is padded. A longer one is cut, and `…` marks the cut: `start` keeps the end (`…b8d54-x7k2p`), `end` keeps the start (`api-6c9f7b8…`), `middle` keeps both ends. With `none` a value is never cut and pushes the rest of the line to the right. A width of `0` hides the column, and the message moves left.

The pod column always gets the full name of the pod. Cut in the middle, the name of a deployment's pod keeps the workload and the pod suffix and loses the hash of the replica set: `api-6c9f7b8d54-x7k2p` is `api…x7k2p` in 12 cells, and whole in the 20 of the default.

`--columns` and `:columns` take a list such as `pod=24:middle,container=8`; `pod=:start` changes the side and leaves the width. The default is `time=12:none,pod=20:middle,container=10:end,level=5:end`. In the config file, `[columns]` holds the columns of every run and a template may hold its own:

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
| `tail` | history at start as a number of lines; 20 when neither `since` nor `tail` is given anywhere |
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

The `format` key of a template or of `[defaults]` takes the detection away: `"auto"`, `"json"`, `"logfmt"` or `"text"` for every container, or a table by container name, as in `format = { api = "json" }`. `"text"` shows lines untouched and does not join continuation lines.

### Events

The cluster's events about the followed pods, and about the workloads above them (replica set, deployment, stateful set, daemon set, job, cron job), are merged into the stream at the time they happened. An event carries an emoji in place of the container, one per reason:

| Emoji | Reasons |
| --- | --- |
| 📍 | `Scheduled` |
| 📦 | `Pulling`, `Pulled` |
| 🚀 | `Started` |
| 💀 | `Killing`, `Exited` |
| 🔥 | `BackOff` |
| 🧹 | `Evicted`, `Preempted` |
| ⚠️ | `Unhealthy`, and every other warning |
| ℹ️ | any other reason |

Normal events are dimmed and warnings take the warning colour. Kubernetes has no event for a container that exits, so Gossip makes `Exited` itself from what the pod reports. Events from before Gossip started are left out, unless `--since` reaches back to them.

`--no-emoji` marks events with `EVNT` instead; that is also what a pipe gets, and a terminal whose locale is not UTF-8. `--no-events` and `:events off` leave events out of the stream.
<!-- /generated:reference -->

## Known limits

- One cluster at a time.
- One connection per followed container, so a few hundred at once can reach the open-file limit (256 by default on macOS); `ulimit -n 4096` in the shell before starting raises it.
- The kubelet polls a container's log file, which adds a second or two that no client can remove.
- History reaches as far back as the node keeps the log files, and events for one hour. Record and replay are for anything longer.
- In a plain-text service, lines printed in the two seconds after a panic join the panic's record.
- Tried on Linux only so far; the macOS, Windows and arm64 Linux builds pass their smoke test and nothing more.

[SPEC.md](SPEC.md) · [SPEC-interactive.md](SPEC-interactive.md) · [SPEC-release.md](SPEC-release.md) · [Building and development](docs/development.md)

Apache-2.0, see [LICENSE](LICENSE).
