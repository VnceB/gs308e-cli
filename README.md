# gs308e

A command-line client for the **NETGEAR GS308Ev4** eight-port switch. Read port status and traffic counters, manage VLANs and change switch settings from a terminal or a script, with no browser needed.

```console
$ gs308e ports --summary
$ gs308e --json counters --port 3
$ gs308e vlan create 20 --name Studio --untagged 2,4          # preview
$ gs308e vlan create 20 --name Studio --untagged 2,4 --apply  # submit
```

> [!WARNING]
> **Early release (0.x).** All read commands are tested on a real GS308Ev4 running firmware V1.0.1.13. Most configuration changes have **only been tested against simulated switch responses**. So far, only the LED and power-saving toggles have been confirmed on hardware. Try changes on a switch you can reach physically, and see the [feature status table](docs/status.md) before relying on a command.

## Features

- **Inspect**: model, firmware, ports, link speed, 64-bit traffic counters, VLAN tables and stored cable-test results.
- **Configure**: VLANs (basic and advanced 802.1Q, voice VLAN), port settings, QoS, LAG, mirroring, IGMP, address, time/SNTP and more.
- **Safe by default**: every change is a preview until you add `--apply`. Every applied change is checked with a fresh read.
- **Scriptable**: versioned JSON output, stable exit codes, password from stdin and multi-command sessions under one login.
- **No vendor software**: a single binary that talks to the switch's normal web interface.

## Requirements

- A NETGEAR **GS308Ev4** running firmware **V1.0.1.13 or newer**. Run `gs308e firmware` to check yours. Older firmware is rejected with exit code `5` and the detected version.
- macOS on Apple Silicon or Linux (x86-64 / ARM64, glibc with OpenSSL 3) for the prebuilt binaries. Other platforms can build from source.

## Install

### Prebuilt binary

Download the archive for your platform from [GitHub Releases](../../releases), check it against `SHA256SUMS.txt` and extract it:

```sh
tar -xzf gs308e-aarch64-apple-darwin.tar.gz
./gs308e --help
```

On macOS the binary is not signed or notarized. If Gatekeeper blocks it, remove the download quarantine flag:

```sh
xattr -d com.apple.quarantine ./gs308e
```

### From source

Requires Rust 1.88 or newer.

```sh
cargo install --locked --path .
```

## Quick start

```sh
export GS308E_HOST=192.0.2.1     # your switch's management address
gs308e info                      # prompts for the switch password
gs308e ports
gs308e vlan list
gs308e snapshot                  # device, ports, counters and VLAN mode at once
```

A bare host uses `http://`. Pass `https://…` and optionally `--cacert ca.pem` for an HTTPS endpoint.

## Commands

| Command | What it does |
| --- | --- |
| `info` | Model, firmware, name, MAC and serial number |
| `firmware` | Detected firmware version (no device identifiers) |
| `ports [--port N] [--summary]` | Link state, speed, duplex and label |
| `counters [--port N]` / `counters clear --all` | Received/transmitted bytes and CRC errors, or clear them |
| `vlan`, `vlan list` | VLAN mode, VLANs, membership, PVIDs, management and voice VLAN |
| `vlan create/rename/delete/members/assign/pvid/management/voice/oui-*` | VLAN changes |
| `settings show FEATURE` / `settings set FEATURE KEY=VALUE…` | Inspect or change port, identity, LED, QoS, LAG, mirror, multicast, address and other settings |
| `time …` | Clock, SNTP servers and daylight saving |
| `maintenance …` | Cable test and results, reboot, configuration backup, firmware upgrade |
| `snapshot` | Device, ports, counters and VLAN mode in one login |
| `diagnostics` | Health check of the main read paths, plus logout verification |
| `session` / `interactive` | Run several commands under one login ([docs](docs/sessions.md)) |
| `completions SHELL` | Shell completions for Bash, Zsh, Fish, Elvish or PowerShell |

Every command has detailed `--help`, for example `gs308e vlan --help` or `gs308e settings show port --help`.

## Making changes safely

```sh
gs308e settings show port --port 4                 # see available keys and values
gs308e settings set port --port 4 'name=Studio'    # preview
gs308e settings set port --port 4 'name=Studio' --apply
```

- **Preview first.** Without `--apply`, nothing is sent. With `--apply`, the CLI keeps the form's other fields, rechecks dependencies, submits, then verifies the result with a separate read. A change that's already in place reports `unchanged`.
- **Protect your uplink.** `--protect-port 1` (or `GS308E_PROTECT_PORTS=1,8`) blocks any change that targets those ports, as well as switch-wide disruptive operations.
- **Risky operations need an extra flag.** VLAN mode changes need `--from MODE --accept-vlan-reset`, management VLAN changes need `--accept-reconnect`, and reboot or firmware upgrade need `--accept-outage`.
- **No automatic retries.** If a connection drops mid-change, check the switch before trying again. The CLI never re-authenticates, replays a write or rolls back.

Firmware newer than V1.0.1.13 can still be read. Writes there are limited to forms that match the reviewed layout (currently the switch identity form).

## Scripting

```sh
gs308e --password-stdin --json counters --port 3 < /run/secrets/switch
```

```json
{"schema_version":1,"command":"counters","data":[{"port":3,"rx_bytes":"4294967298","tx_bytes":"8192","crc_errors":"0"}]}
```

Reports go to stdout and errors to stderr. Exit codes are stable (`0` ok, `2` usage, `3` auth, `4` network/verification, `5` unsupported device, `6` incomplete diagnostics). See [Scripting and JSON](docs/scripting.md) for the full contract and [Sessions](docs/sessions.md) for batch files.

## Security notes

- The GS308Ev4 management interface normally uses **plain HTTP**. The login exchange and session cookie travel unencrypted, so use the CLI on a trusted management network. HTTPS and `--cacert` are supported but have not been tested on hardware.
- The password comes from a hidden prompt or `--password-stdin`, is never accepted as an argument and is wiped from memory after sign-in.
- The session is bound to one origin. The CLI follows no redirects, ignores proxy environment variables and never runs page scripts.
- `maintenance backup --stdout` exports a configuration that may contain sensitive data. Store it accordingly.

Found a vulnerability? See [SECURITY.md](SECURITY.md).

## Documentation

- [Feature and hardware verification status](docs/status.md)
- [Sessions and batch files](docs/sessions.md)
- [Scripting, JSON and exit codes](docs/scripting.md)
- [Sign-in troubleshooting (`--explain-login`)](docs/login-diagnostics.md)
- [Web UI inventory](docs/ui-inventory.md)
- [Changelog](CHANGELOG.md)

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Or run the same checks in Docker, as CI does: `docker build --target check .`

Tests run the real binary against a local HTTP server that serves hand-built HTML fixtures from `tests/fixtures`, so no switch is needed. `src/session.rs` handles the HTTP session, `src/model.rs` parses the switch's pages, and each feature area has its own module (`vlan.rs`, `settings.rs`, `time.rs`, `maintenance.rs`, …).

The project uses [Spec Kit](https://github.com/github/spec-kit). `.specify/` holds the project principles and `specs/` holds the feature specification.

Pushing a tag that matches the `Cargo.toml` version (for example `v0.1.0`) builds and publishes release archives through [the release workflow](.github/workflows/release.yml).

### Contributing

Issues and pull requests are welcome. Reports from real hardware are especially useful. Please:

- update [docs/status.md](docs/status.md) when behavior or hardware verification changes;
- use reserved example addresses (`192.0.2.x`, `.example`) and hand-written fixtures;
- never include passwords, session cookies, serial numbers, MAC addresses or raw captures from a real switch.

## License

[Apache-2.0](LICENSE).

This is an independent project. It is not affiliated with, endorsed by or supported by NETGEAR, Inc. "NETGEAR" and "GS308E" are trademarks of NETGEAR, Inc. and are used here only to identify compatible hardware. Use at your own risk: changing switch configuration can disconnect your network.
