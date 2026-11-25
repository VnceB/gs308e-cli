# Scripting, JSON and exit codes

## Connection and credentials

- `--host` or `GS308E_HOST` sets the management host or origin. A bare host uses `http://`. Use `https://…` for HTTPS, with `--cacert FILE` for a private CA.
- `--password-stdin` reads the first line of stdin as the password (LF or CRLF, spaces are not trimmed, printable ASCII). Without it, the CLI prompts on the terminal and refuses to run non-interactively.
- Global options work before or after the subcommand.

```sh
gs308e --password-stdin --json counters --port 3 < /run/secrets/switch
```

## JSON reports

Every report has the shape:

```json
{"schema_version":1,"command":"counters","data":[{"port":3,"rx_bytes":"4294967298","tx_bytes":"8192","crc_errors":"0"}]}
```

- Reports go to stdout. Errors and cleanup warnings go to stderr.
- Port lists are always arrays, even when filtered, and use physical port numbers 1–8.
- An idle port has `null` speed and duplex.
- Counters are unsigned 64-bit values encoded as **decimal strings**, so JavaScript and other consumers keep every digit.
- `snapshot` returns `device`, `ports`, `counters` and `vlan` under `data`.
- `firmware --json` also includes `read_profile` and `write_policy`.
- `settings show` lists exposed values under `data.values` and missing controls under `data.unavailable`. Scripts that need a setting should check that it's present in `data.values`.
- Changes return `preview`, `verified`, `unchanged` or, for reboot and upgrade, `accepted`.

## Exit codes

| Exit | Meaning |
| --- | --- |
| 0 | Success |
| 1 | Output could not be written |
| 2 | Invalid usage or missing input |
| 3 | Authentication or session failure |
| 4 | Connection, timeout, redirect, HTTP failure or unverified change |
| 5 | Unsupported model, firmware below V1.0.1.13 or unrecognized page |
| 6 | `diagnostics` finished with a failed check or unconfirmed logout |
| 130 | A multi-command session was interrupted |

## Diagnostics

`gs308e diagnostics --json` signs in once and runs independent checks:

- ports (from the dashboard),
- VLAN mode and VLAN table (from the VLAN page),
- stored cable-test results (it never starts a new test),
- logout verification.

Each check has `status: "ok"` with `data`, or `status: "error"` with `exit_code` and `message`. All checks are reported even when one fails. Exit `0` requires every read to succeed and the switch to reject the old session after logout. Otherwise the command exits `6`. Sign-in or model failures exit earlier with their own code.

An empty cable-result list means the switch reported no stored measurements. Rows the switch marks `Not test` (with an empty or `--` distance) and its placeholder rows with two empty cells are omitted. Malformed tables, such as a repeated port, a missing cell or a distance without a result, produce an error.
