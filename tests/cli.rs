use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Output, Stdio},
    thread,
    time::Duration,
};
use tiny_http::{Header, Response, Server};

const LOGIN: &str = include_str!("fixtures/login.html");
const DASH: &str = include_str!("fixtures/dashboard.html");
const STATS: &str = include_str!("fixtures/statistics.html");
const VLAN: &str = include_str!("fixtures/vlan.html");

struct Exchange {
    route: &'static str,
    status: u16,
    body: String,
    request_body: &'static str,
    response_headers: Vec<(&'static str, String)>,
    binary_cookie: Option<&'static [u8]>,
    expected_cookie: String,
    response_delay: Duration,
    minimum_gap: Duration,
}
impl Exchange {
    fn new(route: &'static str, body: impl Into<String>) -> Self {
        Self {
            route,
            status: 200,
            body: body.into(),
            request_body: "",
            response_headers: vec![],
            binary_cookie: None,
            expected_cookie: "SID=synthetic".into(),
            response_delay: Duration::ZERO,
            minimum_gap: Duration::ZERO,
        }
    }
    fn with_header(mut self, key: &'static str, value: &str) -> Self {
        self.response_headers.push((key, value.into()));
        self
    }
}

fn login() -> Vec<Exchange> {
    vec![
        Exchange::new("GET /login.cgi", LOGIN),
        Exchange::new(
            "POST /login.cgi",
            "<script>location.href='/index.cgi'</script>",
        )
        .with_header("Set-Cookie", "SID=synthetic; Path=/; HttpOnly"),
    ]
}

fn cli(args: &[&str], with_input: bool) -> Output {
    cli_input(args, if with_input { b"mock-only!\n" } else { b"" })
}

fn cli_input(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_gs308e"))
        .args(args)
        .env_remove("GS308E_HOST")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if !input.is_empty() {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn run(args: &[&str], steps: Vec<Exchange>) -> Output {
    run_input(args, steps, b"mock-only!\n")
}

fn run_input(args: &[&str], steps: Vec<Exchange>, input: &[u8]) -> Output {
    run_with(args, steps, |args| cli_input(args, input))
}

fn run_with(
    args: &[&str],
    steps: Vec<Exchange>,
    run_cli: impl FnOnce(&[&str]) -> Output,
) -> Output {
    let server = Server::http("127.0.0.1:0").unwrap();
    let host = format!("http://{}", server.server_addr());
    let worker = thread::spawn(move || {
        let mut previous = std::time::Instant::now();
        for step in steps {
            let mut req = server
                .recv_timeout(Duration::from_secs(4))
                .unwrap()
                .expect("missing request");
            let route = format!("{} {}", req.method(), req.url());
            assert_eq!(route, step.route);
            assert!(
                previous.elapsed() >= step.minimum_gap,
                "command started before its post-response gap"
            );
            let mut cookies = req.headers().iter().filter(|h| h.field.equiv("Cookie"));
            let received = cookies.next().map(|h| h.value.as_str());
            let expected = (req.url() != "/login.cgi").then_some(step.expected_cookie.as_str());
            assert!(
                received == expected && cookies.next().is_none(),
                "outgoing Cookie mismatch"
            );
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body).unwrap();
            if route == "POST /login.cgi" {
                assert!(
                    body == format!("password={:x}", md5::compute(b"m4o2ck-only!")),
                    "unexpected fixture login body"
                );
            } else {
                assert_eq!(body, step.request_body);
            }
            if step.status == 0 {
                drop(req.into_writer());
                continue;
            }
            if let Some(cookie) = step.binary_cookie {
                // tiny_http's response builder accepts ASCII only. Exercise obs-text
                // through the real HTTP decoder without printing the test header.
                let mut wire = req.into_writer();
                write!(wire, "HTTP/1.1 200 OK\r\nConnection: close\r\nSet-Cookie: ").unwrap();
                wire.write_all(cookie).unwrap();
                write!(
                    wire,
                    "\r\nContent-Length: {}\r\n\r\n{}",
                    step.body.len(),
                    step.body
                )
                .unwrap();
                wire.flush().unwrap();
                continue;
            }
            let mut response = Response::from_string(step.body).with_status_code(step.status);
            for (name, value) in step.response_headers {
                response = response.with_header(Header::from_bytes(name, value).unwrap());
            }
            thread::sleep(step.response_delay);
            req.respond(response).unwrap();
            previous = std::time::Instant::now();
        }
    });
    let mut full = vec!["--host", &host, "--password-stdin", "--timeout", "2"];
    full.extend(args);
    let result = run_cli(&full);
    worker.join().unwrap();
    result
}

fn view(args: &[&str], dashboard: &str, extra: Option<Exchange>) -> Output {
    let mut steps = login();
    steps.push(Exchange::new("GET /dashboard.cgi", dashboard));
    steps.extend(extra);
    steps.push(Exchange::new("POST /logout.cgi", ""));
    run(args, steps)
}

fn json(out: Output) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
    v
}

fn fails(out: Output, code: i32) {
    assert_eq!(
        out.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("error:"));
    assert!(!err.contains("mock-only!") && !err.contains("SID="));
}

#[test]
fn info_reports_decoded_fields() {
    let r = json(view(&["--json", "info"], DASH, None));
    assert_eq!(r["command"], "info");
    assert_eq!(r["data"]["name"], "Example & lab");
    assert_eq!(r["data"]["firmware"], "V1.0.1.13");
    assert_eq!(r["data"]["model"], "GS308Ev4");
}

#[test]
fn ports_returns_all_rows_or_one_selected_port() {
    let v = json(view(&["ports", "--json"], DASH, None));
    assert_eq!(v["data"].as_array().unwrap().len(), 8);
    assert_eq!(v["data"][2]["status"], "available");
    assert!(v["data"][2]["link_mbps"].is_null());
    let v = json(view(&["ports", "--port", "2", "--json"], DASH, None));
    assert_eq!(v["data"].as_array().unwrap().len(), 1);
    assert_eq!(v["data"][0]["port"], 2);
    assert_eq!(v["data"][0]["link_mbps"], 100);
    assert_eq!(v["data"][0]["duplex"], "half");
}

#[test]
fn counters_reconstruct_64_bit_words_exactly() {
    let v = json(view(
        &["counters", "--port", "1", "--json"],
        DASH,
        Some(Exchange::new("GET /portStatistics.cgi", STATS)),
    ));
    let r = &v["data"][0];
    assert_eq!(r["rx_bytes"], u64::MAX.to_string());
    assert_eq!(r["tx_bytes"], "4294967298");
    assert_eq!(r["crc_errors"], "0");
}

#[test]
fn vlan_translates_the_active_button() {
    for (value, name) in [
        ("0", "disabled"),
        ("1", "basic_port_based"),
        ("2", "advanced_port_based"),
        ("3", "basic_802_1q"),
        ("4", "advanced_802_1q"),
    ] {
        let page = VLAN.replace("value=\"0\"", &format!("value=\"{value}\""));
        let r = json(view(
            &["--json", "vlan"],
            DASH,
            Some(Exchange::new("GET /vlan.cgi", page)),
        ));
        assert_eq!(r["data"]["mode"], name);
    }
}

#[test]
fn auth_failure_and_expiry_do_not_emit_data_or_retry() {
    let steps = vec![
        Exchange::new("GET /login.cgi", LOGIN),
        Exchange::new("POST /login.cgi", LOGIN),
    ];
    let reject = |out: Output, expected: &str| {
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(expected), "{err}");
        assert!(!err.contains("private-response-marker"));
        fails(out, 3);
    };
    reject(
        run(&["info"], steps),
        "POST /login.cgi: received the login page (HTTP 200) without a usable SID",
    );
    reject(
        view(&["info"], "<title>Redirect to Login</title>", None),
        "GET /dashboard.cgi: the switch returned its login page after issuing a session cookie",
    );
    reject(
        view(&["vlan"], DASH, Some(Exchange::new("GET /vlan.cgi", LOGIN))),
        "GET /vlan.cgi: the switch returned its login page",
    );
    let mut login_denied = Exchange::new("POST /login.cgi", "private-response-marker");
    login_denied.status = 401;
    reject(
        run(
            &["info"],
            vec![Exchange::new("GET /login.cgi", LOGIN), login_denied],
        ),
        "POST /login.cgi: the switch denied the request (HTTP 401)",
    );
    let no_cookie = Exchange::new("POST /login.cgi", "private-response-marker");
    reject(
        run(
            &["info"],
            vec![Exchange::new("GET /login.cgi", LOGIN), no_cookie],
        ),
        "POST /login.cgi: received a response (HTTP 200) without a usable SID",
    );
    let mut forbidden = Exchange::new("GET /dashboard.cgi", "");
    forbidden.status = 403;
    let mut steps = login();
    steps.extend([forbidden, Exchange::new("POST /logout.cgi", "")]);
    reject(
        run(&["info"], steps),
        "GET /dashboard.cgi: the switch denied the request (HTTP 403)",
    );
}

const INDEX_REPLY: &str = r#"<script>
    var url = document.location.href;
    if (url.indexOf("index.cgi") == -1) {
        document.location.href = url.substring(0,url.lastIndexOf("/"))+"/index.cgi";
    }
</script>"#;

fn check_signin(steps: Vec<Exchange>, code: i32, expected: &str) {
    let out = run(&["--explain-login", "--json", "ports", "--summary"], steps);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("private-marker") && !stdout.contains("SID="));
    assert_eq!(out.status.code(), Some(code), "{stderr}");
    assert!(stderr.contains(expected), "{stderr}");
    assert!(
        !stderr.contains("private-marker")
            && !stderr.contains("mock-only!")
            && !stderr.contains("SID=")
    );
    assert!(!stderr.contains(&format!("{:x}", md5::compute(b"m4o2ck-only!"))));
    if code != 0 {
        assert!(out.stdout.is_empty());
    } else {
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap()["command"],
            "ports-summary"
        );
    }
}

#[test]
fn explain_login_separates_vendor_notices_from_form_and_cookie_delivery() {
    for (body, kind, cookie) in [
        (format!("{LOGIN}<div class=pwdErrStyle>Invalid Password</div>"),"invalid-password","usable-sid"),
        (format!("{LOGIN}<div class=pwdErrStyle>Your account is temporarily locked. Try again in @12@ minutes.</div>"),"temporary-lock","no-set-cookie"),
        ("<div class=pwdErrStyle>The maximum number of sessions has been reached. Wait a few minutes and then try again.</div>".into(),"session-limit","no-set-cookie"),
        (format!("{LOGIN}<div class=pwdErrStyle></div>"),"login-form","no-set-cookie"),
        ("<title>Redirect to Login</title>".into(),"login-navigation","no-set-cookie"),
        ("<div class=pwdErrStyle>private-marker</div>".into(),"unrecognized-notice","no-set-cookie"),
        ("<script>let msg = 'session max wait private-marker'</script>".into(),"other-script-document","no-set-cookie"),
        (
            format!("{LOGIN}<div hidden><p class=pwdErrStyle>Invalid Password</p></div>"),
            "login-form",
            "no-set-cookie",
        ),
        (
            "<body>The maximum number of sessions has been reached. Wait a few minutes and then try again.</body>".into(),
            "session-limit",
            "no-set-cookie",
        ),
        (String::new(),"empty-body","no-set-cookie"),
        (INDEX_REPLY.into(),"index-navigation","no-set-cookie"),
    ] {
        let mut post=Exchange::new("POST /login.cgi",body);
        if cookie=="usable-sid" { post=post.with_header("Set-Cookie","SID=private-marker"); }
        check_signin(vec![Exchange::new("GET /login.cgi",LOGIN),post],3,&format!("method=POST http=200 page={kind} cookie={cookie}"));
    }
    check_signin(
        vec![Exchange::new(
            "GET /login.cgi",
            format!(
                "{LOGIN}<div class=pwdErrStyle>The maximum number of sessions has been reached. Wait a few minutes and then try again.</div>"
            ),
        )],
        3,
        "method=GET http=200 page=session-limit",
    );
}

#[test]
fn explain_login_rejects_missing_empty_malformed_and_ambiguous_sids() {
    for (headers, label) in [
        (vec!["OTHER=private-marker"], "no-sid"),
        (vec!["SID=; Path=/"], "empty-sid"),
        (vec!["SID=private-marker,invalid"], "sid-comma"),
        (vec!["SID=private-marker,\\bad"], "sid-comma-and-backslash"),
        (vec!["SID=\"private-marker,invalid\""], "sid-comma"),
        (vec!["SID=\"\""], "empty-sid"),
        (vec!["SID"], "sid-missing-equals"),
        (vec!["SID=\"private-marker"], "sid-invalid-quotes"),
        (vec!["SID=pri\"vate-marker"], "sid-invalid-quotes"),
        (vec!["SID=\"private-marker space\""], "sid-whitespace"),
        (vec!["SID=private-marker; Max-Age=0"], "deleted-sid"),
        (vec!["SID=; Max-Age=-1"], "deleted-sid"),
        (
            vec!["SID=\"private-marker\"; Max-Age=-99999999999999999999999"],
            "deleted-sid",
        ),
        (
            vec!["SID=private-marker; Expires=Thu, 01 Jan 1970 00:00:00 GMT"],
            "expired-sid",
        ),
        (
            vec!["SID=private-marker; Expires=private-marker"],
            "sid-invalid-lifetime",
        ),
        (
            vec!["SID=private-marker; Max-Age=+1"],
            "sid-invalid-lifetime",
        ),
        (vec!["SID=private-marker; Secure"], "sid-scope-mismatch"),
        (
            vec!["SID=private-marker; Domain=example.invalid"],
            "sid-scope-mismatch",
        ),
        (
            vec!["SID=private-marker; Path=/elsewhere"],
            "sid-scope-mismatch",
        ),
        (
            vec!["SID=private-marker", " SID = ; Max-Age=0"],
            "multiple-sids",
        ),
        (
            vec!["SID=private-marker", "SID=second-private-marker"],
            "multiple-sids",
        ),
    ] {
        let mut post = Exchange::new("POST /login.cgi", INDEX_REPLY);
        for header in headers {
            post = post.with_header("Set-Cookie", header);
        }
        check_signin(
            vec![Exchange::new("GET /login.cgi", LOGIN), post],
            3,
            &format!("page=index-navigation cookie={label}"),
        );
    }
    let mut redirect = Exchange::new("POST /login.cgi", "private-marker")
        .with_header("Location", "https://example.invalid/?token=private-marker");
    redirect.status = 302;
    check_signin(
        vec![Exchange::new("GET /login.cgi", LOGIN), redirect],
        4,
        "method=POST http=302 page=http-redirect",
    );
    let mut denied = Exchange::new("POST /login.cgi", "private-marker");
    denied.status = 403;
    check_signin(
        vec![Exchange::new("GET /login.cgi", LOGIN), denied],
        3,
        "method=POST http=403 page=http-denial",
    );
}

#[test]
fn explain_login_keeps_authorization_bound_to_the_post_and_model_check() {
    let steps = vec![
        Exchange::new("GET /login.cgi", LOGIN).with_header("Set-Cookie", "SID=private-marker"),
        Exchange::new("POST /login.cgi", INDEX_REPLY)
            .with_header("Set-Cookie", "SID=synthetic ; HttpOnly"),
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new("POST /logout.cgi", ""),
    ];
    check_signin(
        steps,
        0,
        "method=POST http=200 page=index-navigation cookie=usable-sid",
    );
}

#[test]
fn non_text_sid_is_rejected_before_any_authenticated_request() {
    for field in [
        b"SID=private-marker\xff".as_slice(),
        b"SID=synthetic; Path=/\x80",
    ] {
        let mut post = Exchange::new("POST /login.cgi", INDEX_REPLY);
        post.binary_cookie = Some(field);
        check_signin(
            vec![Exchange::new("GET /login.cgi", LOGIN), post],
            3,
            "page=index-navigation cookie=non-text-sid-header",
        );
    }
}

#[test]
fn quoted_sid_round_trips_unchanged_with_max_age_taking_precedence() {
    // Quotes, padding and literal percent escapes are part of the received value.
    let quoted = r#"SID="private-marker%21==""#;
    let mut dashboard = Exchange::new("GET /dashboard.cgi", DASH);
    dashboard.expected_cookie = quoted.into();
    let mut logout = Exchange::new("POST /logout.cgi", "");
    logout.expected_cookie = quoted.into();
    check_signin(vec![
        Exchange::new("GET /login.cgi", LOGIN),
        Exchange::new("POST /login.cgi", INDEX_REPLY)
            .with_header("Set-Cookie", r#" SID = "private-marker%21==" ; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=120; Path=/; HttpOnly"#),
        dashboard, logout,
    ], 0, "page=index-navigation cookie=usable-quoted-sid");
}

#[test]
fn browser_compatible_sid_is_sent_byte_for_byte_after_index_post_only() {
    let nonce = format!(
        "{:016x}{:016x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
    );
    for value in [
        format!(r"private-marker\{nonce}%5C=="),
        format!(r#""\private-marker{nonce}\\""#),
    ] {
        let cookie = format!("SID={value}");
        let mut dashboard = Exchange::new("GET /dashboard.cgi", DASH);
        let mut logout = Exchange::new("POST /logout.cgi", "");
        dashboard.expected_cookie = cookie.clone();
        logout.expected_cookie = cookie.clone();
        check_signin(
            vec![
                Exchange::new("GET /login.cgi", LOGIN),
                Exchange::new("POST /login.cgi", INDEX_REPLY)
                    .with_header("Set-Cookie", &format!("{cookie}; Path=/; HttpOnly")),
                dashboard,
                logout,
            ],
            0,
            "method=POST http=200 page=index-navigation cookie=usable-backslash-sid",
        );
    }
}

#[test]
fn backslash_receipt_requires_the_recognized_200_credential_reply() {
    let cookie = r"SID=private-marker\synthetic";
    for (status, body, kind, code) in [
        (200, LOGIN, "login-form", 3),
        (200, "<p>other</p>", "other-document", 3),
        (
            200,
            "<div class=pwdErrStyle>Invalid Password</div>",
            "invalid-password",
            3,
        ),
        (201, INDEX_REPLY, "index-navigation", 3),
        (302, INDEX_REPLY, "http-redirect", 4),
    ] {
        let mut post = Exchange::new("POST /login.cgi", body).with_header("Set-Cookie", cookie);
        post.status = status;
        check_signin(
            vec![Exchange::new("GET /login.cgi", LOGIN), post],
            code,
            &format!("method=POST http={status} page={kind} cookie=sid-backslash"),
        );
    }
    check_signin(
        vec![
            Exchange::new("GET /login.cgi", LOGIN).with_header("Set-Cookie", cookie),
            Exchange::new("POST /login.cgi", INDEX_REPLY),
        ],
        3,
        "method=POST http=200 page=index-navigation cookie=no-set-cookie",
    );
}

#[test]
fn opaque_backslash_candidate_is_withheld_on_unreadable_or_lookalike_landing_pages() {
    let cookie = r"SID=private-marker\generated";
    let changed = INDEX_REPLY.replace(r#""/index.cgi""#, r#""/index .cgi""#);
    for body in [
        changed,
        format!("<form></form>{INDEX_REPLY}"),
        format!("<template>{INDEX_REPLY}</template>"),
    ] {
        check_signin(
            vec![
                Exchange::new("GET /login.cgi", LOGIN),
                Exchange::new("POST /login.cgi", body).with_header("Set-Cookie", cookie),
            ],
            3,
            "cookie=sid-backslash",
        );
    }
    let unreadable = Exchange::new("POST /login.cgi", "x".repeat(1024 * 1024 + 1))
        .with_header("Set-Cookie", cookie);
    check_signin(
        vec![Exchange::new("GET /login.cgi", LOGIN), unreadable],
        5,
        "page=unreadable-body cookie=sid-backslash",
    );
}

#[test]
fn confirmed_backslash_cookie_stays_on_issuing_origin_after_dashboard_redirect() {
    let foreign = Server::http("127.0.0.1:0").unwrap();
    let cookie = r"SID=private-marker\generated";
    let mut redirect = Exchange::new("GET /dashboard.cgi", "").with_header(
        "Location",
        &format!("http://{}/other", foreign.server_addr()),
    );
    redirect.status = 307;
    redirect.expected_cookie = cookie.into();
    let mut logout = Exchange::new("POST /logout.cgi", "");
    logout.expected_cookie = cookie.into();
    let out = run(
        &["--json", "--explain-login", "firmware"],
        vec![
            Exchange::new("GET /login.cgi", LOGIN),
            Exchange::new("POST /login.cgi", INDEX_REPLY).with_header("Set-Cookie", cookie),
            redirect,
            logout,
        ],
    );
    assert!(foreign.try_recv().unwrap().is_none());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("private-marker"));
    fails(out, 4);
}

#[test]
fn landing_page_alone_cannot_authorize_an_ambiguous_or_expired_backslash_sid() {
    for (fields, label) in [
        (vec![r"SID=private-marker\x; Max-Age=0"], "deleted-sid"),
        (
            vec![r"SID=private-marker\x; Max-Age=0; Max-Age=30"],
            "sid-invalid-lifetime",
        ),
        (
            vec![r"SID=private-marker\x; Expires=Thu, 01 Jan 1970 00:00:00 GMT"],
            "expired-sid",
        ),
        (
            vec![r"SID=private-marker\x; Path=/, SID=another"],
            "multiple-sids",
        ),
        (
            vec![r"SID=private-marker\x", "SID=another"],
            "multiple-sids",
        ),
        (vec![r"SID=private-marker\x; SID=another"], "multiple-sids"),
        (
            vec![r#"SID="private-marker\x;other=1""#],
            "sid-invalid-quotes",
        ),
        (vec!["SID=private-marker\\x\tvalue"], "sid-whitespace"),
        (
            vec![r"SID=private-marker\x; Domain=example.invalid"],
            "sid-scope-mismatch",
        ),
        (vec![r"SID=private-marker\x; Secure"], "sid-scope-mismatch"),
    ] {
        let mut post = Exchange::new("POST /login.cgi", INDEX_REPLY);
        for field in fields {
            post = post.with_header("Set-Cookie", field);
        }
        check_signin(
            vec![Exchange::new("GET /login.cgi", LOGIN), post],
            3,
            &format!("page=index-navigation cookie={label}"),
        );
    }
}

#[test]
fn redirects_never_deliver_proof_or_session_to_another_origin() {
    let other = Server::http("127.0.0.1:0").unwrap();
    let dest = format!("http://{}/elsewhere", other.server_addr());
    for stage in 0..3 {
        let mut steps = login();
        steps.push(Exchange::new("GET /dashboard.cgi", DASH));
        steps.truncate(stage + 1);
        let redirect = steps.last_mut().unwrap();
        redirect.response_headers = vec![("Location", dest.clone())];
        redirect.status = 307;
        if stage == 2 {
            steps.push(Exchange::new("POST /logout.cgi", ""));
        }
        fails(run(&["info"], steps), 4);
        assert!(other.try_recv().unwrap().is_none());
    }
}

#[test]
fn rejects_foreign_form_actions_before_sending_proof() {
    let page = LOGIN.replace(
        "action=\"/login.cgi\"",
        "action=\"https://other.invalid/login.cgi\"",
    );
    fails(
        run(&["info"], vec![Exchange::new("GET /login.cgi", page)]),
        5,
    );
}

#[test]
fn malformed_or_wrong_model_pages_fail_closed() {
    fails(view(&["info"], &DASH.replace("GS308Ev4", "Other"), None), 5);
    fails(
        view(&["ports"], &DASH.replace("isShowPot8", "isShowPot7"), None),
        5,
    );
    fails(
        view(
            &["counters"],
            DASH,
            Some(Exchange::new(
                "GET /portStatistics.cgi",
                STATS.replacen("4294967295", "NaN", 1),
            )),
        ),
        5,
    );
    fails(
        view(
            &["vlan"],
            DASH,
            Some(Exchange::new("GET /vlan.cgi", "<p>unrecognized</p>")),
        ),
        5,
    );
}

#[test]
fn human_output_and_local_shell_completions() {
    let out = view(&["ports", "--port", "2"], DASH, None);
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("PORT") && s.contains("100 Mbps") && s.contains("Example NAS"));
    let out = cli(&["completions", "zsh"], false);
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("#compdef gs308e")
    );
}

#[test]
fn invalid_options_and_unattended_passwords_fail_before_network() {
    for args in [
        &["--host", "http://example.invalid", "info"][..],
        &["--host", "example.invalid", "info"],
        &["--host", "https://example.invalid", "info"],
        &["--host", "ftp://example.invalid", "info"],
        &["--host", "https://u:p@example.invalid", "info"],
        &["--host", "https://example.invalid/path", "info"],
        &["--host", "example.invalid/path", "info"],
        &["ports", "--port", "9"],
        &["info", "--timeout", "0"],
    ] {
        fails(cli(args, false), 2);
    }
}

#[test]
fn bare_host_defaults_to_http_and_invalid_cacert_is_rejected() {
    let server = Server::http("127.0.0.1:0").unwrap();
    let bare = server.server_addr().to_string();
    let steps = vec![
        Exchange::new("GET /login.cgi", LOGIN),
        Exchange::new("POST /login.cgi", INDEX_REPLY)
            .with_header("Set-Cookie", "SID=synthetic; Path=/; HttpOnly"),
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new("POST /logout.cgi", ""),
    ];
    let worker = thread::spawn(move || {
        for step in steps {
            let req = server
                .recv_timeout(Duration::from_secs(4))
                .unwrap()
                .expect("missing request");
            let mut response = Response::from_string(step.body).with_status_code(step.status);
            for (name, value) in step.response_headers {
                response = response.with_header(Header::from_bytes(name, value).unwrap());
            }
            req.respond(response).unwrap();
        }
    });
    let out = cli(
        &[
            "--host",
            &bare,
            "--password-stdin",
            "--json",
            "ports",
            "--summary",
        ],
        true,
    );
    worker.join().unwrap();
    assert_eq!(json(out)["command"], "ports-summary");

    fails(
        cli(
            &["--host", "switch.example", "--cacert", "Cargo.toml", "info"],
            false,
        ),
        2,
    );
    fails(
        cli(
            &[
                "--host",
                "switch.example",
                "--cacert",
                "missing-ca.pem",
                "info",
            ],
            false,
        ),
        2,
    );
}

#[test]
fn snapshot_uses_one_login_and_one_dashboard_read() {
    let mut steps = login();
    steps.extend([
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new("GET /portStatistics.cgi", STATS),
        Exchange::new("GET /vlan.cgi", VLAN),
        Exchange::new("POST /logout.cgi", ""),
    ]);
    let r = json(run(&["snapshot", "--json"], steps));
    assert_eq!(r["command"], "snapshot");
    assert_eq!(r["data"]["device"]["model"], "GS308Ev4");
    assert_eq!(r["data"]["ports"].as_array().unwrap().len(), 8);
    assert_eq!(r["data"]["counters"].as_array().unwrap().len(), 8);
    assert_eq!(r["data"]["vlan"]["mode"], "disabled");
}

const NO_CABLE_RESULTS: &str = "<div id=testResult><ul class=testResult_list></ul></div>";

fn diagnostic_session(
    dashboard: &str,
    vlan: Exchange,
    cable: &str,
    logout: Exchange,
    after: Exchange,
) -> Vec<Exchange> {
    let mut steps = login();
    steps.extend([
        Exchange::new("GET /dashboard.cgi", dashboard),
        vlan,
        Exchange::new("GET /cableTester.cgi", cable),
        logout,
        after,
    ]);
    steps
}

fn diagnostic_output(steps: Vec<Exchange>, exit: i32) -> Value {
    let out = run(&["diagnostics", "--json"], steps);
    let err = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(exit), "{err}");
    assert_eq!(err.is_empty(), exit == 0);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        !text.contains("SID=")
            && !text.contains("mock-only!")
            && !text.contains("private-response-marker")
    );
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "diagnostics");
    report["data"].clone()
}

#[test]
fn diagnostics_reads_each_page_once_and_confirms_end_of_session() {
    let mut after = Exchange::new("GET /dashboard.cgi", "");
    after.status = 401;
    let d = diagnostic_output(
        diagnostic_session(
            DASH,
            Exchange::new("GET /vlan.cgi", VLAN),
            NO_CABLE_RESULTS,
            Exchange::new("POST /logout.cgi", ""),
            after,
        ),
        0,
    );
    assert_eq!(d["ports"]["data"].as_array().unwrap().len(), 8);
    assert_eq!(d["vlan_mode"]["data"]["mode"], "disabled");
    assert_eq!(d["vlan_list"]["data"]["mode"], "disabled");
    assert_eq!(d["cable_results"]["data"], serde_json::json!([]));
    assert_eq!(d["logout_request"]["data"], 200);
    assert_eq!(
        d["logout_verification"]["data"],
        "dashboard denied the previous session with HTTP 401"
    );
    for check in d.as_object().unwrap().values() {
        assert_eq!(check["status"], "ok");
    }
}

#[test]
fn diagnostics_preserves_independent_reads_after_parser_or_request_errors() {
    let broken_ports = DASH.replace("isShowPot8", "isShowPot7");
    let incomplete_vlan = VLAN.replace("value=\"0\"", "value=\"3\"");
    let results = "<div id=testResult><ul class=testResult_list><li class=testResult_li><span class=testResult_content_port>4</span><span class=testResult_content_result>Normal</span></li></ul></div>";
    let d = diagnostic_output(
        diagnostic_session(
            &broken_ports,
            Exchange::new("GET /vlan.cgi", incomplete_vlan),
            results,
            Exchange::new("POST /logout.cgi", ""),
            Exchange::new("GET /dashboard.cgi", LOGIN),
        ),
        6,
    );
    assert_eq!(d["ports"]["exit_code"], 5);
    assert_eq!(d["vlan_mode"]["data"]["mode"], "basic_802_1q");
    assert_eq!(d["vlan_list"]["exit_code"], 5);
    assert_eq!(d["cable_results"]["data"][0]["result"], "Normal");
    assert_eq!(d["logout_verification"]["status"], "ok");

    let mut denied = Exchange::new("GET /vlan.cgi", "private-response-marker");
    denied.status = 403;
    let d = diagnostic_output(
        diagnostic_session(
            DASH,
            denied,
            NO_CABLE_RESULTS,
            Exchange::new("POST /logout.cgi", ""),
            Exchange::new("GET /dashboard.cgi", LOGIN),
        ),
        6,
    );
    assert_eq!(d["vlan_mode"], d["vlan_list"]);
    assert_eq!(d["vlan_mode"]["exit_code"], 3);
    assert_eq!(d["cable_results"]["status"], "ok");
}

#[test]
fn diagnostics_uses_the_bound_cable_reader_and_keeps_independent_checks() {
    let page = include_str!("fixtures/cable-results-untested.html");
    let blank_result = page.replace(">Not test<", "><");
    for (stored, code) in [
        (page.to_owned(), 0),
        (blank_result.replace(">--<", "><"), 0),
        (blank_result, 6),
    ] {
        let report = diagnostic_output(
            diagnostic_session(
                DASH,
                Exchange::new("GET /vlan.cgi", VLAN),
                &stored,
                Exchange::new("POST /logout.cgi", ""),
                Exchange::new("GET /dashboard.cgi", LOGIN),
            ),
            code,
        );
        assert_eq!(report["ports"]["status"], "ok");
        assert_eq!(report["vlan_mode"]["status"], "ok");
        assert_eq!(report["logout_verification"]["status"], "ok");
        if code == 0 {
            assert_eq!(report["cable_results"]["data"], serde_json::json!([]));
        } else {
            assert_eq!(report["cable_results"]["exit_code"], 5);
            assert!(
                report["cable_results"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("distance placeholder")
            );
        }
    }
}

#[test]
fn logout_requires_observed_loss_of_dashboard_access() {
    for (after, expected) in [
        (DASH, "previous session still opened"),
        (
            "<html>private-response-marker</html>",
            "unrecognized dashboard response",
        ),
    ] {
        let d = diagnostic_output(
            diagnostic_session(
                DASH,
                Exchange::new("GET /vlan.cgi", VLAN),
                NO_CABLE_RESULTS,
                Exchange::new("POST /logout.cgi", LOGIN),
                Exchange::new("GET /dashboard.cgi", after),
            ),
            6,
        );
        assert_eq!(d["logout_request"]["status"], "ok");
        assert_eq!(d["logout_verification"]["exit_code"], 4);
        assert!(
            d["logout_verification"]["message"]
                .as_str()
                .unwrap()
                .contains(expected)
        );
    }
    let mut declined = Exchange::new("POST /logout.cgi", "");
    declined.status = 403;
    let d = diagnostic_output(
        diagnostic_session(
            DASH,
            Exchange::new("GET /vlan.cgi", VLAN),
            NO_CABLE_RESULTS,
            declined,
            Exchange::new("GET /dashboard.cgi", LOGIN),
        ),
        0,
    );
    assert_eq!(d["logout_request"]["status"], "error");
    assert_eq!(d["logout_verification"]["status"], "ok");
}

#[test]
fn diagnostics_requires_supported_model_before_feature_reads_and_rejects_apply_locally() {
    let mut steps = login();
    steps.extend([
        Exchange::new("GET /dashboard.cgi", DASH.replace("GS308Ev4", "Other")),
        Exchange::new("POST /logout.cgi", ""),
        Exchange::new("GET /dashboard.cgi", LOGIN),
    ]);
    fails(run(&["diagnostics", "--json"], steps), 5);
    let out = cli(&["--apply", "diagnostics"], false);
    assert!(String::from_utf8_lossy(&out.stderr).contains("does not accept --apply"));
    fails(out, 2);
}

#[test]
fn session_capacity_explains_recovery_without_a_retry() {
    let steps = vec![
        Exchange::new("GET /login.cgi", LOGIN),
        Exchange::new(
            "POST /login.cgi",
            "<p>Maximum sessions reached. Please wait.</p>",
        ),
    ];
    let out = run(&["info"], steps);
    assert!(String::from_utf8_lossy(&out.stderr).contains("HTTP session limit"));
    fails(out, 3);
}

fn session_cleanup() -> [Exchange; 2] {
    [
        Exchange::new("POST /logout.cgi", ""),
        Exchange::new("GET /dashboard.cgi", LOGIN),
    ]
}

fn session_json(output: &Output, code: i32) -> Vec<Value> {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("SID=") && !text.contains("mock-only!"));
    text.lines()
        .map(|l| {
            let value: Value = serde_json::from_str(l).unwrap();
            assert_eq!(value["schema_version"], 1);
            value
        })
        .collect()
}

#[test]
fn session_runs_selected_commands_in_order_with_one_cookie_and_verified_logout() {
    let mut steps = login();
    steps.extend([
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new("GET /vlan.cgi", VLAN),
        Exchange::new("GET /cableTester.cgi", NO_CABLE_RESULTS),
    ]);
    steps.extend(session_cleanup());
    let out = run(
        &[
            "--json",
            "session",
            "-c",
            "ports --summary",
            "-c",
            "vlan list",
            "-c",
            "maintenance cable-results",
        ],
        steps,
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reports = session_json(&out, 0);
    assert_eq!(
        reports
            .iter()
            .map(|v| v["command"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["ports-summary", "vlan-list", "cable-results", "session-end"]
    );
    assert_eq!(reports[0]["data"]["total"], 8);
    assert_eq!(reports[3]["data"]["logout_verification"]["status"], "ok");
}

#[test]
fn session_previews_both_settings_editors_without_a_configuration_post() {
    let token = "<input type=hidden name=hash id=hash value=synthetic-editor-value>";
    let power = format!(
        r#"<input type=checkbox id=powStateModSet><button name=submitPwsv onclick="submitPowerSave('#powStateModSet');">Apply</button>{token}"#
    );
    let leds = include_str!("fixtures/led-settings.html")
        .replace("type=checkbox", "type=checkbox checked")
        .replace("fixture-first", "synthetic-editor-value");
    let mut steps = login();
    steps.extend([
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new("GET /powerSaving.cgi", power),
        Exchange::new("GET /leds.cgi", leds),
    ]);
    steps.extend(session_cleanup());
    let out = run(
        &[
            "--json",
            "--protect-port",
            "2",
            "session",
            "-c",
            "settings set power-saving enabled=on",
            "-c",
            "settings set leds enabled=off",
        ],
        steps,
    );
    let reports = session_json(&out, 0);
    assert_eq!(reports.len(), 3);
    assert_eq!(reports[0]["data"]["outcome"], "preview");
    assert_eq!(reports[1]["data"]["outcome"], "preview");
    assert_eq!(reports[2]["data"]["logout_verification"]["status"], "ok");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("synthetic-editor-value"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("synthetic-editor-value"));
}

#[test]
fn file_session_uses_one_login_and_keeps_stdin_for_the_password() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("read reports.txt");
    std::fs::write(
        &path,
        "\u{feff}# reports\r\n\r\n'ports' --summary # summary\r\nvlan list\r\nexit\r\n  # end",
    )
    .unwrap();
    let mut steps = login();
    let mut next = Exchange::new("GET /vlan.cgi", VLAN);
    next.minimum_gap = Duration::from_millis(1900);
    steps.extend([Exchange::new("GET /dashboard.cgi", DASH), next]);
    steps.extend(session_cleanup());
    let out = run(
        &[
            "--json",
            "session",
            "--read-only",
            "-f",
            path.to_str().unwrap(),
        ],
        steps,
    );
    let reports = session_json(&out, 0);
    assert!(out.stderr.is_empty());
    assert_eq!(
        reports
            .iter()
            .map(|r| r["command"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["ports-summary", "vlan-list", "session-end"]
    );
    assert_eq!(reports[2]["data"]["logout_verification"]["status"], "ok");
}

#[test]
fn file_diagnostics_precede_host_and_password_and_report_physical_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("batch.txt");
    let path = file.to_str().unwrap();
    let args = ["session", "--read-only", "--file", path];
    let oversized = vec![b'x'; 65537];
    for (contents, expected) in [
        (
            b"info\n\n# blocked\nsettings set leds enabled=off\n".as_slice(),
            "command file line 4: read-only",
        ),
        (b"info\n\n'bad quote", "command file line 3: unterminated"),
        (
            b"# comment\nexit\n\ninfo",
            "command file line 2: logout must be the last",
        ),
        (b"\r\n  # empty", "file contains no commands"),
        (
            b"info\n\xff",
            "command file line 2: session commands must be UTF-8",
        ),
        (
            oversized.as_slice(),
            "command file line 1: session command exceeds 64 KiB",
        ),
    ] {
        std::fs::write(&file, contents).unwrap();
        let out = cli(&args, false);
        assert!(String::from_utf8_lossy(&out.stderr).contains(expected));
        fails(out, 2);
    }
    std::fs::remove_file(&file).unwrap();
    let missing = cli(&args, false);
    assert!(String::from_utf8_lossy(&missing.stderr).contains("could not open command file"));
    fails(missing, 2);
    let conflict = cli(&["session", "--file", path, "-c", "info"], false);
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("cannot be used with"));
    fails(conflict, 2);
}

#[test]
fn file_batch_preserves_a_preview_and_stops_at_a_protected_apply() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("changes.txt");
    std::fs::write(&file,"# preview\ncounters clear --all\n\n# requires an eligible scope\ncounters clear --all --apply\ninfo\n").unwrap();
    let mut steps = login();
    steps.extend([
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new(
            "GET /portStatistics.cgi",
            format!("<input type=hidden id=hash value=fixture>{STATS}"),
        ),
    ]);
    steps.extend(session_cleanup());
    let out = run(
        &[
            "--json",
            "--protect-port",
            "2",
            "session",
            "-f",
            file.to_str().unwrap(),
        ],
        steps,
    );
    let reports = session_json(&out, 2);
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["data"]["outcome"], "preview");
    assert_eq!(reports[1]["data"]["logout_verification"]["status"], "ok");
    assert!(String::from_utf8_lossy(&out.stderr).contains("command file line 5"));
}

#[test]
fn session_reads_lines_after_password_and_closes_on_exit_or_eof() {
    for ending in ["", "logout\ninfo\n"] {
        let mut steps = login();
        steps.extend([
            Exchange::new("GET /dashboard.cgi", DASH),
            Exchange::new(
                "GET /dashboard.cgi",
                DASH.replace("Example NAS", "Example updated"),
            ),
        ]);
        steps.extend(session_cleanup());
        let input = format!("mock-only!\nports --port '2'\nports --port 2\n{ending}");
        let out = run_input(&["--json", "session"], steps, input.as_bytes());
        assert!(out.stderr.is_empty());
        let reports = session_json(&out, 0);
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0]["data"][0]["name"], "Example NAS");
        assert_eq!(reports[1]["data"][0]["name"], "Example updated");
    }
}

#[test]
fn session_rejects_origin_changes_and_batch_wide_apply_before_login() {
    for (args, needle) in [
        (vec!["--apply", "session", "-c", "info"], "individual"),
        (
            vec![
                "session",
                "-c",
                "info",
                "-c",
                "--host https://other.invalid info",
            ],
            "session command 2",
        ),
        (
            vec!["session", "-c", "--protect-port 4 info"],
            "session command 1",
        ),
        (vec!["session", "-c", "ports 'unterminated"], "unterminated"),
        (vec!["session", "-c", "session"], "outside an open session"),
    ] {
        let out = cli(&args, false);
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(needle),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        fails(out, 2);
    }
}

#[test]
fn session_preserves_per_command_preview_and_protection() {
    let mut steps = login();
    let stats = format!("<input type=hidden id=hash value=fixture>{STATS}");
    steps.extend([
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new("GET /portStatistics.cgi", stats),
    ]);
    steps.extend(session_cleanup());
    let out = run(
        &[
            "--json",
            "--protect-port",
            "5",
            "session",
            "-c",
            "counters clear --all",
            "-c",
            "counters clear --all --apply",
            "-c",
            "info",
        ],
        steps,
    );
    let reports = session_json(&out, 2);
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["data"]["outcome"], "preview");
    assert!(String::from_utf8_lossy(&out.stderr).contains("session command 2"));
}

#[test]
fn session_stops_after_expiry_or_capacity_without_a_new_sign_in() {
    let mut steps = login();
    steps.extend([
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new("GET /vlan.cgi", LOGIN),
    ]);
    steps.extend(session_cleanup());
    let out = run(
        &[
            "--json",
            "session",
            "-c",
            "ports --summary",
            "-c",
            "vlan list",
            "-c",
            "info",
        ],
        steps,
    );
    let reports = session_json(&out, 3);
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[1]["command"], "session-end");
    assert!(String::from_utf8_lossy(&out.stderr).contains("GET /vlan.cgi"));
    for (body, message) in [
        (LOGIN, "without a usable SID"),
        (
            "<p>Maximum sessions reached. Please wait.</p>",
            "HTTP session limit",
        ),
    ] {
        let out = run(
            &["session", "-c", "vlan"],
            vec![
                Exchange::new("GET /login.cgi", LOGIN),
                Exchange::new("POST /login.cgi", body),
            ],
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains(message));
        fails(out, 3);
    }
}

#[test]
fn session_reports_unconfirmed_logout_independently_from_completed_commands() {
    let mut steps = login();
    steps.extend([
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new("POST /logout.cgi", ""),
        Exchange::new("GET /dashboard.cgi", DASH),
    ]);
    let out = run(&["--json", "session", "-c", "ports --summary"], steps);
    let reports = session_json(&out, 4);
    assert_eq!(reports[0]["command"], "ports-summary");
    assert_eq!(reports[1]["data"]["logout_request"]["status"], "ok");
    assert_eq!(reports[1]["data"]["logout_verification"]["status"], "error");
}

#[test]
fn session_accepts_a_lost_logout_reply_only_with_independent_confirmation() {
    let mut steps = login();
    let mut lost = Exchange::new("POST /logout.cgi", "");
    lost.status = 0;
    steps.extend([
        Exchange::new("GET /dashboard.cgi", DASH),
        lost,
        Exchange::new("GET /dashboard.cgi", LOGIN),
    ]);
    let out = run(&["--json", "session", "-c", "ports --summary"], steps);
    let reports = session_json(&out, 0);
    let end = &reports[1]["data"];
    assert!(
        end["logout_request"]["message"]
            .as_str()
            .unwrap()
            .contains("POST /logout.cgi")
    );
    assert_eq!(end["logout_verification"]["status"], "ok");
}

#[test]
fn session_never_replays_an_unconfirmed_submission() {
    let mut steps = login();
    let stats = format!("<input type=hidden id=hash value=fixture>{STATS}");
    let mut lost = Exchange::new("POST /portStatistics.cgi", "");
    lost.request_body = "clearCounters=1&hash=fixture";
    lost.status = 0;
    steps.extend([
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new("GET /portStatistics.cgi", stats),
        lost,
    ]);
    steps.extend(session_cleanup());
    let out = run(
        &[
            "--json",
            "session",
            "-c",
            "--apply counters clear --all",
            "-c",
            "info",
        ],
        steps,
    );
    let reports = session_json(&out, 4);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["command"], "session-end");
    assert!(String::from_utf8_lossy(&out.stderr).contains("inspect current state before retrying"));
}

#[cfg(unix)]
#[test]
fn interrupting_an_idle_session_verifies_logout() {
    use std::io::{BufRead, BufReader, Read};
    let mut steps = login();
    steps.push(Exchange::new("GET /dashboard.cgi", DASH));
    steps.extend(session_cleanup());
    let out = run_with(&["--json", "session"], steps, |args| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_gs308e"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"mock-only!\nports --summary\n")
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut text = String::new();
        stdout.read_line(&mut text).unwrap();
        assert!(text.contains("ports-summary"));
        assert!(
            Command::new("kill")
                .args(["-INT", &child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        stdout.read_to_string(&mut text).unwrap();
        let mut output = child.wait_with_output().unwrap();
        output.stdout = text.into_bytes();
        output
    });
    let reports = session_json(&out, 130);
    assert_eq!(reports[1]["data"]["logout_verification"]["status"], "ok");
}

#[test]
fn public_cli_inventory_and_unsupported_operations() {
    let help = cli(&["--help"], false);
    assert!(help.status.success());
    let output = String::from_utf8(help.stdout).unwrap();
    let commands = output
        .split_once("Commands:")
        .unwrap()
        .1
        .split_once("Options:")
        .unwrap()
        .0;
    let listed: std::collections::BTreeSet<_> = commands
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(
        listed,
        [
            "info",
            "firmware",
            "ports",
            "counters",
            "snapshot",
            "diagnostics",
            "session",
            "settings",
            "maintenance",
            "time",
            "vlan",
            "completions",
            "help"
        ]
        .into()
    );
    for args in [
        &["ports", "set"][..],
        &["vlan", "apply"],
        &["counters", "clear"],
        &["reboot"],
        &["restore"],
    ] {
        fails(cli(args, false), 2);
    }
}

#[test]
fn firmware_profiles_come_from_a_unique_dashboard_version_without_identifiers() {
    for (raw, version, profile) in [
        ("V1.0.1.13", Some("V1.0.1.13"), "reviewed-ui"),
        ("1.0.1.100", Some("V1.0.1.100"), "unverified"),
    ] {
        let dashboard = DASH
            .replace("V1.0.1.13", raw)
            .replace("ml678", "absent-mac");
        let d = json(view(&["--json", "firmware"], &dashboard, None));
        assert_eq!(d["data"]["version"].as_str(), version);
        assert_eq!(d["data"]["read_profile"], profile);
        assert!(d["data"].get("mac_address").is_none());
    }
    for raw in ["V1.0.1.12", "V1.0.1.13-beta", "unidentified"] {
        let dashboard = DASH.replace("V1.0.1.13", raw);
        let out = view(&["--json", "firmware"], &dashboard, None);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("V1.0.1.13"), "{stderr}");
        if raw == "V1.0.1.12" {
            assert!(stderr.contains("V1.0.1.12"), "{stderr}");
        } else {
            assert!(!stderr.contains(raw), "{stderr}");
        }
        fails(out, 5);
        fails(view(&["--json", "snapshot"], &dashboard, None), 5);
    }
}

#[test]
fn write_gate_covers_all_command_families_and_newer_unmatched_forms() {
    let commands: &[&[&str]] = &[
        &["settings", "set", "power-saving", "enabled=on"],
        &["settings", "set", "leds", "enabled=off"],
        &["settings", "set", "identity", "name=Fixture"],
        &[
            "vlan",
            "create",
            "20",
            "--name",
            "Fixture",
            "--untagged",
            "2",
        ],
        &[
            "time",
            "set-clock",
            "manual",
            "--date",
            "2026-06-01",
            "--time",
            "12:00:00",
        ],
        &["counters", "clear", "--all"],
        &["maintenance", "cable-test", "2"],
        &["maintenance", "reboot", "--accept-outage"],
        &["maintenance", "upgrade", "absent.bin", "--accept-outage"],
    ];
    for args in commands {
        let mut a = vec!["--apply"];
        a.extend_from_slice(args);
        for (raw, code) in [("V1.0.1.12", 5), ("V1.0.1.+13", 5), ("V1.0.1.14", 2)] {
            let out = view(&a, &DASH.replace("V1.0.1.13", raw), None);
            assert_eq!(out.status.code(), Some(code));
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(err.contains("firmware"), "{err}");
        }
    }
    let duplicate = DASH.replace("ml678", "ml089");
    let mut seq = login();
    seq.push(Exchange::new("GET /dashboard.cgi", duplicate));
    seq.extend(session_cleanup());
    let out = run(
        &["--json", "session", "-c", "--apply counters clear --all"],
        seq,
    );
    assert_eq!(out.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unsupported switch firmware"));
}

#[test]
fn passive_sessions_wait_after_completion_in_batch_and_line_mode() {
    for batch in [true, false] {
        let mut seq = login();
        let mut first = Exchange::new("GET /dashboard.cgi", DASH);
        first.response_delay = Duration::from_millis(300);
        let mut next = Exchange::new("GET /dashboard.cgi", DASH);
        next.minimum_gap = Duration::from_millis(1950);
        seq.extend([first, next]);
        seq.extend(session_cleanup());
        let mut args = vec!["--json", "session", "--read-only"];
        let input: &[u8] = if batch {
            args.extend(["-c", "firmware", "-c", "ports --summary"]);
            b"mock-only!\n"
        } else {
            b"mock-only!\nfirmware\nports --summary\n"
        };
        let reports = session_json(&run_input(&args, seq, input), 0);
        assert_eq!(reports[0]["data"]["version"], "V1.0.1.13");
        assert_eq!(reports[1]["command"], "ports-summary");
    }
}

#[test]
fn passive_batch_refuses_mutations_or_previews_before_sign_in() {
    for cmd in [
        "counters clear --all",
        "settings set leds enabled=off --apply",
        "ports --apply",
    ] {
        let out = cli(
            &["session", "--read-only", "-c", "firmware", "-c", cmd],
            false,
        );
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("passive reports only"));
    }
}

#[test]
fn changed_binding_stops_before_the_next_feature_request() {
    let mut seq = login();
    seq.extend([
        Exchange::new("GET /dashboard.cgi", DASH),
        Exchange::new("GET /dashboard.cgi", DASH.replace("V1.0.1.13", "V1.0.1.14")),
    ]);
    seq.extend(session_cleanup());
    let out = run(
        &[
            "--json",
            "session",
            "--read-only",
            "-c",
            "firmware",
            "-c",
            "info",
        ],
        seq,
    );
    assert_eq!(session_json(&out, 5).len(), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("firmware changed"));
}
