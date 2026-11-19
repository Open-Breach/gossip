**Linux**

- x64: [gossip-{{version}}-linux-x64.tar.gz]({{download}}/gossip-{{version}}-linux-x64.tar.gz)
- arm64: [gossip-{{version}}-linux-arm64.tar.gz]({{download}}/gossip-{{version}}-linux-arm64.tar.gz)

The binary is static and runs on any distribution. Unpack the archive and put `gossip` on the `PATH`; on arm64, take the other archive:

```bash
curl -LO {{download}}/gossip-{{version}}-linux-x64.tar.gz
tar xzf gossip-{{version}}-linux-x64.tar.gz
sudo install gossip-{{version}}-linux-x64/gossip /usr/local/bin/
```

**macOS**

- Apple silicon: [gossip-{{version}}-macos-arm64.tar.gz]({{download}}/gossip-{{version}}-macos-arm64.tar.gz)

```bash
curl -LO {{download}}/gossip-{{version}}-macos-arm64.tar.gz
tar xzf gossip-{{version}}-macos-arm64.tar.gz
sudo install gossip-{{version}}-macos-arm64/gossip /usr/local/bin/
```

The binary is not signed or notarised: if macOS refuses to start it, `xattr -d com.apple.quarantine /usr/local/bin/gossip` lifts the quarantine a download carries.

**Windows**

- x64: [gossip-{{version}}-windows-x64.zip]({{download}}/gossip-{{version}}-windows-x64.zip)

In PowerShell, unpack the archive and put its folder on the `PATH` of the session; to keep it, move `gossip.exe` to a folder that is on your `PATH`:

```powershell
Expand-Archive gossip-{{version}}-windows-x64.zip -DestinationPath .
$env:Path += ";$PWD\gossip-{{version}}-windows-x64"
```

The binary is not signed: if Windows refuses to start it, `Unblock-File gossip-{{version}}-windows-x64\gossip.exe` lifts the mark a download carries.

**From source**

With the latest stable Rust toolchain, which `rust-toolchain.toml` selects when you use `rustup`:

```bash
git clone {{repository}}
cargo install --path gossip/crates/gossip
```

Every archive holds the binary, this README, the licence and the licences of the crates it is built from. `SHA256SUMS.txt` on the [release page]({{repository}}/releases/tag/v{{version}}) has the checksums.
