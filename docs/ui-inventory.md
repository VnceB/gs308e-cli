# GS308Ev4 Management UI Reference

Maps the GS308Ev4 web management interface (firmware `V1.0.1.13`) to `gs308e` CLI commands. See [status.md](status.md) for verification status.

## Home

| Page or panel | Fields and controls | CLI command |
| --- | --- | --- |
| System Info | Model, firmware, system name, MAC, serial, UI language | `info`, `snapshot`, `settings show/set identity` |
| Connections & Port Status | Link state, negotiated speed, duplex, port label | `ports`, `ports --summary`, `snapshot` |
| Port Status → Edit Port | Label, configured speed, flow control, ingress/egress rate limits, priority (port-based QoS) | `settings show/set port --port N` |
| IP Address | DHCP toggle, static IPv4 address, subnet mask, gateway, primary/secondary DNS | `settings show/set address` |
| Port LEDs | Device-wide port LED enable/disable | `settings show/set leds` |
| System Time | Manual date/time or SNTP sync, time zone index | `time clock`, `time set-clock` |

## Switching

| Page or panel | Fields and controls | CLI command |
| --- | --- | --- |
| QoS | Port-based vs. 802.1P/DSCP mode | `settings show/set qos-mode` |
| QoS → Broadcast Filtering | Global filtering toggle and per-port storm control rate | `settings show/set broadcast-filter`, `storm-rate` |
| VLAN → Mode | Disabled, basic/advanced port-based, basic/advanced 802.1Q | `vlan`, `snapshot`, `vlan set-mode` |
| VLAN → Basic Port / 802.1Q | Per-port VLAN / trunk (`All`) assignment, basic VLAN catalog | `vlan list`, `vlan assign`, `vlan create/rename/delete` |
| VLAN → Advanced Port / 802.1Q | Member masks (tagged/untagged/excluded), PVID table, management VLAN | `vlan list/create/rename/delete/members`, `vlan pvid`, `vlan management` |
| VLAN → Voice VLAN | Voice VLAN ID, CoS (`0-7`), OUI table | `vlan voice`, `vlan oui-add/oui-edit/oui-delete` |
| Link Aggregation | Static LAG groups 1–2 and member ports | `settings show/set lag` |
| Multicast | IGMP snooping, VLAN ID, unknown multicast block, header validation, static router port | `settings show/set multicast` |

## Settings

| Page or tab | Fields and controls | CLI command |
| --- | --- | --- |
| GUI Access → HTTP Configuration | Idle session timeout and maximum concurrent sessions | `settings show/set http-session` |
| DoS Prevention | Device-wide DoS prevention toggle | `settings show/set dos-prevention` |
| Power Saving | IEEE 802.3az energy-saving toggle | `settings show/set power-saving` |
| Switch Discovery | UPnP discovery toggle | `settings show/set discovery` |
| Firmware | Installed version and firmware image upload | `info`, `firmware`, `maintenance upgrade` |
| Configuration File | Binary configuration export (restore excluded) | `maintenance backup --stdout` |
| Factory Default / Reboot | Switch reboot (factory defaults excluded) | `maintenance reboot --accept-outage` |
| Time → SNTP Client | Enable, unicast/broadcast mode, UDP port, poll interval, timeout, retries | `settings show/set sntp-client` |
| Time → SNTP Servers | Unicast server list (address, port, priority, version) | `time servers`, `time server-add`, `time server-delete` |
| Time → Daylight Saving | Regional (EU/US), recurring, or one-off DST rules and active status | `time dst`, `time dst-status`, `time set-dst` |
| Access Control / Password | Allowed IP ACLs and device password changes | Excluded |

## Diagnostics

| Page | Fields and controls | CLI command |
| --- | --- | --- |
| Cable Test | Per-port cable diagnostic and stored result/distance table | `maintenance cable-results`, `maintenance cable-test PORT` |
| Loop Prevention | Global loop prevention toggle | `settings show/set loop-prevention` |
| Port Statistics | 64-bit RX bytes, TX bytes, CRC errors, and global counter reset | `counters`, `snapshot`, `counters clear --all` |
| Port Mirroring | Mirroring toggle, source ports, and destination port | `settings show/set mirror` |
