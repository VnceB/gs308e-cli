# Sessions and batch files

Each normal command signs in, checks the model and firmware, does its work and logs out. To run several commands under **one login**, use `session` (also called `interactive`).

## Interactive

```sh
gs308e interactive
```

Enter the password at the hidden prompt, then type one command per `gs308e>` prompt:

```text
gs308e> ports --summary
gs308e> vlan list
gs308e> exit
```

- `help` or `COMMAND --help` shows syntax. `exit` or Ctrl-D logs out.
- A typo or invalid argument returns you to the prompt.
- For history and line editing, wrap it: `rlwrap gs308e interactive`.
- Edits still preview by default. Add `--apply` to an individual command.

## Batch from a file

Put one command per line in a UTF-8 text file. Blank lines and `#` comments are ignored, and shell-style quoting keeps arguments together.

```text
# reports.txt
firmware
ports --summary
vlan list
```

```sh
gs308e session --read-only -f reports.txt
```

## Batch from the command line

```sh
gs308e --password-stdin --json session --read-only \
  -c 'firmware' \
  -c 'ports --summary' \
  -c 'vlan list' \
  -c 'maintenance cable-results'
```

Without `-f` or `-c`, redirected stdin supplies the password line followed by one command per line.

## Behavior

- `--read-only` restricts the session to passive reports.
- Batch commands (from `-f` or `-c`) are checked for syntax and read-only policy **before** sign-in. A file error reports the physical line number.
- A runtime failure stops the batch, keeps earlier output and attempts logout. Earlier operations are not rolled back.
- Global options such as `--host`, `--timeout`, `--json` and `--protect-port` are set when the session opens. `maintenance backup` and `completions` are standalone only.
- Device commands are spaced at least two seconds apart to avoid overloading the switch's web server.
- The session records the model and firmware on the first dashboard read and stops if they change.
- The cookie stays in memory, is sent only to the configured origin and is never refreshed by signing in again. Expiry ends the session.
- Ctrl-C requests logout after the current command finishes (exit `130`).

## JSON output

With `--json`, a session emits newline-delimited JSON: one report per completed command, then a final `session-end` record. On failure, stderr identifies the command number or file line.

## Logout

The switch often drops the connection while answering the logout request. On exit, `session` and `diagnostics` therefore always check whether the old cookie still opens the dashboard. The `session-end` record reports the logout request and that check separately, so a lost logout response can still coexist with a verified logout. If the old cookie still works or the check is inconclusive, let the session expire on the switch.

One-shot commands do the follow-up check only when the logout request itself fails at the network level.
