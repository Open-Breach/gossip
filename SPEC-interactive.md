# Gossip — Interactive mode and columns (round two)

Status: draft, 2026-11-18. Nothing of this is implemented yet. It adds to `SPEC.md`.

Round two makes a running `gossip follow` changeable from the keyboard: filters, targets and events can be changed while the stream runs, the output can be paused and cleared, the session can be saved as a template and loaded again, and the prefix columns get fixed widths. It adds to the prototype of `SPEC.md` and changes none of its behaviour when nobody presses a key.

## 1. Goal and scope

The prototype sets a session once, from flags or a template, and keeps it until `Ctrl-C`. In use that is the first thing one wants to change: narrow to one tenant after a line went past, add the second service, turn the events off for a minute, stop the scrolling to read. Round two adds that, in the terminal, without a full-screen interface.

In scope:

- A mutable session in `gossip-core`: filters, targets, events and output options can change while the stream runs.
- An interactive mode in the CLI, on only when stdout and stdin are a terminal: a command line opened with `:` or `/`, pause and resume, clear, quit.
- Pause with a bounded buffer, so reading costs no lines.
- Fixed-width prefix columns with configurable width and cut side, values cut with an ellipsis.
- Templates from inside the session: save, update, load, list, and `--last` to resume the previous session.
- Record and replay (section 9).

Not in scope, unchanged from `SPEC.md`:

- Everything about targets, parsing, filters, events and rendering of a line. The spec's section numbers are referenced here as `SPEC 4.8` and so on.
- Output to a pipe or a file. Without a terminal, `gossip follow` behaves exactly as today; none of this activates.
- The GUI, several clusters at once, scrolling back through history, search in what has already been printed. A terminal's own scrollback and `less` stay the tools for that.

The rule for the whole round: a session nobody touches behaves as it does today, line for line, except for the column widths of section 5.

## 2. Core: the mutable session

`Session::start` keeps its signature and still returns the stream of records. It now also returns a handle, `SessionHandle`, cloneable and usable from another task, with these methods. Every method is applied by the session's own task, in order, and answered with a `notice` record in the stream, so whoever reads the stream sees what changed.

| Method | Effect |
| --- | --- |
| `set_filter(Filter)` | replaces the filter; takes effect on the next record |
| `add_target(Target)` | resolves it (SPEC 4.3) and opens its streams; a target already followed is ignored |
| `remove_target(Target)` | closes the streams of pods no other target still matches; lines already received stay in the reorder buffer and are emitted |
| `set_events(bool)` | starts or stops the event watcher |
| `set_container(Option<Glob>)` | re-applies the container glob to every followed pod |
| `load(SessionSpec)` | replaces targets, filter, container and events in one step, the difference applied as above |
| `spec() -> SessionSpec` | the current targets, filter, container, events, since and tail, exactly what a template holds |
| `stop()` | what dropping the stream does today |

- `SessionSpec` is the one type behind a template, `[defaults]`, the command-line flags, and the saved session of section 6. The config module already has it in all but name; this round names it and uses it everywhere.
- Rendering options (fields, columns, colour, emoji, output format) are not in the session. They belong to the CLI's renderer, which gets the same kind of handle for the commands that change them.
- A change is cheap when it only touches the filter. A change of targets goes through the resolver and may take a second against the cluster; the notice is written when it is done, not when it is asked.
- Nothing in the core knows about keys or a command line. The CLI translates commands into handle calls; the GUI will do the same later.

## 3. Interactive mode

The model is `less`: the stream scrolls as it does today, and a key opens a one-line prompt at the bottom of the terminal. No panes, no alternate screen, no redraw of what was printed; the terminal's own scrollback stays usable.

### 3.1 When it is on

- On when stdin and stdout are both a terminal. `--no-interactive` turns it off; `--interactive` forces it on when only stdout is a terminal. Not a terminal on either side: today's behaviour, byte for byte.
- While on, the terminal is in raw mode for key reading only; lines are printed as today, with `\r\n` so raw mode does not stagger them. Raw mode is restored on quit, on `Ctrl-C`, and on panic.
- A one-line status at the bottom is shown only while the prompt is open or the stream is paused. It never stays when lines are flowing.

### 3.2 Keys

| Key | Does |
| --- | --- |
| `Space` | pause / resume (section 4) |
| `Ctrl-L` | clear the screen, keep streaming |
| `:` | open the command line |
| `/` | open the command line with `grep ` typed, so `/payment` is `:grep payment` |
| `Esc` | close the command line without applying |
| `q` or `Ctrl-C` | quit; `Ctrl-C` also works while the prompt is open |
| `?` | print the key and command list as notice lines |

Any other key is ignored. Typing a command never pauses the stream; it keeps scrolling above the prompt.

### 3.3 Commands

One command per line, `Enter` applies it, a `notice` confirms it, an unknown command or a bad argument prints the error as a notice and changes nothing. Arguments use the same syntax as the flags of SPEC 5.2.

| Command | Does |
| --- | --- |
| `:level <lvl>` | sets the level rule; `:level off` removes it |
| `:grep <text>` / `:match <regex>` | adds a message rule, joined by `and` to what is there; the same command again replaces its own previous rule |
| `:field <k=v>` | adds a field rule, same way; `:field <k>` alone removes rules on that key |
| `:filter` | prints the current filter as the TOML it would be in a template |
| `:filter off` | removes every rule |
| `:add <target>...` | follows more targets |
| `:drop <target>...` | stops following them |
| `:targets` | lists what is followed and how many pods each resolves to |
| `:events on` / `:events off` | the event switch |
| `:container <glob>` / `:container off` | the container glob |
| `:fields <a,b,c>` / `:fields all` / `:fields default` | the shown fields, as `--fields` |
| `:columns ...` | column widths, section 5 |
| `:save [name]`, `:load <name>`, `:templates` | section 6 |
| `:record ...` | section 9 |
| `:clear` | as `Ctrl-L` |
| `:q` | quit |

- The command line has a history for the session (`Up` / `Down`) and the usual line editing (`Backspace`, `Ctrl-U`, `Ctrl-W`, `Left` / `Right`, `Home` / `End`). No completion in this round.
- Flags on the command line of `gossip follow` and commands typed later build the same `Filter` the same way: a flag is the first rule, a command replaces or adds to it. Narrowing is the rule throughout, as decided for templates.

## 4. Pause, resume and clear

- `Space` pauses the output, not the session: streams stay open, the resolver keeps watching, the filter keeps running. Records that pass the filter go into a buffer instead of the terminal.
- The status line shows `paused · 142 lines waiting` and counts up.
- `Space` again resumes: the buffer is flushed in order, then live output continues. Nothing is lost up to the cap.
- The cap is 10,000 records (`pause_buffer` in the config file, `0` for unbounded). Beyond it the oldest are dropped, and on resume one notice says `dropped 2,310 lines while paused`.
- Commands work while paused, and a filter change applies to the buffered records too, so one can pause, narrow, and resume to see only what matters.
- `Ctrl-L` and `:clear` clear the terminal with the usual escape sequence and keep streaming; while paused they also empty the buffer, since clearing means "I have seen enough".
- Not a terminal: pause does not exist; `Ctrl-S` / `Ctrl-Q` of the terminal driver remain what they are.

## 5. Columns

Today the pod column is padded to the longest name seen, so every line carries a gap and the message starts somewhere else on every run. Round two gives the prefix fixed widths, so the message always starts at the same place and the eye has one column to follow.

| Column | Default width | Default cut | Holds |
| --- | --- | --- | --- |
| time | 12 | none | `HH:MM:SS.mmm`, local zone |
| pod | 20 | middle | the pod name, shortened as SPEC 5.3 says, or `replicaset/...` for an event on a workload |
| container | 10 | end | the container name; the emoji or `EVNT` for an event |
| level | 5 | end | `TRACE` to `ERROR`; empty for events and notices |

- A value longer than its column is cut and an ellipsis `…` (one cell) marks the cut: `start` keeps the end (`…9c7-vxfgs`), `end` keeps the start (`fc-api-7f698…`), `middle` keeps both ends and drops the middle (`fc-api…vxfgs`), which for a pod name keeps the workload and the pod suffix and drops the replica-set hash.
- A value shorter than its column is padded to it. Width `0` hides the column.
- With `--no-emoji`, `--no-color`, or no terminal, the same widths apply, with `EVNT` in the container column.
- Configuration: a `[columns]` table in the config file, each entry `width` and `cut`, and a `--columns pod=24:middle,container=8` flag and `:columns` command with the same syntax for one run. A template may carry a `columns` table too.

```toml
[columns]
pod = { width = 20, cut = "middle" }
container = { width = 10, cut = "end" }
level = { width = 5 }
```

- The widths are in terminal cells, not bytes: an emoji counts two, and the measurement uses the same width table as the fallback rule of SPEC 5.3. This is the one change that alters today's output without anyone pressing a key, and it is intended.

## 6. Templates from inside

A saved session is a template: targets, filter, container, events, since, tail, fields and columns are what a template holds (SPEC 6). Round two makes the config file writable by the core and reachable from the prompt.

| Command | Does |
| --- | --- |
| `:save <name>` | writes the current session as the template `<name>`; an existing one is overwritten after a `y/n` on the status line |
| `:save` | updates the template the session was started from (`--template`); without one, asks for a name |
| `:load <name>` | replaces the running session with the template, through `SessionHandle::load`; streams not in it close, new ones open, rendering options of the template apply |
| `:templates` | lists the templates as `gossip templates` does, as notice lines |
| `:templates show <name>` | prints one as TOML |

- `gossip follow --last` starts from the session as it was when Gossip last quit. On every quit, including `Ctrl-C`, the current session is written to the reserved template `last`. `--last` is a shortcut for `--template last`; `:save last` is refused.
- Writes to the config file keep everything else in it, comments included, and are atomic (write a temporary file, rename). A file the core cannot parse is never overwritten; `:save` then fails with the parse error.
- The template name is the TOML key: letters, digits, `-` and `_`.
- `gossip templates` from outside is unchanged and still read-only.

## 7. Acceptance

1. `gossip follow fc-stg/fc-api | cat` and `gossip follow ... > file` produce exactly what the prototype produces, apart from the column widths of section 5.
2. On a terminal, `:level warn` while following hides the info lines from the next record on, with one notice; `:level off` brings them back.
3. `:add fc-stg/fc-web-api` while following `fc-stg/fc-api` brings the second deployment's lines in without a restart; `:drop fc-stg/fc-api` stops the first and its pods' events; `:targets` shows the result.
4. `Space` during a burst stops the scrolling; the status line counts; `Space` again prints the waiting lines in order, then live ones, with no gap and no repeat. With `pause_buffer = 100` and 150 lines arriving, resume shows 100 and one notice about 50 dropped.
5. `Ctrl-L` clears the screen and the next line arrives at the top.
6. Pod `fc-api-7f698fb95f-vxfgs` renders as `fc-api…vxfgs` in a 20-cell pod column; `--columns pod=40` shows it whole; `--columns pod=0` hides the column and the message moves left.
7. `:save dev-api` writes the template with the targets and filter as they are at that moment, preserving the other templates and the comments in the file; `gossip follow --template dev-api` in a new terminal reproduces the session.
8. Quit with `Ctrl-C`, then `gossip follow --last` resumes the same targets and filter.
9. `:load staging-all` switches the running session to that template; the old pods' streams close with a notice each.
10. A panic inside the CLI, or `kill -TERM`, leaves the terminal in a usable state (raw mode off, cursor visible).
11. `cargo test`, `cargo clippy --workspace -- -D warnings` and `cargo fmt --check` are clean. Tests cover: the command parser (every command, bad arguments), filter composition from flags and commands, the pause buffer with its cap, column cutting at each side with ASCII and with emoji, the config writer preserving other content, and `SessionHandle` against the stand-in API server (add, drop, load, set_events).

## 8. Notes for the implementer

- Terminal handling: `crossterm` for raw mode, key events, the clear sequence and the cursor; no `ratatui`, no alternate screen. Read keys in a blocking thread and send them over a channel to the async output task, or use crossterm's `EventStream`.
- Output and the status line share stdout: the renderer owns it, and everything goes through one writer task, so a line and a status redraw never interleave. The status line is written, and erased before the next record, with `\r` and clear-to-end-of-line, not with cursor positioning.
- Width measurement: `unicode-width` for cells; keep the one width table the emoji fallback already uses.
- The config writer: `toml_edit`, not `toml`, so comments and ordering survive a `:save`.
- The pause buffer is a `VecDeque<Record>` in the renderer, not in the core: the core's stream is still consumed while paused, so backpressure never reaches the log streams.
- `SessionHandle` is an `mpsc` sender of commands into the session task plus a `watch` receiver for the current `SessionSpec`; the task applies commands between stream polls. `remove_target` must not close a pod another target still matches: keep a reference count per (pod, container).
- `--last` is written on quit from the renderer's current spec, including rendering options; if writing fails (read-only config), say so once and quit anyway.
- Raw-mode restore goes in a guard dropped on every exit path, and in a panic hook, so acceptance 10 holds without special cases.

## 9. Record and replay

The cluster keeps little history: a container's log file on the node until it rotates or the pod goes, events for an hour. `--since` and `--tail` reach back that far and no further. For anything longer, Gossip records what it streamed and plays it back.

- `gossip follow ... --record <file>` writes every record that passes the resolver, before the filter, as one JSON object per line, the format of `--output json` (SPEC 5.3), appended to the file. The filter still applies to the screen, so the recording holds more than was shown.
- `:record <file>` starts recording mid-session; `:record off` stops it; `:record` alone says whether one is running and how many records it holds.
- `gossip replay <file> [flags]` reads a recording and plays it through the same renderer: the filter flags of `follow`, `--fields`, `--columns`, and the interactive mode all work on it, including `:level`, `:grep` and `:field`. Targets, events and `:add` / `:drop` are refused in replay with a notice: the source is the file.
- Replay speed: `--speed 1` plays at the recorded pace, `--speed 0` (the default) as fast as the terminal takes it, `--speed 10` ten times faster. `Space` pauses as in section 4. `--from <time>` and `--to <time>` cut the recording by the records' `time`.
- A recording is the record's full JSON, so the cluster, namespace, pod, container, fields and raw line are all there; the replayed lines render exactly as they did live.
- Not in scope: compression, rotation of the recording, recording to anything but a local file. A recording left running fills the disk at the rate of the stream; the status line shows the size while it runs.

Acceptance for this section: `--record` on a one-minute follow writes a file in which every line parses with `jq` and carries every field of the record; `gossip replay` of that file with `--level warn` shows the same lines a live `--level warn` would have, in order; `--speed 1` takes about a minute, `--speed 0` under a second.
