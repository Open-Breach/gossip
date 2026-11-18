# Gossip

Gossip is a Rust tool for following Kubernetes pod logs and cluster events
across restarts and rollouts. It is built as a core library plus a command
line, step by step from [SPEC.md](SPEC.md).

This README describes what exists in the repository today, not what the spec
plans.

## Status

One command works against a cluster so far: `gossip list`. Following logs is
not built yet.

| Crate | Kind | Holds today |
| --- | --- | --- |
| `crates/gossip-core` | library | cluster access and discovery; and, not yet used by the command line, targets, records, the line parser, filters and the config file |
| `crates/gossip` | binary `gossip` | `gossip list` |

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
