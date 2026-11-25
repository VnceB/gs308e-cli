# GS308E-CLI Constitution

## I. Scope & Interface

GS308E-CLI is an independent Apache-2.0 Rust client for the NETGEAR GS308Ev4 (minimum firmware `V1.0.1.13`). Interact only through the switch's standard management web forms. Exclude ACL and password management.

## II. Explicit Changes & Safety

Preview configuration changes by default and require `--apply` to submit. Preserve co-submitted sibling form fields, validate scope and hardware dependencies before submitting, verify applied changes via independent readback, and honor `--protect-port`.

## III. Operator & Script Contracts

Support offline `--help` and shell completions. Emit human-readable tables and versioned JSON (`schema_version: 1`) to stdout, and errors/diagnostics to stderr, preserving exit codes (`0`–`6`, `130`). Accept passwords via hidden prompt or `--password-stdin`. Bind sessions and cookies strictly to the configured origin without redirects or proxy leakage.

## IV. Verification

Run `cargo fmt --check`, strict `cargo clippy --locked --all-targets -- -D warnings`, and `cargo test --locked`—either natively with Cargo or via the repository's Docker check target (`docker build --target check .`, which CI runs). Use synthetic HTML fixtures and reserved example addresses (`192.0.2.x`, `.example`, `.invalid`). Never commit credentials, session material, or real device captures.
