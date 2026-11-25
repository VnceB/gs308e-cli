# Sign-in troubleshooting

Add `--explain-login` to any command to classify the two sign-in requests (the login form GET and the credential POST) on stderr. It adds no requests and no retries.

```sh
gs308e --password-stdin --explain-login --json session -c 'ports --summary'
```

Each `login-check` line contains the HTTP method, status, page category and cookie category. It never prints cookie or header values, URLs, page text or password material, so it's safe to paste into a bug report together with the exit code.

## Page categories

- `invalid-password`, `temporary-lock`, `session-limit`: explicit notices from the switch. On `session-limit`, wait for existing sessions to expire.
- `login-form`: the login page again, with no stated reason.
- `index-navigation`: a known landing page. Neither this nor HTTP 200 proves sign-in worked. The following dashboard read decides that.
- Other categories identify redirects, denials, unfamiliar notices and unknown documents.

## Cookie categories

| Category | Meaning |
| --- | --- |
| `usable-sid`, `usable-quoted-sid` | The credential POST issued one valid session cookie. It's sent back verbatim (including quotes) to the same origin. |
| `usable-backslash-sid` | A session cookie containing a literal backslash, received on the recognized successful-login reply. Its bytes are preserved. |
| `sid-backslash` | A backslash cookie seen outside that successful reply. |
| `no-set-cookie`, `no-sid` | No cookie header, or only unrelated cookies. |
| `empty-sid`, `deleted-sid`, `expired-sid` | The cookie was empty, had a non-positive Max-Age or had already expired. |
| `sid-missing-equals`, `sid-invalid-quotes`, `sid-whitespace`, `sid-comma`, `sid-comma-and-backslash`, `sid-invalid-octet` | Malformed cookie value. |
| `non-text-sid-header`, `multiple-sids` | Non-text bytes in the header, or more than one session cookie. |
| `sid-invalid-lifetime`, `sid-scope-mismatch` | The lifetime or scope can't be honored on this origin. |

Cookies are parsed following the [RFC 6265 user-agent rules](https://datatracker.ietf.org/doc/html/rfc6265#section-5.2). Ambiguous, malformed or expired cookies fail closed (exit `3`). Each invocation makes at most one sign-in attempt.
