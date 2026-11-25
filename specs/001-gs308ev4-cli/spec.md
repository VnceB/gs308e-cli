# Specification · GS308E-CLI

**Scope** · Open-source CLI (`gs308e`) for the NETGEAR GS308Ev4 eight-port switch running firmware `V1.0.1.13` or newer. See [docs/status.md](../../docs/status.md) for feature and verification status.

## Functional Contracts

### Inspection
- `info` reports model, firmware, system name, MAC, and serial. Verify model and firmware (`>= V1.0.1.13`) before emitting any report; exit `5` on unsupported devices.
- `firmware` reports the detected firmware version without device identifiers.
- `ports` lists all eight ports (link state, negotiated Mbps, duplex, label) or a filtered `--port 1-8`, with `--summary` counts.
- `counters` reconstructs 64-bit RX bytes, TX bytes, and CRC error counters for all eight ports or `--port 1-8`.
- `vlan` and `vlan list` inspect the active VLAN mode, tables, membership, PVIDs, management VLAN, and voice OUI table.
- `snapshot` gathers device, ports, counters, and VLAN mode in a single session.
- `diagnostics` runs independent read checks for ports, VLAN mode, VLAN table, and stored cable-test results, followed by logout verification (exiting `6` if any check or logout verification fails).

### Scripting & Sessions
- `--host` / `GS308E_HOST` accepts a single management host or origin (defaulting to `http://` when no scheme is provided). HTTPS validates certificates and supports `--cacert <FILE>`. Follow no redirects, ignore proxy env vars, and never retry authentication automatically.
- Password input comes from a hidden terminal prompt or `--password-stdin`.
- `--json` emits `schema_version: 1` JSON (or newline-delimited JSON in `session`, ending with `session-end`). Errors go to stderr.
- Exit codes: `0` success, `1` output error, `2` usage/validation error, `3` auth/session error, `4` transport/HTTP/verification error, `5` unsupported model/firmware or unrecognized response, `6` incomplete diagnostics, `130` interrupted session.
- `session` (`interactive`) supports `-c COMMAND`, `-f FILE`, or stdin commands under a single login with syntax pre-validation, 2-second pacing, `--read-only`, and verified logout.
- `--help` and `completions <SHELL>` work offline without a host or credentials.

### Configuration & Maintenance
- Configuration commands (`vlan`, `settings set`, `counters clear`, `time`, `maintenance`) preview by default and apply only with `--apply`, preserving co-submitted fields and verifying results via independent readback.
- Honor `--protect-port` / `GS308E_PROTECT_PORTS` for port-scoped and global forwarding changes.
- Destructive or disruptive operations require explicit safety flags (`--from` + `--accept-vlan-reset`, `--accept-reconnect`, `--accept-outage`, `--stdout`).
