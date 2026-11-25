# GS308Ev4 Feature Status

This table tracks `gs308e` CLI commands, automated synthetic test coverage, and hardware verification on the NETGEAR GS308Ev4 (minimum supported firmware `V1.0.1.13`). [Vendor release](https://kb.netgear.com/000070621/GS308Ev4-Firmware-Version-1-0-1-13) · [Model manual](https://www.downloads.netgear.com/files/GDC/GS308Ev4/GS308Ev4_UM_EN.pdf) · [UI inventory](ui-inventory.md)

| Feature | Public command | Automated fixture coverage | Hardware verification |
| --- | --- | --- | --- |
| Firmware check & write gate | `firmware`, `--apply` | Enforces `V1.0.1.13` minimum (exits `5` below or unidentified), pins session firmware, and restricts newer releases to matched forms. | Verified on GS308Ev4 `V1.0.1.13`. |
| Session & scripting | `session`, `interactive`, `--json`, `--cacert`, `--explain-login` | Origin-bound login/logout, bare-host HTTP default, SID classification, `-c`/`-f`/stdin batches, 2s command pacing, `--read-only`, interrupt and expiry handling. | Batch/interactive read sessions and logout verification tested on `V1.0.1.13`. HTTPS and `--cacert` await hardware verification. |
| Single-session diagnostics | `diagnostics` | Independent reads for ports, VLAN mode, VLAN list, and stored cable results, plus logout verification and partial-failure exit `6`. | Ports, VLAN mode/table, stored cable results, and logout verification tested on `V1.0.1.13`. |
| Dashboard & counters | `info`, `ports`, `ports --summary`, `counters`, `snapshot` | Model/firmware checks, all 8 port rows, 64-bit counter reconstruction, and combined snapshot. | Verified on `V1.0.1.13`. |
| VLAN mode | `vlan`, `vlan list`, `vlan set-mode` | All five VLAN modes, `--from` + `--accept-vlan-reset` guards, preview, and readback. | Mode/disabled-table read verified on `V1.0.1.13`; populated tables and mode switches await hardware testing. |
| Basic port & 802.1Q VLANs | `vlan assign`, `create`, `rename`, `delete` | Complete 8-port forms, trunk/access dependencies, and `--protect-port` checks. | Synthetic tests only; hardware verification pending. |
| Advanced port & 802.1Q VLANs | `vlan create/rename/delete/members`, `pvid`, `management` | Member bitmasks, tagged/untagged/excluded ports, PVID consistency, management VLAN membership, and readback. | Synthetic tests only; hardware verification pending. |
| Voice VLAN & OUI table | `vlan voice`, `oui-add`, `oui-edit`, `oui-delete` | CoS range (`0-7`), 3-byte OUI validation, row index binding, and readback. | Synthetic tests only; hardware verification pending. |
| Port editor, identity & LEDs | `settings show/set port/identity/leds` | Grouped port editor preservation, unavailable-field reporting, disabled-Apply validation on LEDs, and readback. | LED toggle read/write and port editor read verified on `V1.0.1.13`. |
| QoS, broadcast & energy | `settings show/set qos-mode/broadcast-filter/storm-rate/power-saving/loop-prevention` | Mode/toggle forms, storm-filtering prerequisite, all-port rate preservation, and readback. | Power Saving toggle verified on `V1.0.1.13`. |
| LAG, mirroring & multicast | `settings show/set lag/mirror/multicast` | LAG speed/flow/VLAN consistency, mirror endpoint conflicts, and IGMP VLAN dependency checks. | Synthetic tests only; hardware verification pending. |
| Address, discovery, HTTP & DoS | `settings show/set address/discovery/http-session/dos-prevention` | IPv4/subnet/gateway/DNS validation, retired NSDP control handling on `V1.0.1.13`, and HTTP session bounds. | Discovery, HTTP session, and DoS reads verified on `V1.0.1.13`; writes pending. |
| Clock, SNTP & daylight saving | `time clock/set-clock/status/servers/server-add/server-delete/dst/dst-status/set-dst`, `settings show/set sntp-client` | Polling ranges, parent token lookup, DNS/IPv4 servers, priority/address checks, DST rules, and leap-date clock readback. | Synthetic tests only; hardware verification pending. |
| Clear port statistics | `counters clear --all` | Global form submission and zero-counter readback. | Synthetic tests only; hardware verification pending. |
| Cable diagnostics & reboot | `maintenance cable-results/cable-test/reboot` | Stored result/distance parsing, `Not test` and blank-pair handling on `V1.0.1.13`, `--protect-port`, and `--accept-outage`. | `cable-results` read verified on `V1.0.1.13`; active cable test and reboot pending. |
| Configuration backup | `maintenance backup --stdout` | Requires redirected stdout and `Content-Disposition: attachment` from `/confFile.cgi?cmd=backup_conf`. | Synthetic tests only; hardware verification pending. |
| Firmware upgrade | `maintenance upgrade IMAGE --accept-outage` | SHA-256 verification against the `V1.0.1.13` image and multipart upload contract. | Reserved for future firmware targets (already-installed `V1.0.1.13` reports `unchanged`). |
| Excluded features | — | Configuration restore, factory defaults, ACL management, and password changes have no CLI commands. | Intentionally excluded. |

## Firmware Notes

- **Logout behavior**: The GS308Ev4 web server often closes the TCP connection mid-response on `POST /logout.cgi`. `session` and `diagnostics` always verify logout with a follow-up `GET /dashboard.cgi` (expecting the login page or HTTP `401`/`403`); one-shot commands perform that follow-up check only when the logout request fails at the transport layer.
- **Stored cable results**: On `V1.0.1.13`, untested ports may render `Not test` (with `--` or blank distance) or two empty `<span>` cells. Both are omitted cleanly while preserving populated results and fault distances.
- **Power Saving & LEDs**: On `V1.0.1.13`, `settings set power-saving` uses `/powerSaving.cgi` (which carries the `hash` token, unlike `/green_ethernet.cgi`), and `settings set leds` validates the edit-to-enable `enableButtons()` / `submitLED()` form contract on `/leds.cgi`.

## Verification

Run formatting, strict Clippy, and all tests natively or via Docker (matching CI):

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

```sh
docker build --target check .
```
