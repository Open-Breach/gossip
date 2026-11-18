# Gossip

Gossip is a Rust tool for following Kubernetes pod logs and cluster events
across restarts and rollouts. It is built as a core library plus a command
line, step by step from [SPEC.md](SPEC.md).

This README describes what exists in the repository today, not what the spec
plans.

## Status

The project is a skeleton: the Cargo workspace, the two crates and the
continuous integration are in place. The `gossip` binary only prints its help
and its version so far; it does not talk to a cluster yet.

| Crate | Kind | Holds today |
| --- | --- | --- |
| `crates/gossip-core` | library | nothing yet |
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
