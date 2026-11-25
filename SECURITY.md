# Security policy

## Supported versions

Only the latest release receives fixes.

## Reporting a vulnerability

Please **do not open a public issue** for security problems. Report them privately through GitHub's [private vulnerability reporting](../../security/advisories/new) for this repository.

Include the `gs308e` version, the switch firmware version and steps to reproduce. Please leave out real passwords, session cookies, serial numbers, MAC addresses and raw device captures.

You should get a reply within a week. This is a hobby project maintained in spare time.

## Scope

Examples of issues in scope: credential or session-cookie leakage, requests sent to an origin other than the configured one, a write submitted without `--apply`, or `--protect-port` being bypassed.

Weaknesses of the switch itself, such as its plain-HTTP management interface, are out of scope. Please report those to NETGEAR.
