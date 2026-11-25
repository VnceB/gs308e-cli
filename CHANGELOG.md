# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/).

## 0.1.0 - 2026-11-24

Initial public release.

- Read commands: `info`, `firmware`, `ports`, `counters`, `vlan`, `snapshot`, `diagnostics`.
- Configuration commands for VLANs, port and switch settings, time/SNTP and maintenance. Changes preview by default and require `--apply`.
- Multi-command `session` / `interactive` mode with batch files and newline-delimited JSON.
- Versioned JSON output (`schema_version: 1`) and stable exit codes.
- Requires GS308Ev4 firmware V1.0.1.13 or newer.
- Prebuilt binaries for Apple Silicon macOS, x86-64 Linux and ARM64 Linux.
