# Building and development

What the [README](../README.md) leaves out because a user of Gossip does not need it: how the repository is laid out, how to build and test it, how the README is kept true, and how a release is cut. The specs say what Gossip does: [SPEC.md](../SPEC.md), [SPEC-interactive.md](../SPEC-interactive.md) and [SPEC-release.md](../SPEC-release.md).

## The workspace

One Cargo workspace with three crates:

| Crate | Kind | Holds |
| --- | --- | --- |
| `crates/gossip-core` | library | cluster access, discovery, target resolution, log streams with reconnection, cluster events, the session that merges them and can be changed while it runs, the line parser, filters, the config file and its writer |
| `crates/gossip` | library and the binary `gossip` | the commands `list`, `follow`, `replay` and `templates`, the rendering, the interactive mode, recording |
| `crates/xtask` | `cargo xtask` | the generator of the README and its tests |

The command line only presents what the core produces; nothing in it talks to the cluster, and the core prints nothing and knows nothing of keys or of a terminal. The binary is a `main` of three lines over the library of `crates/gossip`, so that `xtask` can read the commands, the flags, the keys and the defaults from the code that implements them (`crates/gossip/src/reference.rs`).

Gossip is a client only: it reads from the cluster and never creates, changes or deletes anything in it. It reaches the cluster through the kubeconfig (`KUBECONFIG` or `~/.kube/config`) with the same credentials `kubectl` uses, exec plugins such as `kubelogin` included, and it honours `HTTPS_PROXY` and a `proxy-url` in the kubeconfig.

## Build

Gossip needs the latest stable Rust toolchain (edition 2024); `rust-toolchain.toml` selects it when you use `rustup`.

Install the `gossip` binary into `~/.cargo/bin`:

```bash
cargo install --path crates/gossip
```

Or build and run from a checkout:

```bash
cargo run -p gossip -- --help
```

A build from a checkout reports the version of `Cargo.toml` as a development build, with the commit it was built from: `gossip 0.1.0-dev (9b3fafc)`. A release build is handed its version in `GOSSIP_VERSION` and reports that (`crates/gossip/build.rs`).

## Tests

The tests need no cluster and no terminal. They run the whole of it against a stand-in API server inside the test code (`gossip_core::testing`), and the interactive mode against a terminal in memory. The behaviour against a real cluster and in a real terminal is checked by hand; there are no tests against a cluster in CI.

The files in `crates/gossip/tests/pinned/` hold the output of one scenario without a terminal, byte for byte, as text, as JSON and as raw lines; a test compares them, so that nothing reaches a pipe or a file that was not there before.

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

Two tables in the CLI crate are the only place to change what they hold, and the README repeats them from there: `KNOWN_FIELDS` in `crates/gossip/src/fields.rs`, the fields the output shows first and the ones it hides, and `EVENT_EMOJI` in `crates/gossip/src/render.rs`.

## The README

`README.md` follows `main`: it describes what the code on `main` does, and it is checked against the code instead of trusted (SPEC-release.md, section 4). It has two kinds of text:

- The fixed parts are written by hand: the tagline, the paragraph of what Gossip is, the status text, the warning, the sentences of the walkthrough, the known limits, the closing lines. They are not reworded without Serge.
- The generated parts stand between `<!-- generated:name -->` and `<!-- /generated:name -->`: the status line with the version, the sample of the output, the Install section, what every step of the walkthrough prints, and the Reference.

Write the generated parts:

```bash
cargo xtask readme
```

Say what in the README is not what the code says, and change nothing:

```bash
cargo xtask readme --check
```

Where the generated parts come from:

- The commands and the flags are the ones `clap` reads, with the help and the default each carries there. The keys and the `:` commands are the tables `KEYS` and `COMMANDS` of `crates/gossip/src/reference.rs`, which tests hold against the keyboard and the parser.
- The prose of the Reference is `crates/xtask/src/readme/reference.md`, and the Install section is `install.md` beside it; `{{name}}` marks where a table or the version goes.
- The walkthrough and the sample are what the built `gossip` binary prints. The generator starts the stand-in API server with a made-up cluster (the namespace `shop` with the deployments `api` and `web`), runs the binary on a pseudo-terminal of a fixed size, types the keys, and takes what the terminal shows (`crates/xtask/src/readme/walkthrough.rs`). It needs Linux or macOS, and it keeps its files in `/tmp/gossip-readme`, which is why that path is in the README.

For the output to be the same on every run, the binary reads the time from a file when the environment variable `GOSSIP_CLOCK` names one (`crates/gossip-core/src/clock.rs`). The generator moves that clock as the walkthrough goes. Nothing else uses the variable.

`cargo test` fails when a command, a flag of `follow`, `replay` or `list`, a key or a `:` command is in the code and not in the Reference, or the other way round; when a default in the Reference differs from the code's; when a fixed text is not in the README word for word; and when a generated block differs from what the generator writes now (`crates/xtask/tests/readme.rs`). So a pull request that adds or changes a command, a flag, a key or a `:` command, or that changes what the walkthrough prints, runs `cargo xtask readme` and commits the result in that same pull request; `cargo test` fails until it has.

## Releases

A release is a tag `vX.Y.Z` on `main` (SPEC-release.md, sections 2 and 3). Serge pushes the tag.

1. The release pull request sets the version in `Cargo.toml`, runs `cargo xtask readme`, which writes that version into the status line and the Install links, and holds nothing else. For 0.1.0 the version was 0.1.0 from the start, so the pull request that brought the README was the release pull request.
2. Serge merges it and pushes the tag on the merge commit.
3. `.github/workflows/release.yml` checks the tag against `Cargo.toml` and the README, collects the licences of the crates with `cargo-about` (`about.toml`, `about.hbs`), builds the binary for Linux x64 and arm64 (static, musl), macOS arm64 and Windows x64, packs an archive for each, tests every archive on a clean runner of its platform, and creates the GitHub release with the archives and `SHA256SUMS.txt`.

To try the workflow without publishing, run it by hand from the Actions page with a tag name such as `v0.1.0-rc1`: it builds and tests the same way and ends in a draft release, which is deleted afterwards. The tag itself is only created if a draft is published.

A new crate whose licence is not in `about.toml` stops the release in the licences job; the licence is added there once someone has looked at it.
