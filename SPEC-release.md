# Gossip — Release build and README

Status: draft, 2026-11-18. Nothing of this is implemented yet. It adds to `SPEC.md` and `SPEC-interactive.md` and changes no behaviour of the tool.

A tag builds Gossip for four platforms and publishes the archives as a GitHub release. The README becomes the front page for someone who has never seen Gossip, is brought up to date with every change to what it describes, and is checked against the code instead of trusted. The model for both is the Rick repository (`Open-Breach/rick`: `.github/workflows/release.yml` and `specs/readme.md`), cut down to what a single command-line binary needs.

## 1. Scope

In scope:

- A release workflow: archives for Linux x64 and arm64, macOS arm64 and Windows x64, checksums, a smoke test, a GitHub release.
- The version of the binary taken from the tag.
- The README: its structure, which parts are fixed and which are regenerated, and the checks that keep it true.

Out of scope:

- deb, rpm, AUR, Homebrew, winget, an install script. `Open-Breach/trebuchet` takes those over later from the same tag and the same archives. The workflow carries one comment saying so and nothing else for it.
- Signing and notarisation on macOS and Windows.
- A changelog file. The release page uses GitHub's generated notes.

## 2. The release workflow

`.github/workflows/release.yml`.

### 2.1 Triggers

- A pushed tag `v*` (`v0.1.0`, `v0.2.0-rc1`): builds and publishes the release. A tag with a pre-release suffix is published as a pre-release.
- `workflow_dispatch` with a `tag` input: the same build, but the release is created as a draft, for trying the workflow without publishing.
- Serge pushes the tag. The agent never tags.

### 2.2 Targets

| Platform | Rust target | Archive |
| --- | --- | --- |
| `linux-x64` | `x86_64-unknown-linux-musl` | `gossip-<version>-linux-x64.tar.gz` |
| `linux-arm64` | `aarch64-unknown-linux-musl` | `gossip-<version>-linux-arm64.tar.gz` |
| `macos-arm64` | `aarch64-apple-darwin` | `gossip-<version>-macos-arm64.tar.gz` |
| `windows-x64` | `x86_64-pc-windows-msvc` | `gossip-<version>-windows-x64.zip` |

- `<version>` is the tag without the `v`. The platform names are Rick's, so one set of names serves every Open-Breach project.
- The Linux binaries are static (musl), so one archive runs on every distribution with no glibc floor. Each is built natively on a runner of its own architecture, not cross-compiled.
- No Intel macOS build and no arm64 Windows build.
- Runners come from the repository variables `RUNNER_LINUX_X64`, `RUNNER_LINUX_ARM64`, `RUNNER_MACOS` and `RUNNER_WINDOWS`; where one is not set, the job runs on GitHub's runner of that platform (`ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-latest`, `windows-latest`). Same scheme as Rick.
- Release profile: optimised, symbols stripped, `lto = "thin"`. The toolchain is the one `rust-toolchain.toml` selects.

### 2.3 What an archive holds

One top-level folder named like the archive without its extension:

```
gossip-0.1.0-linux-x64/
  gossip                      (gossip.exe on Windows)
  README.md
  LICENSE
  THIRD-PARTY-LICENSES.html
```

- `README.md` and `LICENSE` are the repository's, at the tagged commit.
- `THIRD-PARTY-LICENSES.html` holds the licence texts of every crate compiled into the binary, generated in the workflow by `cargo-about` from `about.toml`. The workflow fails when a crate has a licence `about.toml` does not accept.
- The binary keeps its execute bit in the tarballs.

### 2.4 Checksums

- Beside each archive during the build, a `.sha256` file. The release carries them merged into one `SHA256SUMS.txt`, sorted by file name, in the format `sha256sum -c` reads.

### 2.5 Smoke test

A second job per platform, on a clean runner of that platform, downloads the archive the build job made and:

1. verifies it against its checksum;
2. unpacks it and checks the four files are there;
3. runs `gossip --version` and compares the output with the tag's version;
4. runs `gossip --help`, `gossip follow --help` and `gossip list --help` and expects exit code 0;
5. on Linux, checks the binary is static (`ldd` reports no dynamic dependencies).

No cluster is involved. The release job runs only when every smoke test passed.

### 2.6 The release

- Title: the tag. Body: GitHub's generated release notes since the previous tag.
- Assets: the four archives and `SHA256SUMS.txt`.
- The first job of the workflow, before anything is built, runs the README check of section 4.5 against the tag, so a release cut from a commit whose README names another version stops at once.

## 3. The version

- The tag is the version. No file is edited to make a release build report the right number.
- The workflow passes the version to the build in the environment variable `GOSSIP_VERSION`; `crates/gossip/build.rs` hands it to the compiler, and `gossip --version` prints `gossip 0.1.0`.
- A build without `GOSSIP_VERSION` (a checkout, `cargo install --path`) prints the version of `Cargo.toml` followed by `-dev` and the short commit when the build runs inside a git checkout: `gossip 0.1.0-dev (9b3fafc)`.
- `Cargo.toml` carries the version of the release being worked towards. It is set in the release pull request of section 4.4; the workflow fails when the tag and `Cargo.toml` disagree.

## 4. The README

### 4.1 Purpose and tone

`README.md` is written for someone who runs Kubernetes and has never heard of Gossip. In a minute they know what it is and how to install it; in ten they have followed the walkthrough and know how to work with it; after that it is the reference they come back to.

- The README follows `main`: it describes what the code on `main` does.
- Terse. Sentences that say what Gossip does, not what it aims to do. No sentence that says nothing.
- No issue numbers, pull request numbers or commit hashes.
- Nothing is described that the code does not do. Lists of flags, keys and commands come from the code.
- Every name in an example (cluster, namespace, workload, tenant) is made up and the same throughout the README: one namespace `shop` with the deployments `api` and `web`. No name from a real cluster.

### 4.2 Structure

In this order. Parts marked *fixed* are verbatim and are not reworded without Serge; parts marked *regenerated* are written by `cargo xtask readme` between `<!-- generated:<name> -->` and `<!-- /generated:<name> -->` markers, in every pull request that changes what they show.

**Title block** (fixed, version regenerated)

`# Gossip`, then the tagline on its own line, in italics:

> Everything your cluster says behind your back.

Then the paragraph of what it is, verbatim:

> Gossip is a terminal tool that follows the logs of Kubernetes workloads. It keeps following them across pod restarts and rollouts, merges the cluster's own events into the same stream, reads JSON and logfmt lines as data, and can be changed while it runs: filter, pause, add a service, save the session.

Then the status line, one line, the version followed by `·` and the status text:

> v0.1.0 · Gossip is young and has been tried by hand against one cluster so far, so expect bugs. Please tell us what you see.

The version follows the release. The status text stays word for word between releases. At every release the agent reads it against what the release ships; when a sentence looks no longer true it proposes new wording under **Needs a decision** in the pull request and keeps the old text until Serge answers.

Then one sample of the output, four or five lines in a code block, showing a line, a pod being killed, the new pod starting.

**1. Warning** (fixed)

> **Use with caution.** Gossip is written largely by an AI coding agent working from [SPEC.md](SPEC.md), with human review, a strict test gate and CI. That makes it fast to build and easy to audit, but it is still young software. Gossip only reads from your cluster and never creates, changes or deletes anything in it, but it runs with the credentials of your kubeconfig. Until 1.0, treat every release as a preview. If it does something surprising, that's a bug: please report it.

**2. Install** (regenerated)

Heading `## Install`. Three short blocks, **Linux**, **macOS**, **Windows**. Each has the direct link to the release's archive for every architecture built for that platform (`https://github.com/Open-Breach/gossip/releases/download/v<version>/gossip-<version>-<platform>.<ext>`) and the real steps to get `gossip` onto the `PATH`, as they are in that release: two or three lines, tried in the smoke test's environment, not assumed. Whatever an unsigned binary needs on macOS and Windows to start is said in one line each.

Then one block **From source**: `cargo install --path crates/gossip` from a clone, and the toolchain it needs.

When Trebuchet ships packages, the steps change with it.

**3. Using Gossip** (walkthrough fixed in shape, output regenerated)

Heading `## Using Gossip`. One session read top to bottom. Each step is a sentence or two, the command, and a few lines of what it prints, produced by the built binary against the stand-in API server of the tests, never typed by hand. The steps, in this order:

1. Find your way: `gossip list contexts`, `gossip list namespaces`, `gossip list shop`.
2. Follow one deployment: `gossip follow shop/api`.
3. What a restart looks like: a pod is killed while following; the events with their emoji, the notices, and the first lines of the new pod, with no key pressed.
4. Narrow it: `:level info`, then `:filter` to see what is set.
5. Stop to read: `Space`, the status line counting, `Space` again.
6. Add a second service: `:add shop/web`, then `:targets`.
7. Keep it: `:save shop`.
8. Come back: quit, then `gossip follow --last`, and `gossip follow --template shop`.
9. Keep a record: `--record`, then `gossip replay`.

**4. Reference** (regenerated)

Heading `## Reference`. Complete, in tables, nothing left to `--help`. One subsection each:

- Commands: every `gossip` command and subcommand.
- Targets: the path syntax, globs, the `kind:name` prefix, a pod given directly.
- `follow` flags: every flag, its meaning and its default. The same for `replay` and `list`.
- Keys of the interactive mode.
- `:` commands, every one.
- Filters: the fields, the operators, `all` / `any` / `not`, how flags and commands combine, what a record without a level does, that events pass.
- Fields: the default order, the hidden names, `--fields`.
- Columns: the columns, default widths and cut sides, `[columns]`, `--columns`.
- Config file: its path, every key of `[defaults]` and of a template, one complete example with a template.
- Line formats: JSON, logfmt, text, continuation and panic blocks, the `format` option.
- Events: the reasons and their emoji, `--no-emoji`, `--no-events`.

This section is exempt from any length rule: it is never shortened.

**5. Known limits** (fixed list, checked at each release)

Heading `## Known limits`. These, in this order:

- One cluster at a time.
- One connection per followed container, so a few hundred at once can reach the open-file limit (256 by default on macOS); how to raise it.
- The kubelet polls a container's log file, which adds a second or two that no client can remove.
- History reaches as far back as the node keeps the log files, and events for one hour. Record and replay are for anything longer.
- In a plain-text service, lines printed in the two seconds after a panic join the panic's record.
- Tried on Linux only so far; the macOS, Windows and arm64 Linux builds pass their smoke test and nothing more.

At each release the agent checks each item against the code. One that no longer holds is proposed for removal under **Needs a decision**; a new limit it knows of is proposed the same way. It never drops or adds one on its own.

**Closing lines** (fixed)

- One line of links: the three specs, building and development (`docs/development.md`).
- The licence line: Apache-2.0 with a link to `LICENSE`.

### 4.3 What leaves the README

The current *Status* section (the crate table, how the tests run), *Development* and the long *Build and install* text are not in the structure above. What is still true moves to `docs/development.md`; nothing is deleted from the repository.

### 4.4 Rules for the release

- A release is prepared in one **release pull request**: it sets the version in `Cargo.toml`, regenerates the README, and nothing else. Serge merges it and pushes the tag on the merge commit.
- The README follows `main`. A pull request that adds or changes a command, a flag, a key or a `:` command, or that changes what the walkthrough prints, regenerates the README in that same pull request; the test of 4.5 fails until it has.
- Regenerated: the version in the status line, the Install section, the output shown in the walkthrough, the Reference.
- Left alone: the tagline, the paragraph of what it is, the status text, the warning, the shape and order of the walkthrough, the known limits, the closing lines. A change to any of them is listed in the pull request under **Decisions made**.
- Proposed, not decided: new wording for the status text, and changes to the known limits, under **Needs a decision**.
- When the README cannot be made truthful (a step of the walkthrough does not work as written, an archive is missing), the agent stops and says so under **Needs a decision**. It does not adjust the text to fit.

### 4.5 Checked, not trusted

- A generator, `cargo xtask readme`, rewrites the regenerated parts: it reads the flags and commands from the `clap` definitions and the table of `:` commands and keys from the CLI crate, and it produces the walkthrough's output by running the built binary against the stand-in API server.
- A test (`cargo test`) fails when: a `gossip` command, a flag of `follow`, `replay` or `list`, a key or a `:` command exists in the code and not in the Reference, or the other way round; a default in the Reference differs from the code's; the fixed texts of 4.2 are not word for word in the README; a generated block differs from what the generator would write now.
- `.github/scripts/check-readme.sh <tag>` fails when the version in the status line or in the Install links is not the tag's. The release workflow runs it first.

## 5. Acceptance

1. `workflow_dispatch` with tag `v0.1.0-rc1` on `main` produces a draft release with four archives and `SHA256SUMS.txt`; every smoke test passes.
2. Each archive unpacks to one folder holding the binary, `README.md`, `LICENSE` and `THIRD-PARTY-LICENSES.html`; `sha256sum -c SHA256SUMS.txt` passes on the downloaded assets.
3. The Linux x64 binary from the archive starts on Debian 12, Ubuntu 24.04, Fedora 41 and Alpine without installing anything.
4. `gossip --version` from an archive prints the tag's version; from `cargo install --path crates/gossip` it prints the `-dev` form.
5. Pushing a tag whose version differs from the README's status line or from `Cargo.toml` fails in the first job, before any build.
6. The README has the structure of 4.2, in that order, with the fixed texts word for word.
7. Removing a row from the Reference's flag table, or adding a flag in the code without regenerating, fails `cargo test`.
8. `cargo xtask readme` run twice on the same commit changes nothing the second time.
9. `cargo fmt --check`, `cargo clippy --workspace -- -D warnings` and `cargo test --workspace` are clean.

## 6. Notes for the implementer

- Read Rick's `release.yml` for the job layout (build matrix, smoke-test matrix, release job, the `TAG` variable shared by both triggers) and its `specs/readme.md` for the README rules. Take the structure, not the content: Gossip has no plugins, no GStreamer, no app bundle.
- musl: `kube` is already built with `rustls-tls` and `ring`, so there is no OpenSSL to link. On the Linux runners install `musl-tools` and add the musl target; build natively on each architecture.
- Windows: pack with `7z a -tzip`; macOS and Linux with `tar czf`. Checksums with `sha256sum` or `shasum -a 256`, whichever the runner has.
- `softprops/action-gh-release` or `gh release create`; `permissions: contents: write` on the release job only.
- The walkthrough's pod kill needs the stand-in server to script a pod going away and a new one starting; it already does this for the tests of SPEC acceptance 3.
- Keep the generator's output stable: fixed clock, fixed pod names, no colour, a fixed terminal width.
