# Gossip

Gossip is a Rust tool for following Kubernetes pod logs and cluster events
across restarts and rollouts. It is built as a core library plus a command
line, step by step from [SPEC.md](SPEC.md).

This README describes what exists in the repository today, not what the spec
plans.

## Status

The command line is still a skeleton: the `gossip` binary only prints its help
and its version, and nothing talks to a cluster yet.

The core library holds the logic that needs no cluster, with unit tests:

| Module | What it does |
| --- | --- |
| `target` | Parses targets, the paths `namespace[/workload[/pod[/container]]]` whose names may be globs, such as `flowcpt/flowcpt-api`, `flowcpt/cronjob:nightly-sync` or `monitoring/prometheus/*/exporter`. |
| `record` | The record: one type for a log line, a cluster event or a notice, with where it came from, when, a level, a message and structured fields. |
| `parser` | Turns the lines of a container into records. JSON and logfmt lines become a level, a message and fields; anything else is text with a guessed level. Indented lines and lines starting with `at ` or `Caused by:` join the line before them. |
| `filter` | Rules `field operator value` over records, joined into a tree with `all`, `any` and `not`. |
| `config` | Reads and writes the config file with its `[defaults]` and templates, and layers a template over the defaults. Unknown keys are an error that names the line. |
| `duration` | Durations such as `30s`, `5m` and `1h30m`. |

The command line does not use any of this yet.

| Crate | Kind | Holds today |
| --- | --- | --- |
| `crates/gossip-core` | library | the modules above |
| `crates/gossip` | binary `gossip` | `--help` and `--version` |

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
