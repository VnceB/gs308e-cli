use std::{
    collections::BTreeMap,
    io::Write,
    process::{Command, Output, Stdio},
    thread,
    time::Duration,
};
use tiny_http::{Header, Response, Server};

const DASH: &str = include_str!("fixtures/dashboard.html");
const LOGIN: &str = include_str!("fixtures/login.html");
const MODES: &str = include_str!("fixtures/vlan.html");
const STATS: &str = include_str!("fixtures/statistics.html");
const VLAN: &str = "/vlan.cgi";
const TOKEN: &str = "<input type=hidden id=hash value=fixture-marker>";
struct Exchange {
    method: &'static str,
    path: &'static str,
    page: String,
    form: Option<BTreeMap<String, String>>,
}
fn get(path: &'static str, page: impl Into<String>) -> Exchange {
    Exchange {
        method: "GET",
        path,
        page: page.into(),
        form: None,
    }
}
fn post(path: &'static str, fields: &[(&str, &str)]) -> Exchange {
    let mut form: BTreeMap<String, String> = fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    form.insert("hash".into(), "fixture-marker".into());
    Exchange {
        method: "POST",
        path,
        page: if matches!(
            path,
            "/vlanMod.cgi"
                | "/portBasedBasic.cgi"
                | "/portBasedAdvanced.cgi"
                | "/8021qBasic.cgi"
                | "/8021qAdvanced.cgi"
                | "/portPVID.cgi"
                | "/OUITable.cgi"
        ) {
            "<div>fixture acknowledgement</div>"
        } else {
            "SUCCESS"
        }
        .into(),
        form: Some(form),
    }
}
fn run(args: &[&str], steps: Vec<Exchange>) -> Output {
    mock(args, steps, true, DASH)
}
fn mock(args: &[&str], mut steps: Vec<Exchange>, json: bool, dashboard: &str) -> Output {
    let dashboard = dashboard.to_owned();
    let server = Server::http("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", server.server_addr());
    let mut sequence = vec![
        get("/login.cgi", LOGIN),
        Exchange {
            method: "POST",
            path: "/login.cgi",
            page: "".into(),
            form: None,
        },
        get("/dashboard.cgi", dashboard),
    ];
    sequence.append(&mut steps);
    sequence.push(Exchange {
        method: "POST",
        path: "/logout.cgi",
        page: "".into(),
        form: None,
    });
    let worker = thread::spawn(move || {
        for step in sequence {
            let mut req = loop {
                let req = server
                    .recv_timeout(Duration::from_secs(4))
                    .unwrap()
                    .expect("missing expected request");
                // Supply the switch's independent dependency pages when no override was scripted.
                if req.method().as_str() == "GET" && req.url() != step.path {
                    let page = match req.url() {
                        "/lag.cgi" => lag_page("00000000", false, false),
                        "/multicast.cgi" => multicast_page(false, None),
                        _ => break req,
                    };
                    req.respond(Response::from_string(page)).unwrap();
                    continue;
                }
                break req;
            };
            assert_eq!((req.method().as_str(), req.url()), (step.method, step.path));
            if req.url() != "/login.cgi" {
                assert!(
                    req.headers()
                        .iter()
                        .any(|h| h.field.equiv("Cookie")
                            && h.value.as_str() == "SID=fixture-session")
                );
            }
            let mut data = String::new();
            req.as_reader().read_to_string(&mut data).unwrap();
            if let Some(f) = step.form {
                let url = reqwest::Url::parse(&format!("http://example.invalid/?{data}")).unwrap();
                assert_eq!(url.query_pairs().count(), f.len());
                assert_eq!(
                    url.query_pairs()
                        .map(|(k, v)| (k.into_owned(), v.into_owned()))
                        .collect::<BTreeMap<_, _>>(),
                    f
                );
            }
            let attachment = req.url() == "/confFile.cgi?cmd=backup_conf"
                && step.page == "synthetic configuration";
            let mut reply = Response::from_string(step.page);
            if attachment {
                reply = reply.with_header(
                    Header::from_bytes("Content-Disposition", "attachment; filename=example.cfg")
                        .unwrap(),
                );
            }
            if req.url() == "/login.cgi" && step.method == "POST" {
                reply = reply.with_header(
                    Header::from_bytes("Set-Cookie", "SID=fixture-session; Path=/").unwrap(),
                )
            }
            req.respond(reply).unwrap();
        }
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_gs308e"))
        .env_remove("GS308E_PROTECT_PORTS")
        .args(["--host", &origin, "--password-stdin"])
        .args(json.then_some("--json"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"fixture-only\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    worker.join().unwrap();
    out
}
fn data(out: Output) -> serde_json::Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
    v["data"].clone()
}
fn rejected(out: Output) {
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
}
fn mode(n: u8, inside: &str) -> String {
    let name = ["noVlan", "bscPotBsd", "advPotBsd", "bsc8021Q", "adv8021Q"][usize::from(n)];
    format!(
        "{}{TOKEN}<section vlan-page='{name}'>{inside}</section>",
        MODES.replace("value=\"0\"", &format!("value=\"{n}\""))
    )
}
fn assignments(dot: bool, first: &str) -> String {
    (1..=8).map(|p|{let v=if p==1{first}else{"1"};
        if dot{format!("<li name-3=port{p} pot-mod='{}'><div vid-wrap=Access><span vid-str-3>{v} - Example</span></div><div vid-wrap=Trunk><input vid-str-3 real-val=ALL></div></li>",if v=="ALL"{"Trunk"}else{"Access"})}
        else{format!("<li name-1=port{p}><span vid-1>{v}</span></li>")}
    }).collect()
}
fn row(n: u8, id: u16, name: &str, mask: &str) -> String {
    format!(
        "<li><span vid-{n}>{id}</span><span vnm-{n}>{name}</span><input hidden-mem value='{mask}'></li>"
    )
}
fn advq(mask: Option<&str>, pvid: u8, voice: u8, cos: u8, oui: bool) -> String {
    let mut rows = row(4, 1, "Default", "22222222");
    if let Some(mask) = mask {
        rows += &row(4, 7, "Lab &amp; Bench", mask);
    }
    let pvids: String = (1..=8)
        .map(|p| {
            format!(
                "<li><span class=pot-num>{p}</span><span pvid-str>{}*, </span></li>",
                if p == 2 { pvid } else { 1 }
            )
        })
        .collect();
    mode(
        4,
        &format!(
            "<ul bind=btns-4>{rows}</ul><input id=vlanNum value={}><input name=mgmtVlan value=1><input id=voVid value={voice}><span id=cos>{cos}</span><ul bind=pvid-tab>{pvids}</ul><ul bind=btns-oui-4>{}</ul>",
            if mask.is_some() { 2 } else { 1 },
            if oui {
                "<li oui-num='INDEX_0'><span tele>02:00:01</span><span desc>Bench</span></li>"
            } else {
                ""
            }
        ),
    )
}
fn lag_page(mask: &str, a: bool, b: bool) -> String {
    let mut html = format!("{TOKEN}<input id=hiddenMem value='{mask}'>");
    for (n, enabled, start) in [(1, a, 1), (2, b, 5)] {
        html += &format!(
            "<input type=checkbox id=lagStatus{n} {}><ul id=lag{n}>",
            if enabled { "checked" } else { "" }
        );
        for p in start..start + 4 {
            html += &format!("<li><span class=port_circle_num>{p}</span></li>");
        }
        html += "</ul>";
    }
    html
}
fn multicast_page(on: bool, id: Option<u16>) -> String {
    format!(
        "{TOKEN}<input type=checkbox id=igmp_status {}><input type=checkbox id=mul_addr><input type=checkbox id=ip_header><input id=igmpSnoopingVlanId {} value={}><div class=dropdown><span seld-val=rtpt>any</span><ul><li value=0><a>any</a></li></ul></div>",
        if on { "checked" } else { "" },
        if id.is_none() { "disabled" } else { "" },
        id.unwrap_or(1)
    )
}
fn apply(
    args: &[&str],
    path: &'static str,
    before: &str,
    req: Exchange,
    after: &str,
) -> serde_json::Value {
    let mut a = vec!["--apply"];
    a.extend(args);
    data(run(
        &a,
        vec![get(path, before), get(path, before), req, get(path, after)],
    ))
}
#[test]
fn mode_change_is_explicit_and_verified_from_a_separate_read() {
    let start = mode(0, "");
    let finish = mode(1, &assignments(false, "1"));
    let opts = [
        "vlan",
        "set-mode",
        "basic-port",
        "--from",
        "disabled",
        "--accept-vlan-reset",
    ];
    assert_eq!(
        data(run(&opts, vec![get(VLAN, &start)]))["outcome"],
        "preview"
    );
    assert_eq!(
        apply(
            &opts,
            VLAN,
            &start,
            post("/vlanMod.cgi", &[("VLAN_MOD", "1")]),
            &finish
        )["outcome"],
        "verified"
    );
    rejected(run(
        &[
            "--apply",
            "vlan",
            "set-mode",
            "basic-port",
            "--from",
            "advanced-port",
            "--accept-vlan-reset",
        ],
        vec![get(VLAN, start)],
    ));
}
#[test]
fn basic_port_assignment_sends_the_complete_port_table() {
    let before = mode(1, &assignments(false, "1"));
    let after = mode(1, &assignments(false, "All"));
    let mut req = post("/portBasedBasic.cgi", &[]);
    for p in 1..=8 {
        req.form
            .as_mut()
            .unwrap()
            .insert(format!("port{p}"), if p == 1 { "ALL" } else { "1" }.into());
    }
    assert_eq!(
        apply(&["vlan", "assign", "1", "all"], VLAN, &before, req, &after)["outcome"],
        "verified"
    );
}
#[test]
fn advanced_port_create_and_delete_have_distinct_forms() {
    let base = row(2, 1, "Default", "11111111");
    let both = format!("{base}{}", row(2, 3, "Bench", "10000001"));
    let page = |s: &str| mode(2, &format!("<ul bind=btns-2>{s}</ul>"));
    let before = page(&base);
    let after = page(&both);
    apply(
        &["vlan", "create", "3", "--name", "Bench", "--ports", "1,8"],
        VLAN,
        &before,
        post(
            "/portBasedAdvanced.cgi",
            &[
                ("VLAN_ID", "3"),
                ("VLAN_NAME", "Bench"),
                ("hiddenMem", "10000001"),
            ],
        ),
        &after,
    );
    apply(
        &["vlan", "delete", "3"],
        VLAN,
        &after,
        post(
            "/portBasedAdvanced.cgi",
            &[("VLAN_ID", "3"), ("hiddenMem", "00000000")],
        ),
        &before,
    );
}
#[test]
fn basic_dot1q_catalog_and_trunks_use_distinct_workflows() {
    let page = |extra: bool, first: &str| {
        mode(
            3,
            &format!(
                "{}<ul bind=btns-3>{}{}</ul><input id=vlanNum value={}>",
                assignments(true, first),
                row(3, 1, "Default", ""),
                if extra {
                    row(3, 22, "Lab", "")
                } else {
                    String::new()
                },
                if extra { 2 } else { 1 }
            ),
        )
    };
    let before = page(false, "1");
    let after = page(true, "1");
    apply(
        &["vlan", "create", "22", "--name", "Lab"],
        VLAN,
        &before,
        post(
            "/8021qBasic.cgi",
            &[
                ("page", "vlanCfg"),
                ("Action", "Apply"),
                ("vlanNum", "1"),
                ("VLAN_ID", "22"),
                ("VLAN_NAME", "Lab"),
            ],
        ),
        &after,
    );
    let mut req = post("/8021qBasic.cgi", &[("page", "portCfg")]);
    for p in 1..=8 {
        req.form
            .as_mut()
            .unwrap()
            .insert(format!("port{p}"), if p == 1 { "ALL" } else { "1" }.into());
    }
    apply(
        &["vlan", "assign", "1", "all"],
        VLAN,
        &after,
        req,
        &page(true, "ALL"),
    );
    apply(
        &["vlan", "delete", "22"],
        VLAN,
        &after,
        post(
            "/8021qBasic.cgi",
            &[("page", "vlanCfg"), ("Action", "Delete"), ("VLAN_ID", "22")],
        ),
        &before,
    );
}
#[test]
fn advanced_dot1q_json_membership_voice_and_pvid_round_trip() {
    let a = advq(None, 1, 0, 6, false);
    let b = advq(Some("12333333"), 1, 0, 6, false);
    let form = |action: &str, mask: &str, voice: &str, cos: &str, count: &str| {
        post(
            "/8021qAdvanced.cgi",
            &[
                ("ACTION", action),
                ("VLAN_ID", "7"),
                ("VLAN_NAME", "Lab & Bench"),
                ("vlanNum", count),
                ("hiddenMem", mask),
                ("voiceVLANID", voice),
                ("voiceVlanCos", cos),
            ],
        )
    };
    apply(
        &[
            "vlan",
            "create",
            "7",
            "--name",
            "Lab & Bench",
            "--tagged",
            "1",
            "--untagged",
            "2",
        ],
        VLAN,
        &a,
        form("Add", "12333333", "0", "6", "1"),
        &b,
    );
    let c = advq(Some("22333333"), 1, 0, 6, false);
    apply(
        &["vlan", "members", "7", "--untagged", "1,2"],
        VLAN,
        &b,
        form("Apply", "22333333", "0", "6", "2"),
        &c,
    );
    let d = advq(Some("22333333"), 7, 0, 6, false);
    apply(
        &["vlan", "pvid", "2", "7"],
        VLAN,
        &c,
        post("/portPVID.cgi", &[("PORT", "2"), ("PVID", "7")]),
        &d,
    );
    let e = advq(Some("22333333"), 7, 7, 5, false);
    apply(
        &["vlan", "voice", "7", "--cos", "5"],
        VLAN,
        &d,
        form("Apply", "22333333", "1", "5", "2"),
        &e,
    );
    let v = data(run(&["vlan", "list"], vec![get(VLAN, e)]));
    assert_eq!(v["pvids"]["2"], 7);
    assert_eq!(v["vlans"]["7"]["members"]["1"], "untagged");
    assert_eq!(v["voice_vlan"], 7);
}
#[test]
fn voice_oui_operations_send_only_the_selected_entry() {
    let a = advq(None, 1, 0, 6, false);
    let b = advq(None, 1, 0, 6, true);
    for (verb, action, before, after) in
        [("oui-add", "Add", &a, &b), ("oui-delete", "Delete", &b, &a)]
    {
        let mut args = vec!["vlan", verb, "02:00:01"];
        if action == "Add" {
            args.push("Bench");
        }
        apply(
            &args,
            VLAN,
            before,
            post(
                "/OUITable.cgi",
                &[
                    ("ACTION", action),
                    ("TELEPHONY", "02:00:01"),
                    ("DESCRIPTION", "Bench"),
                ],
            ),
            after,
        );
    }
}
#[test]
fn unsafe_or_ambiguous_vlan_requests_never_post() {
    let p = advq(Some("22333333"), 7, 7, 6, false);
    for args in [
        vec!["vlan", "delete", "1"],
        vec!["vlan", "delete", "7"],
        vec!["vlan", "pvid", "8", "7"],
        vec!["vlan", "members", "7", "--tagged", "1"],
        vec!["vlan", "members", "7", "--tagged", "1,2", "--untagged", "2"],
        vec!["vlan", "assign", "1", "all"],
        vec!["vlan", "create", "4095", "--tagged", "1"],
    ] {
        let mut a = vec!["--apply"];
        a.extend(args);
        rejected(run(&a, vec![get(VLAN, &p)]));
    }
    rejected(run(
        &["--protect-port", "2", "--apply", "vlan", "pvid", "2", "1"],
        vec![],
    ));
    rejected(run(
        &["--apply", "vlan", "pvid", "2", "1"],
        vec![get(VLAN, p.replace(TOKEN, ""))],
    ));
}
#[test]
fn stale_or_unconfirmed_writes_never_retry() {
    let a = mode(1, &assignments(false, "1"));
    let b = mode(1, &assignments(false, "2"));
    let opts = ["--apply", "vlan", "assign", "1", "3"];
    rejected(run(&opts, vec![get(VLAN, &a), get(VLAN, &b)]));
    let mut req = post("/portBasedBasic.cgi", &[]);
    for p in 1..=8 {
        req.form
            .as_mut()
            .unwrap()
            .insert(format!("port{p}"), if p == 1 { "3" } else { "1" }.into());
    }
    let out = run(
        &opts,
        vec![get(VLAN, &a), get(VLAN, &a), req, get(VLAN, &b)],
    );
    assert_eq!(out.status.code(), Some(4));
}
#[test]
fn loop_and_dos_controls_use_independent_readback() {
    for (feature, path, sel, field) in [
        (
            "loop-prevention",
            "/loopDetection.cgi",
            "loopPrevMod",
            "loopDetection",
        ),
        ("dos-prevention", "/dos.cgi", "dosState", "DOS"),
    ] {
        let start = format!("{TOKEN}<input id={sel} type=checkbox>");
        let end = start.replace("type=checkbox", "type=checkbox checked");
        assert_eq!(
            apply(
                &["settings", "set", feature, "enabled=on"],
                path,
                &start,
                post(path, &[(field, "1")]),
                &end
            )["outcome"],
            "verified"
        );
    }
}
#[test]
fn power_and_led_settings_use_the_native_editor_and_its_fresh_token() {
    const FORM_TOKEN: &str = "<input type=hidden name=hash id=hash value=fixture-first>";
    for (feature, editor, destination, checkbox, button, action, field, on, off) in [
        (
            "power-saving",
            "/powerSaving.cgi",
            "/green_ethernet.cgi",
            "powStateModSet",
            "submitPwsv",
            "submitPowerSave('#powStateModSet');",
            "powerSavingMode",
            "1",
            "0",
        ),
        (
            "leds",
            "/leds.cgi",
            "/port_led.cgi",
            "ledSetting",
            "submitLed2",
            "submitLED()",
            "portled",
            "0",
            "2",
        ),
    ] {
        let start = if feature == "leds" {
            include_str!("fixtures/led-settings.html").to_owned()
        } else {
            format!(
                "<input id={checkbox} type=checkbox><button name={button} onclick=\x22{action}\x22>Apply</button>{FORM_TOKEN}"
            )
        };
        let end = start.replace("type=checkbox", "type=checkbox checked");
        for (from, to, requested, expected) in [
            (&start, &end, "enabled=on", on),
            (&end, &start, "enabled=off", off),
        ] {
            let args = ["settings", "set", feature, requested];
            let preview = data(run(&args, vec![get(editor, from)]));
            assert_eq!(preview["outcome"], "preview");
            let mut submission = post(destination, &[(field, expected)]);
            submission
                .form
                .as_mut()
                .unwrap()
                .insert("hash".into(), "fixture-fresh".into());
            let result = run(
                &["--apply", "settings", "set", feature, requested],
                vec![
                    get(editor, from),
                    get(editor, from.replace("fixture-first", "fixture-fresh")),
                    submission,
                    get(editor, to),
                ],
            );
            assert_eq!(data(result)["outcome"], "verified");
        }
        let opts = ["--apply", "settings", "set", feature, "enabled=on"];
        for (broken, reason) in [
            (
                start.replace(FORM_TOKEN, ""),
                "one page-local hidden form token",
            ),
            (
                start.replace(FORM_TOKEN, &FORM_TOKEN.repeat(2)),
                "one page-local hidden form token",
            ),
            (
                start.replace("type=hidden", "type=text"),
                "form-token control",
            ),
            (
                start.replace("value=fixture-first", "value=''"),
                "form-token value",
            ),
            (
                start.replace(action, "differentHandler()"),
                "editor Apply control",
            ),
            (
                start.replace(&format!("name={button}"), "name=other"),
                "one editor Apply button",
            ),
        ] {
            for steps in [
                vec![get(editor, &broken)],
                vec![get(editor, &start), get(editor, &broken)],
            ] {
                let out = run(&opts, steps);
                assert_eq!(out.status.code(), Some(5));
                assert!(out.stdout.is_empty());
                let err = String::from_utf8_lossy(&out.stderr);
                assert!(err.contains(reason), "{err}");
                assert!(!err.contains("fixture-first"));
            }
        }
        let stale = run(&opts, vec![get(editor, &start), get(editor, &end)]);
        assert!(
            String::from_utf8_lossy(&stale.stderr)
                .contains("configuration changed during preflight")
        );
    }
}

#[test]
fn disabled_led_apply_requires_the_reviewed_edit_handler_and_button_scope() {
    let led = include_str!("fixtures/led-settings.html");
    let args = ["--apply", "settings", "set", "leds", "enabled=on"];
    for (broken, reason) in [
        (
            led.replace("enableButtons()", "other()"),
            "disabled Apply button",
        ),
        (
            led.replace("type=checkbox", "type=checkbox disabled"),
            "enabled Settings editor checkbox",
        ),
        (
            led.replace("submitLED()", "differentHandler()"),
            "unrecognized editor Apply control",
        ),
        (
            format!("<span id=settingsStatusContainer></span>{led}"),
            "disabled Apply button",
        ),
        (
            led.replace("settingsStatusContainer", "otherContainer"),
            "disabled Apply button",
        ),
        (
            led.replace("class=checkbox", "class=other"),
            "disabled Apply button",
        ),
        (
            led.replace("class=submit_btn", "class=other"),
            "disabled Apply button",
        ),
        (
            led.replace("button_theme_flat", "other"),
            "disabled Apply button",
        ),
        (
            led.replace("disabled=''", "disabled='restricted'"),
            "disabled Apply button",
        ),
        (
            format!("<fieldset disabled>{led}</fieldset>"),
            "disabled Apply button",
        ),
        (
            led.replace("class=submit_btn", "class=submit_btn inert"),
            "disabled Apply button",
        ),
        (
            led.replace("type=checkbox", "type=checkbox aria-disabled=true"),
            "disabled Apply button",
        ),
    ] {
        for steps in [
            vec![get("/leds.cgi", &broken)],
            vec![get("/leds.cgi", led), get("/leds.cgi", &broken)],
        ] {
            let out = run(&args, steps);
            assert_eq!(out.status.code(), Some(5));
            assert!(out.stdout.is_empty());
            assert!(String::from_utf8_lossy(&out.stderr).contains(reason));
        }
    }
    let power = led
        .replace("ledSetting", "powStateModSet")
        .replace("submitLed2", "submitPwsv")
        .replace("submitLED()", "submitPowerSave('#powStateModSet')");
    let out = run(
        &["--apply", "settings", "set", "power-saving", "enabled=on"],
        vec![get("/powerSaving.cgi", power)],
    );
    assert_eq!(out.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("disabled Apply button"));
}

#[test]
fn power_saving_quick_panel_is_readable_but_cannot_replace_the_settings_editor() {
    let quick = "<input type=checkbox id=powStateMod>";
    let read = run(
        &["settings", "show", "power-saving"],
        vec![get("/green_ethernet.cgi", quick)],
    );
    assert_eq!(data(read)["values"]["enabled"], "off");
    let preview = run(
        &["settings", "set", "power-saving", "enabled=on"],
        vec![get("/powerSaving.cgi", quick)],
    );
    assert_eq!(preview.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&preview.stderr).contains("Settings editor checkbox"));
}

#[test]
fn clearing_statistics_needs_global_scope_and_verifies_actual_rows() {
    let before = format!("{TOKEN}{STATS}");
    let after = STATS
        .lines()
        .map(|line| {
            if line.contains("type=\"hidden\"") {
                line.split("value=\"")
                    .enumerate()
                    .map(|(i, x)| {
                        if i == 0 {
                            x.into()
                        } else {
                            format!("value=\"0\"{}", x.split_once('"').unwrap().1)
                        }
                    })
                    .collect::<String>()
            } else {
                line.to_string()
            }
        })
        .collect::<String>()
        + TOKEN;
    assert_eq!(
        data(run(
            &["counters", "clear", "--all", "--apply"],
            vec![
                get("/portStatistics.cgi", before),
                post("/portStatistics.cgi", &[("clearCounters", "1")]),
                get("/portStatistics.cgi", after)
            ]
        ))["outcome"],
        "verified"
    );
    rejected(run(
        &["counters", "--port", "3", "clear", "--all", "--apply"],
        vec![],
    ));
}

fn dashboard_config() -> String {
    let controls = "<input class=Speed value=6><input class=flowCtr value=2><input class=priority value=2><input class=ingressRate value=2><input class=egressRate value=1>";
    format!(
        "{TOKEN}{}<ul class=IngressList><li><a>No limit</a></li><li><a>8 Mbps</a></li></ul><ul class=EgressList><li><a>No limit</a></li><li><a>8 Mbps</a></li></ul>",
        DASH.replace(
            "<div class=\"port_info\">",
            &format!("<div class=\"port_info\">{controls}")
        )
    )
}
#[test]
fn port_settings_show_reuses_dashboard_and_names_unavailable_controls() {
    let args = ["settings", "show", "port", "--port", "2"];
    let cfg = dashboard_config();
    let complete = data(mock(&args, vec![], true, &cfg));
    assert_eq!(complete["values"]["speed"], "100-full");
    assert!(complete.get("unavailable").is_none());

    let partial = cfg
        .replace("<input class=priority value=2>", "")
        .replace("class=IngressList", "class=absent");
    let shown = data(mock(&args, vec![], true, &partial));
    assert_eq!(shown["values"]["name"], "Example NAS");
    assert_eq!(shown["values"]["flow"], "off");
    assert_eq!(shown["values"]["egress"], "No limit");
    assert!(shown["values"].get("priority").is_none());
    assert!(shown["values"].get("ingress").is_none());
    assert_eq!(
        shown["unavailable"],
        serde_json::json!({
            "priority": "control is absent", "ingress": "rate menu is absent"
        })
    );
    let human = mock(&args, vec![], false, &partial);
    assert!(human.status.success());
    assert!(
        String::from_utf8_lossy(&human.stdout)
            .lines()
            .any(|s| s.starts_with("priority") && s.contains("unavailable"))
    );
    let out = run(
        &[
            "--apply",
            "settings",
            "set",
            "port",
            "--port",
            "2",
            "name=Example",
        ],
        vec![get("/dashboard.cgi", &partial)],
    );
    assert_eq!(out.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("port editor field priority"));
    assert!(out.stdout.is_empty());
}

#[test]
fn port_settings_preserve_visible_identity_and_reject_contradictory_controls() {
    let args = ["settings", "show", "port", "--port", "2"];
    let id = r#"<input class="port" value="2">"#;
    let result = data(mock(&args, vec![], true, &DASH.replace(id, "")));
    assert_eq!(result["values"], serde_json::json!({"name": "Example NAS"}));
    assert_eq!(result["unavailable"]["editor-port"], "control is absent");
    for field in ["speed", "priority", "flow", "ingress", "egress"] {
        assert_eq!(result["unavailable"][field], "control is absent");
    }
    for (page, expected) in [
        (
            DASH.replace(id, r#"<input class="port" value="4">"#),
            "identifiers disagree",
        ),
        (DASH.replace(id, &id.repeat(2)), "duplicate controls"),
        (
            dashboard_config().replace("class=Speed value=6", "class=Speed value=999"),
            "port editor field speed",
        ),
    ] {
        let out = mock(&args, vec![], true, &page);
        assert_eq!(out.status.code(), Some(5));
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains(expected));
    }
}

#[test]
fn grouped_port_edit_round_trips_nondefault_siblings_and_checks_qos_and_lag() {
    let cfg = dashboard_config();
    let end = cfg.replace("Example NAS", "Bench 2");
    let key = "/dashboard.cgi";
    let answer = apply(
        &["settings", "set", "port", "--port", "2", "name=Bench 2"],
        key,
        &cfg,
        post(
            "/port_status.cgi",
            &[
                ("port2", "checked"),
                ("DESCRIPTION", "Bench 2"),
                ("SPEED", "6"),
                ("priority", "2"),
                ("FLOW_CONTROL", "2"),
                ("IngressRate", "2"),
                ("EgressRate", "1"),
            ],
        ),
        &end,
    );
    assert_eq!(answer["outcome"], "verified");
    rejected(run(
        &[
            "--apply",
            "settings",
            "set",
            "port",
            "--port",
            "2",
            "priority=low",
        ],
        vec![
            get(key, &cfg),
            get("/qos.cgi", "<input id=qosMode value=0>"),
        ],
    ));
    rejected(run(
        &[
            "--apply",
            "settings",
            "set",
            "port",
            "--port",
            "2",
            "speed=auto",
        ],
        vec![
            get(key, &cfg),
            get("/lag.cgi", lag_page("11000000", true, false)),
        ],
    ));
}
#[test]
fn port_overview_and_grouped_port_identifiers_are_checked_at_the_binary() {
    let report = data(run(&["ports", "--summary"], vec![]));
    assert_eq!(
        report,
        serde_json::json!({"total":8,"connected":2,"idle":6})
    );
    let cfg =
        dashboard_config().replace("class=\"port\" value=\"5\"", "class=\"port\" value=\"4\"");
    let out = run(
        &[
            "--apply", "settings", "set", "port", "--port", "4", "name=x",
        ],
        vec![get("/dashboard.cgi", cfg)],
    );
    assert_eq!(out.status.code(), Some(5));
}
#[test]
fn identity_qos_and_broadcast_filter_use_separate_forms() {
    let a = format!(
        "{DASH}{TOKEN}<input id=selLang value=en><ul id=langList><li value=en>English</li><li value=fr>French</li></ul>"
    );
    assert_eq!(
        apply(
            &["settings", "set", "identity", "language=fr"],
            "/dashboard.cgi",
            &a,
            post(
                "/switch_name.cgi",
                &[("switch_name", "Example & lab"), ("SET_LANG", "fr")]
            ),
            &a.replace("id=selLang value=en", "id=selLang value=fr")
        )["outcome"],
        "verified"
    );
    let a = format!("{TOKEN}<input type=checkbox id=stormControlCheck>");
    assert_eq!(
        apply(
            &["settings", "set", "broadcast-filter", "enabled=on"],
            "/qos.cgi",
            &a,
            post("/broadCastFilter.cgi", &[("status", "Enable")]),
            &a.replace("type=checkbox", "type=checkbox checked")
        )["outcome"],
        "verified"
    );
    let a = format!("{TOKEN}<input id=qosMode value=1>");
    assert_eq!(
        apply(
            &["settings", "set", "qos-mode", "mode=dscp"],
            "/qos.cgi",
            &a,
            post("/qos.cgi", &[("status", "Enable")]),
            &a.replace("value=1", "value=0")
        )["outcome"],
        "verified"
    );
}
#[test]
fn storm_rate_requires_filtering_and_preserves_all_seven_other_rates() {
    let page = |limit: bool| {
        let mut s = format!(
            "{TOKEN}<input type=checkbox id=stormControlCheck checked><section id=stormcontrol_edit>"
        );
        for p in 1..=8 {
            s += &format!(
                "<div class=dropdown><span class=selectItem>{}</span><ul><li value=2><a>Slow</a></li><li value=7><a>Fast</a></li></ul></div>",
                if limit && p == 4 { "Slow" } else { "Fast" }
            );
        }
        s + "</section>"
    };
    let args = ["settings", "set", "storm-rate", "--port", "4", "rate=Slow"];
    let mut req = post("/stromControl.cgi", &[]);
    for p in 1..=8 {
        req.form
            .as_mut()
            .unwrap()
            .insert(format!("port{p}"), if p == 4 { "2" } else { "7" }.into());
    }
    let json = data(run(
        &["settings", "show", "storm-rate", "--port", "4"],
        vec![get("/qos.cgi", page(false))],
    ));
    assert_eq!(json["values"], serde_json::json!({"rate":"Fast"}));
    assert_eq!(
        apply(&args, "/qos.cgi", &page(false), req, &page(true))["outcome"],
        "verified"
    );
    rejected(run(
        &[
            "--apply",
            "settings",
            "set",
            "storm-rate",
            "--port",
            "4",
            "rate=Slow",
        ],
        vec![get("/qos.cgi", page(false).replace(" checked", ""))],
    ));
}
#[test]
fn ipv4_dns_and_discovery_are_validated_before_any_submission() {
    let read = "/ip_dhcp.cgi";
    let a = format!(
        "{TOKEN}<input id=dhcpMod type=checkbox><input id=ipAddr value='192.0.2.8'><input id=subMsk value='255.255.255.0'><input id=gatWay value='192.0.2.1'><input id=dnsPri value=''><input id=dnsSec value=''>"
    );
    assert_eq!(
        apply(
            &["settings", "set", "address", "primary-dns=192.0.2.53"],
            read,
            &a,
            post(
                read,
                &[
                    ("dhcpMode", "0"),
                    ("ip_address", "192.0.2.8"),
                    ("subnet_mask", "255.255.255.0"),
                    ("gateway_address", "192.0.2.1"),
                    ("dns_primary", "192.0.2.53"),
                    ("dns_secondary", "")
                ]
            ),
            &a.replace("id=dnsPri value=''", "id=dnsPri value='192.0.2.53'")
        )["outcome"],
        "verified"
    );
    for arg in [
        "mask=255.0.255.0",
        "address=192.0.2.255",
        "gateway=198.51.100.1",
        "primary-dns=invalid",
    ] {
        rejected(run(
            &["--apply", "settings", "set", "address", arg],
            vec![get(read, &a)],
        ));
    }
    let d = "/switchDiscovery.cgi";
    let a = format!(
        "{TOKEN}<input id=upnp_status type=checkbox><input id=nsdp_status type=checkbox checked>"
    );
    assert_eq!(
        apply(
            &["settings", "set", "discovery", "upnp=on"],
            d,
            &a,
            post(d, &[("upnp", "Enable"), ("nsdp", "Enable")]),
            &a.replace(
                "id=upnp_status type=checkbox",
                "id=upnp_status type=checkbox checked"
            )
        )["outcome"],
        "verified"
    );
    rejected(run(
        &["--apply", "settings", "set", "discovery", "upnp=on"],
        vec![get(d, "<input id=upnp_status type=checkbox>")],
    ));
}
fn mirror_page(source: &[u8], dest: &[u8], on: bool) -> String {
    let mut s = format!(
        "{TOKEN}<input id=portMirrorMod type=checkbox {}>",
        if on { "checked" } else { "" }
    );
    for (id, ports) in [("sourcePortlist", source), ("destiPortlist", dest)] {
        s += &format!("<ul id={id}>");
        for p in 1..=8 {
            s += &format!(
                "<li class='port_circle {}'><span class=port_circle_num>{p}</span></li>",
                if ports.contains(&p) {
                    "port_circle_selected"
                } else {
                    ""
                }
            );
        }
        s += "</ul>";
    }
    s
}
#[test]
fn mirror_lag_and_multicast_consult_their_dependent_features() {
    let m = "/mirroring.cgi";
    let l = "/lag.cgi";
    let a = mirror_page(&[], &[], false);
    let args = [
        "settings",
        "set",
        "mirror",
        "enabled=on",
        "sources=2,4",
        "destination=5",
    ];
    assert_eq!(
        apply(
            &args,
            m,
            &a,
            post(
                m,
                &[
                    ("mirrorStatus", "1"),
                    ("sourcePort", "01010000"),
                    ("destiPort", "5")
                ]
            ),
            &mirror_page(&[2, 4], &[5], true)
        )["outcome"],
        "verified"
    );
    rejected(run(
        &[
            "--apply",
            "settings",
            "set",
            "mirror",
            "enabled=on",
            "sources=2",
            "destination=5",
        ],
        vec![get(m, &a), get(l, lag_page("11000000", true, false))],
    ));
    let a = lag_page("00000000", false, false);
    let after = lag_page("11000000", true, false);
    let mut seq = Vec::new();
    for _ in 0..2 {
        seq.extend([
            get(l, &a),
            get(m, mirror_page(&[], &[], false)),
            get("/dashboard.cgi", dashboard_config()),
            get(VLAN, mode(0, "")),
        ]);
    }
    seq.extend([
        post(
            l,
            &[
                ("adminMode1", "1"),
                ("portSet_1", "11000000"),
                ("adminMode2", "0"),
                ("portSet_2", "00000000"),
            ],
        ),
        get(l, after),
    ]);
    assert_eq!(
        data(run(
            &[
                "--apply",
                "settings",
                "set",
                "lag",
                "enabled-1=on",
                "members-1=1,2"
            ],
            seq
        ))["outcome"],
        "verified"
    );
    rejected(run(
        &["--apply", "vlan", "pvid", "2", "7"],
        vec![
            get(VLAN, advq(Some("22333333"), 1, 0, 0, false)),
            get(l, lag_page("11000000", true, false)),
        ],
    ));
    let mc = "/multicast.cgi";
    let p = multicast_page(true, Some(1));
    let steps = vec![get(mc, &p), get(VLAN, advq(None, 1, 0, 0, false))];
    rejected(run(
        &["--apply", "settings", "set", "multicast", "vlan=7"],
        steps,
    ));
    rejected(run(
        &["--apply", "vlan", "delete", "7"],
        vec![
            get(VLAN, advq(Some("22333333"), 1, 0, 0, false)),
            get(mc, multicast_page(true, Some(7))),
        ],
    ));
    assert_eq!(
        apply(
            &["settings", "set", "multicast", "validate-header=on"],
            mc,
            &p,
            post(
                mc,
                &[
                    ("status", "1"),
                    ("IP_HEADER", "1"),
                    ("BLOCK_UN_MUL_ADDR", "0"),
                    ("VLAN_ID_ENABLED", "1"),
                    ("ROUTER_PORT", "0")
                ]
            ),
            &p.replace("id=ip_header>", "id=ip_header checked>")
        )["outcome"],
        "verified"
    );
}
#[test]
fn sntp_client_and_http_sessions_preserve_grouped_fields() {
    let p = "/sntpSub.cgi?TAB_NUM=2";
    let root = "/sntpMain.cgi";
    let a = "<input id=sntpAdmin type=checkbox><input id=sntpMode type=checkbox><input id=sntpPort value=123><input id=ucastPollInterval value=8><input id=ucastPollTimeout value=4><input id=ucastPollRetry value=1>";
    let mut req = post(
        "/sntpClientConfig.cgi",
        &[
            ("SNTP_ADMIN", "1"),
            ("SNTP_CLIENT_MODE", "1"),
            ("SNTP_CLIENT_PORT", "123"),
            ("UCAST_POLL_INTERVAL", "8"),
            ("UCAST_POLL_TIMEOUT", "4"),
            ("UCAST_POLL_RETRY", "1"),
        ],
    );
    let form = req.form.as_mut().unwrap();
    let t = form.remove("hash").unwrap();
    form.insert("HASH".into(), t);
    assert_eq!(
        data(run(
            &["--apply", "settings", "set", "sntp-client", "enabled=on"],
            vec![
                get(p, a),
                get(root, TOKEN),
                get(p, a),
                get(root, TOKEN),
                req,
                get(
                    p,
                    a.replace(
                        "id=sntpAdmin type=checkbox",
                        "id=sntpAdmin type=checkbox checked"
                    )
                )
            ]
        ))["outcome"],
        "verified"
    );
    let h = "/httpConfiguration.cgi";
    let a = format!("{TOKEN}<input id=httpSessTimeout value=5><input id=httpMaxSess value=3>");
    assert_eq!(
        apply(
            &["settings", "set", "http-session", "timeout-minutes=12"],
            h,
            &a,
            post(
                h,
                &[("HTTP_SESSION_TIMEOUT", "12"), ("HTTP_SESSION_MAXNUM", "3")]
            ),
            &a.replace("value=5", "value=12")
        )["outcome"],
        "verified"
    );
}
#[test]
fn vlan_rename_management_and_oui_edit_preserve_neighbor_fields() {
    let a = advq(Some("22333333"), 1, 0, 3, true);
    assert_eq!(
        apply(
            &["vlan", "rename", "7", "Studio"],
            VLAN,
            &a,
            post(
                "/8021qAdvanced.cgi",
                &[
                    ("ACTION", "Apply"),
                    ("VLAN_ID", "7"),
                    ("VLAN_NAME", "Studio"),
                    ("hiddenMem", "22333333"),
                    ("vlanNum", "2"),
                    ("voiceVLANID", "0"),
                    ("voiceVlanCos", "3")
                ]
            ),
            &a.replace("Lab &amp; Bench", "Studio")
        )["outcome"],
        "verified"
    );
    assert_eq!(
        apply(
            &["vlan", "management", "7", "--accept-reconnect"],
            VLAN,
            &a,
            post(
                "/8021qAdvanced.cgi",
                &[("ACTION", "vlanMgmt"), ("mgmtVlanId", "7")]
            ),
            &a.replace("name=mgmtVlan value=1", "name=mgmtVlan value=7")
        )["outcome"],
        "verified"
    );
    assert_eq!(
        apply(
            &["vlan", "oui-edit", "02:00:01", "Phone"],
            VLAN,
            &a,
            post(
                "/OUITable.cgi",
                &[
                    ("ACTION", "Apply"),
                    ("TELEPHONY", "02:00:01"),
                    ("DESCRIPTION", "Phone"),
                    ("INDEX_0", "checked")
                ]
            ),
            &a.replace("<span desc>Bench", "<span desc>Phone")
        )["outcome"],
        "verified"
    );
    rejected(run(
        &[
            "--apply",
            "vlan",
            "set-mode",
            "basic-port",
            "--from",
            "disabled",
            "--accept-vlan-reset",
        ],
        vec![get(
            VLAN,
            mode(0, "").replace("name=\"bscPotBsd\"", "name=\"unrecognized\""),
        )],
    ));
}

fn tab_apply(
    args: &[&str],
    tab: &'static str,
    start: &str,
    mut request: Exchange,
    after: &str,
) -> serde_json::Value {
    let form = request.form.as_mut().unwrap();
    let val = form.remove("hash").unwrap();
    form.insert("HASH".into(), val);
    let mut options = vec!["--apply"];
    options.extend(args);
    let parent = "/sntpMain.cgi";
    data(run(
        &options,
        vec![
            get(tab, start),
            get(parent, TOKEN),
            get(tab, start),
            get(parent, TOKEN),
            request,
            get(tab, after),
        ],
    ))
}
fn sntp_servers(with: bool) -> String {
    let row = if with {
        "<li><div class=li-header><span>DNS</span><span>clock.example</span><span>123</span><span serverPri>2</span><span>4</span></div><div class=li-btn><button onclick='delSntpServer(0)'>Delete</button></div></li>"
    } else {
        ""
    };
    format!(
        "<div id=sntpServerList><ul class=tab-init>{row}</ul></div><ul id=serverTypeMenu><li><input class=hidVal value=1><a>IPv4</a></li><li><input class=hidVal value=2><a>DNS</a></li></ul>"
    )
}
#[test]
fn time_servers_use_the_visible_row_index_and_confirm_the_address() {
    let tab = "/sntpSub.cgi?TAB_NUM=3";
    let post_url = "/sntpServerConfig.cgi";
    let a = sntp_servers(false);
    let b = sntp_servers(true);
    let args = ["time", "server-add", "clock.example", "--priority", "2"];
    assert_eq!(
        tab_apply(
            &args,
            tab,
            &a,
            post(
                post_url,
                &[
                    ("ACTION", "ADD"),
                    ("SERVER_ADDR_TYPE", "2"),
                    ("SERVER_ADDRESS", "clock.example"),
                    ("SERVER_PORT", "123"),
                    ("SERVER_PRIORITY", "2"),
                    ("SERVER_VERSION", "4")
                ]
            ),
            &b
        )["outcome"],
        "verified"
    );
    assert_eq!(
        data(run(&["time", "servers"], vec![get(tab, &b)]))["servers"]["2"]["address"],
        "clock.example"
    );
    rejected(run(
        &[
            "--apply",
            "time",
            "server-delete",
            "2",
            "--address",
            "other.example",
        ],
        vec![get(tab, &b)],
    ));
    assert_eq!(
        tab_apply(
            &["time", "server-delete", "2", "--address", "clock.example"],
            tab,
            &b,
            post(post_url, &[("ACTION", "DELETE"), ("SERVER_INDEX", "0")]),
            &a
        )["outcome"],
        "verified"
    );
}
fn dst_form(selected: &str, zone: u8, custom: bool) -> String {
    let mut page = format!("<input id=timezoneHid value={zone}>");
    for (id, value) in [
        ("disable", 0),
        ("recurringUSA", 1),
        ("recurringEU", 2),
        ("recurring", 3),
        ("nonRecurring", 4),
    ] {
        page += &format!(
            "<input type=radio name=dstType id={id} value={value} {}>",
            if selected == id { "checked" } else { "" }
        );
    }
    if custom {
        for side in ["Begin", "End"] {
            for (suffix, n) in [
                ("Year", 2028),
                ("Month", if side == "Begin" { 3 } else { 10 }),
                ("Date", 1),
                ("Hours", 2),
                ("Mins", 0),
            ] {
                page += &format!(
                    "<input id='nonRec{side}{suffix}Hid' value={n}><ul id='nonRec{side}{suffix}Menu'><li><input class=hidVal value={n}></li></ul>"
                );
            }
        }
    }
    page
}
#[test]
fn daylight_saving_handles_regional_and_custom_rules_without_resubmitting_hidden_fields() {
    let tab = "/sntpSub.cgi?TAB_NUM=4";
    let dst = "/sntpDstConfig.cgi";
    let a = dst_form("disable", 5, true);
    assert_eq!(
        tab_apply(
            &["time", "set-dst", "eu", "--zone", "22"],
            tab,
            &a,
            post(dst, &[("DST_TYPE", "2"), ("TIMEZONE_INDEX", "22")]),
            &dst_form("recurringEU", 22, true)
        )["outcome"],
        "verified"
    );
    let b = dst_form("nonRecurring", 5, true);
    let args = [
        "time",
        "set-dst",
        "once",
        "--start",
        "2028-03-01T02:00",
        "--end",
        "2028-10-01T02:00",
    ];
    assert_eq!(
        tab_apply(
            &args,
            tab,
            &a,
            post(
                dst,
                &[
                    ("DST_TYPE", "4"),
                    ("TIMEZONE_INDEX", "5"),
                    ("DST_START_YEAR", "2028"),
                    ("DST_START_MONTH", "3"),
                    ("DST_START_DATE", "1"),
                    ("DST_START_HOURS", "2"),
                    ("DST_START_MINS", "0"),
                    ("DST_END_YEAR", "2028"),
                    ("DST_END_MONTH", "10"),
                    ("DST_END_DATE", "1"),
                    ("DST_END_HOURS", "2"),
                    ("DST_END_MINS", "0")
                ]
            ),
            &b
        )["outcome"],
        "verified"
    );
    rejected(run(
        &[
            "--apply",
            "time",
            "set-dst",
            "once",
            "--start",
            "2028-03-01T02:00",
            "--end",
            "2028-02-30T02:00",
        ],
        vec![get(tab, &a)],
    ));
}
#[test]
fn manual_clock_readback_accepts_a_tick_and_detects_the_wrong_date() {
    let read = "/sntpSystemTime.cgi";
    let page = |d: &str, t: &str| {
        format!(
            "{TOKEN}<input id=sntpAdmin type=checkbox><input id=systemDate value='{d}'><input id=systemTime value='{t}'><input id=timezoneHid value=7>"
        )
    };
    let a = page("2027-01-11", "01:02:03");
    let args = [
        "time",
        "set-clock",
        "manual",
        "--date",
        "2028-02-29",
        "--time",
        "23:59:59",
    ];
    let req = || {
        post(
            read,
            &[
                ("sntpAdmin", "false"),
                ("systemDate", "2028-02-29"),
                ("systemTime", "23:59:59"),
                ("timeZone", "7"),
            ],
        )
    };
    assert_eq!(
        apply(&args, read, &a, req(), &page("2028-03-01", "00:00:00"))["outcome"],
        "verified"
    );
    let mut o = vec!["--apply"];
    o.extend(args);
    assert_eq!(
        run(&o, vec![get(read, &a), get(read, &a), req(), get(read, &a)])
            .status
            .code(),
        Some(4)
    );
}
#[test]
fn time_status_requires_complete_distinct_fields() {
    for (cmd, tab, keys) in [
        (
            "status",
            "/sntpSub.cgi?TAB_NUM=0",
            vec![
                1003, 1004, 1005, 1006, 1007, 1008, 1009, 1010, 1011, 1012, 1013, 1014, 1015,
            ],
        ),
        (
            "dst-status",
            "/sntpSub.cgi?TAB_NUM=1",
            vec![1062, 1063, 1064, 1065, 1066, 1067],
        ),
    ] {
        let s=keys.iter().map(|n|format!("<div class=hid_info_cell><div class=hid_info_title><span>ml{n}</span></div><div><span>example</span></div></div>")).collect::<String>();
        let values = data(run(&["time", cmd], vec![get(tab, &s)]));
        assert_eq!(values.as_object().unwrap().len(), keys.len());
        rejected(run(
            &["time", cmd],
            vec![get(tab, s.replace(&format!("ml{}", keys[0]), "unknown"))],
        ));
    }
}
#[test]
fn stored_cable_results_follow_verified_column_headings() {
    let header = "<ul class=testResult_title><li class=testResult_title_li>Port</li><li class=testResult_title_li>ml078</li><li class=testResult_title_li>ml445</li></ul>";
    let result = "<span class=testResult_content_result>Open Cable</span>";
    let distance = "<span class=testResult_content_result>12</span>";
    let row = format!(
        "<li class=testResult_li><div class=testResultContainer><span class=testResult_content_port>4 - <b>Fixture</b></span>{result}{distance}</div></li>"
    );
    let list = format!("<ul class=testResult_list>{row}</ul>");
    let page = format!("<div id=testResult>{header}{list}</div>");
    let read = |p: &str| {
        run(
            &["maintenance", "cable-results"],
            vec![get("/cableTester.cgi", p)],
        )
    };
    assert_eq!(
        data(read(&page)),
        serde_json::json!([{"port":4,"result":"Open Cable","distance":"12"}])
    );
    for (invalid, expected) in [
        (page.replace(header, ""), "one stored result cell"),
        (page.replace("ml078", "ml445"), "column headings"),
        (page.replace(header, &header.repeat(2)), "column headings"),
        (page.replace(distance, ""), "test result and distance"),
        (
            page.replace(distance, &distance.repeat(2)),
            "test result and distance",
        ),
        (
            page.replace(result, "<span>unknown</span>"),
            "disagree with column headings",
        ),
        (
            page.replace(&row, &row.repeat(2)),
            "duplicate port identifier",
        ),
        (
            page.replace(&list, &list.repeat(2)),
            "one stored cable-results list",
        ),
    ] {
        let out = read(&invalid);
        assert_eq!(out.status.code(), Some(5));
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains(expected));
    }
}
const UNTESTED_CABLE: &str = include_str!("fixtures/cable-results-untested.html");

#[test]
fn explicit_untested_rows_need_firmware_and_verified_columns() {
    let run_on = |version: &str, page: &str| {
        mock(
            &["maintenance", "cable-results"],
            vec![get("/cableTester.cgi", page)],
            true,
            &DASH.replace("V1.0.1.13", version),
        )
    };
    for firmware in ["V1.0.1.13", "V1.0.2.0"] {
        for page in [
            UNTESTED_CABLE.to_string(),
            UNTESTED_CABLE.replace(">--</span>", "></span>"),
        ] {
            assert_eq!(data(run_on(firmware, &page)), serde_json::json!([]));
        }
        let row = &UNTESTED_CABLE[UNTESTED_CABLE.find("<li class=\"testResult_li\">").unwrap()
            ..UNTESTED_CABLE.rfind("</li>").unwrap() + 5];
        let real = row
            .replace(">2&nbsp;", ">4&nbsp;")
            .replace(">Not test<", ">Open Cable<")
            .replace(">--<", ">12<");
        let mixed = UNTESTED_CABLE.replace(row, &format!("{row}{real}"));
        assert_eq!(
            data(run_on(firmware, &mixed)),
            serde_json::json!([{"port":4,"result":"Open Cable","distance":"12"}])
        );
        let no_cable = UNTESTED_CABLE.replace(">Not test<", ">No Cable<");
        assert_eq!(data(run_on(firmware, &no_cable))[0]["result"], "No Cable");
        let same = UNTESTED_CABLE.replace(
            row,
            &format!("{row}{}", real.replace(">4&nbsp;", ">2&nbsp;")),
        );
        let repeated = run_on(firmware, &same);
        assert_eq!(repeated.status.code(), Some(5));
        assert!(String::from_utf8_lossy(&repeated.stderr).contains("duplicate port identifier"));
    }
    for firmware in ["V1.0.1.12", "unidentified"] {
        let out = mock(
            &["maintenance", "cable-results"],
            vec![],
            true,
            &DASH.replace("V1.0.1.13", firmware),
        );
        assert_eq!(out.status.code(), Some(5));
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn stored_blank_pairs_are_absent_only_on_the_reviewed_firmware_and_exact_rows() {
    let page = UNTESTED_CABLE
        .replace(">Not test<", "><")
        .replace(">--<", "><");
    let run_on = |version: &str, html: &str| {
        mock(
            &["maintenance", "cable-results"],
            vec![get("/cableTester.cgi", html)],
            true,
            &DASH.replace("V1.0.1.13", version),
        )
    };
    let read = |html: &str| run_on("V1.0.1.13", html);
    let label = "&nbsp;-&nbsp;<b common>Synthetic label</b>";
    let plain = page.replace(label, "");
    for html in [&page, &plain] {
        assert_eq!(data(read(html)), serde_json::json!([]));
    }
    let row = &plain
        [plain.find("<li class=\"testResult_li\">").unwrap()..plain.rfind("</li>").unwrap() + 5];
    let measured = row.replace(">2</span>", ">5</span>").replacen(
        "_result\"></span>",
        "_result\">No Cable</span>",
        1,
    );
    let mixed = plain.replace(row, &format!("{row}{measured}"));
    assert_eq!(
        data(read(&mixed)),
        serde_json::json!([{"port":5,"result":"No Cable"}])
    );
    let out = run_on("V1.0.1.14", &page);
    assert_eq!(out.status.code(), Some(5));
    assert!(out.stdout.is_empty());
    for version in ["V1.0.1.12", "unidentified"] {
        let out = mock(
            &["maintenance", "cable-results"],
            vec![],
            true,
            &DASH.replace("V1.0.1.13", version),
        );
        assert_eq!(out.status.code(), Some(5));
        assert!(out.stdout.is_empty());
    }
    for invalid in [
        mixed.replace(">5</span>", ">2</span>"),
        plain.replace(row, &row.repeat(2)),
        plain.replace(">2</span>", ">9</span>"),
        plain.replace(">2</span>", ">02</span>"),
        plain.replace(">2</span>", "><b common>2</b></span>"),
        plain.replace(
            "testResult_content testResult_content_result",
            "testResult_content_result",
        ),
        plain.replacen("_result\"></span>", "_result\"><b></b></span>", 1),
        plain.replacen("_result\"></span>", "_result\"> </span>", 1),
        plain.replace("ml445", "ml078"),
        plain.replace("</span>\n          <span", "</span>unrecognized<span"),
    ] {
        let out = read(&invalid);
        assert_eq!(out.status.code(), Some(5));
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn ambiguous_stored_cable_rows_fail_without_printing_cell_values() {
    let empty = UNTESTED_CABLE.replace(">Not test<", "><");
    for (page, reason) in [
        (empty.clone(), "distance placeholder"),
        (
            empty.replace(">--<", ">Not test<"),
            "untested marker in the distance column",
        ),
        (
            empty.replace(">--<", ">private-marker<"),
            "populated distance",
        ),
        (
            UNTESTED_CABLE.replace(">--<", ">12<"),
            "conflicts with a populated distance",
        ),
        (
            UNTESTED_CABLE.replace(">Not test<", ">private-marker<"),
            "unrecognized status",
        ),
        (
            UNTESTED_CABLE.replace(">Not test<", ">--<"),
            "unrecognized status",
        ),
        (
            UNTESTED_CABLE.replace(">2&nbsp;", " >2x&nbsp;"),
            "unrecognized port label",
        ),
        (
            UNTESTED_CABLE.replace("class=\"testResult_li\"", "class=\"unknown-row\""),
            "unrecognized content",
        ),
        (
            UNTESTED_CABLE.replace("Not test", "<span>Not test</span>"),
            "unrecognized cells",
        ),
        (
            UNTESTED_CABLE.replace(
                "testResult_content testResult_content_result",
                "testResult_content_result",
            ),
            "unrecognized cells",
        ),
        (UNTESTED_CABLE.replace("ml078", "ml445"), "column headings"),
    ] {
        let out = run(
            &["maintenance", "cable-results"],
            vec![get("/cableTester.cgi", page)],
        );
        assert_eq!(out.status.code(), Some(5));
        assert!(out.stdout.is_empty());
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(error.contains(reason), "{error}");
        assert!(!error.contains("private-marker") && !error.contains("Synthetic label"));
    }
}

#[test]
fn disruptive_diagnostics_require_scope_and_report_acceptance_separately() {
    let read = "/cableTester.cgi";
    let ports = (1..=8)
        .map(|p| format!("<li><span class=port_circle_num>{p}</span></li>"))
        .collect::<String>();
    let page = format!(
        "{TOKEN}<ul class=cable_test_port_list>{ports}</ul><button name=submitCbtst>Next</button><div id=testResult><ul class=testResult_list><li class=testResult_li><span class=testResult_content_port>4 - Studio</span><span class=testResult_content_result>Normal</span></li></ul></div>"
    );
    assert_eq!(
        data(run(
            &["maintenance", "cable-results"],
            vec![get(read, &page)]
        ))[0]["port"],
        4
    );
    assert_eq!(
        data(run(
            &["--apply", "maintenance", "cable-test", "4"],
            vec![get(read, &page), post(read, &[("port3", "checked")])]
        ))["outcome"],
        "accepted"
    );
    rejected(run(
        &[
            "--apply",
            "--protect-port",
            "4",
            "maintenance",
            "cable-test",
            "4",
        ],
        vec![],
    ));
    let reboot = "/rebootSwitch.cgi";
    let a = format!("{TOKEN}<span name=submitReboot>Reboot</span>");
    assert_eq!(
        data(run(
            &["maintenance", "reboot", "--accept-outage"],
            vec![get(reboot, &a)]
        ))["outcome"],
        "preview"
    );
    assert_eq!(
        data(run(
            &["maintenance", "reboot", "--accept-outage", "--apply"],
            vec![get(reboot, &a), post("/device_reboot.cgi", &[])]
        ))["outcome"],
        "accepted"
    );
    let mut err = post("/device_reboot.cgi", &[]);
    err.page = "application error".into();
    assert_eq!(
        run(
            &["maintenance", "reboot", "--accept-outage", "--apply"],
            vec![get(reboot, &a), err]
        )
        .status
        .code(),
        Some(4)
    );
}
#[test]
fn backup_returns_only_a_configuration_attachment_and_firmware_rejects_other_images() {
    let screen = "/confFile.cgi";
    let file = "/confFile.cgi?cmd=backup_conf";
    let html = r#"<div class=download-conf-btn onclick="document.location.href='/confFile.cgi?cmd=backup_conf'"><button name=downloadCf>Download</button></div>"#;
    let args = ["maintenance", "backup", "--stdout"];
    let out = mock(
        &args,
        vec![get(screen, html), get(file, "synthetic configuration")],
        false,
        DASH,
    );
    assert!(out.status.success());
    assert_eq!(out.stdout, b"synthetic configuration");
    rejected(mock(
        &args,
        vec![get(screen, html), get(file, "<html>unexpected</html>")],
        false,
        DASH,
    ));
    rejected(mock(
        &args,
        vec![get(
            screen,
            html.replace("/confFile.cgi?cmd=backup_conf", "https://example.invalid"),
        )],
        false,
        DASH,
    ));
    let f = "<form id=uploadForm method=post enctype='multipart/form-data' action='/firmware.cgi?cmd=fw_upgrade'><input type=hidden id=hash name=hash value=fixture-marker><input type=file name=fileField accept='.bin'></form><input id=prctName value=GS308Ev4><input id=minFileSize value=1><input id=maxFileSize value=2048>";
    rejected(run(
        &[
            "maintenance",
            "upgrade",
            "Cargo.toml",
            "--accept-outage",
            "--apply",
        ],
        vec![get("/firmware.cgi", f)],
    ));
    rejected(run(
        &[
            "maintenance",
            "upgrade",
            "Cargo.toml",
            "--accept-outage",
            "--apply",
        ],
        vec![get(
            "/firmware.cgi",
            f.replace("/firmware.cgi?cmd=fw_upgrade", "https://example.invalid"),
        )],
    ));
}

#[test]
fn removed_discovery_control_is_version_specific_and_guarded_port_edits_remain_local() {
    let d = "/switchDiscovery.cgi";
    let before = format!("{TOKEN}<input type=checkbox id=upnp_status>");
    let a = ["--apply", "settings", "set", "discovery", "upnp=on"];
    let seq = vec![
        get(d, &before),
        get(d, &before),
        post(d, &[("upnp", "Enable"), ("nsdp", "Enable")]),
        get(d, before.replace("type=checkbox", "type=checkbox checked")),
    ];
    assert_eq!(data(mock(&a, seq, true, DASH))["outcome"], "verified");
    let dash = "/dashboard.cgi";
    let cfg = dashboard_config();
    let args = [
        "--apply",
        "--protect-port",
        "2",
        "settings",
        "set",
        "port",
        "--port",
        "4",
        "name=Fixture label",
    ];
    let after = cfg.replace(
        "value=\"4\"><span>4</span><input class=\"portName\" value=\"\"",
        "value=\"4\"><span>4</span><input class=\"portName\" value=\"Fixture label\"",
    );
    let seq = vec![
        get(dash, &cfg),
        get(dash, &cfg),
        post(
            "/port_status.cgi",
            &[
                ("port4", "checked"),
                ("DESCRIPTION", "Fixture label"),
                ("SPEED", "6"),
                ("FLOW_CONTROL", "2"),
                ("IngressRate", "2"),
                ("EgressRate", "1"),
                ("priority", "2"),
            ],
        ),
        get(dash, after),
    ];
    assert_eq!(data(run(&args, seq))["outcome"], "verified");
    let args = [
        "--apply",
        "--protect-port",
        "4",
        "settings",
        "set",
        "port",
        "--port",
        "4",
        "name=x",
    ];
    rejected(run(&args, vec![]));
}

#[test]
fn newer_identity_requires_the_complete_local_form_through_fresh_preflight() {
    let dash = "/dashboard.cgi";
    let page = DASH.replace("V1.0.1.13", "V1.0.2.0")
        .replace(r#"<div id="sysinfoContainer">"#, r#"<div id="sysinfoContainer"><form name=sysInfo method=post action=switch_name.cgi>"#)
        .replace(r#"<input id="switchName""#,r#"<input name=switch_name type=text id="switchName""#)
        .replace("</div>\n<div id=\"port_details\"",r#"<input type=hidden id=selLang name=SET_LANG value=en><ul id=langList><li value=en>English</li></ul><input type=hidden id=hash name=hash value=fixture-marker></form></div><div id="port_details""#);
    let cmd = ["--apply", "settings", "set", "identity", "name=New"];
    let confirmed = page.replace("Example &amp; lab", "New");
    let seq = vec![
        get(dash, &page),
        get(dash, &page),
        post(
            "/switch_name.cgi",
            &[("switch_name", "New"), ("SET_LANG", "en")],
        ),
        get(dash, confirmed),
    ];
    assert_eq!(data(mock(&cmd, seq, true, &page))["outcome"], "verified");
    for changed in [
        page.replace("action=switch_name.cgi", "action=https://example.invalid/"),
        page.replace("</form>", "<input name=sibling></form>"),
    ] {
        rejected(mock(&cmd, vec![], true, &changed));
        rejected(mock(
            &cmd,
            vec![get(dash, &page), get(dash, changed)],
            true,
            &page,
        ));
    }
}

#[test]
fn reviewed_discovery_reports_retired_control_only_on_exact_release() {
    let discovery = "/switchDiscovery.cgi";
    for (v, retired) in [("V1.0.1.13", true), ("V1.0.1.14", false)] {
        let d = data(mock(
            &["settings", "show", "discovery"],
            vec![get(discovery, "<input type=checkbox id=upnp_status>")],
            true,
            &DASH.replace("V1.0.1.13", v),
        ));
        assert_eq!(d["unavailable"]["nsdp"].is_string(), retired);
    }
}

#[test]
fn protect_port_allows_unprotected_port_and_vlan_operations_and_blocks_protected_ones() {
    let p = advq(Some("22333333"), 1, 0, 6, false);
    // Port 1 is protected, but changing PVID on port 2 or renaming VLAN 7 is allowed
    let after_pvid = advq(Some("22333333"), 7, 0, 6, false);
    assert_eq!(
        data(run(
            &["--protect-port", "1", "--apply", "vlan", "pvid", "2", "7"],
            vec![
                get(VLAN, &p),
                get(VLAN, &p),
                post("/portPVID.cgi", &[("PORT", "2"), ("PVID", "7")]),
                get(VLAN, &after_pvid),
            ]
        ))["outcome"],
        "verified"
    );
    // Changing VLAN 1 (management VLAN) membership is allowed as long as PVIDs remain consistent
    let p_pvid40 = advq(Some("22222222"), 7, 0, 6, false);
    let after_vlan1 = p_pvid40.replace(
        "<li><span vid-4>1</span><span vnm-4>Default</span><input hidden-mem value='22222222'></li>",
        "<li><span vid-4>1</span><span vnm-4>Default</span><input hidden-mem value='23222222'></li>",
    );
    assert_eq!(
        data(run(
            &[
                "--protect-port",
                "1",
                "--apply",
                "vlan",
                "members",
                "1",
                "--untagged",
                "1,3,4,5,6,7,8",
            ],
            vec![
                get(VLAN, &p_pvid40),
                get(VLAN, &p_pvid40),
                post(
                    "/8021qAdvanced.cgi",
                    &[
                        ("ACTION", "Apply"),
                        ("VLAN_ID", "1"),
                        ("VLAN_NAME", "Default"),
                        ("vlanNum", "2"),
                        ("hiddenMem", "23222222"),
                        ("voiceVLANID", "0"),
                        ("voiceVlanCos", "6"),
                    ]
                ),
                get(VLAN, &after_vlan1),
            ]
        ))["outcome"],
        "verified"
    );
    // Changing VLAN 1 membership in a way that removes protected port 1 is blocked
    rejected(run(
        &[
            "--protect-port",
            "1",
            "--apply",
            "vlan",
            "members",
            "7",
            "--untagged",
            "2",
        ],
        vec![get(VLAN, &p)],
    ));
    // Storm-rate on unprotected port 4 is allowed when port 1 is protected, and blocked on port 1
    rejected(run(
        &[
            "--protect-port",
            "1",
            "--apply",
            "settings",
            "set",
            "storm-rate",
            "--port",
            "1",
            "rate=Slow",
        ],
        vec![],
    ));
    // vlan create touching a protected port is blocked before any VLAN GET
    for ports_flag in ["--tagged", "--untagged"] {
        let ports_arg = if ports_flag == "--tagged" { "1" } else { "1,4" };
        rejected(run(
            &[
                "--protect-port",
                "1",
                "--apply",
                "vlan",
                "create",
                "20",
                "--name",
                "Blocked",
                ports_flag,
                ports_arg,
            ],
            vec![],
        ));
    }
    // vlan create on unprotected port 4 is allowed when port 1 is protected
    let before_create = advq(None, 1, 0, 6, false);
    let after_create = before_create
        .replace(
            "<ul bind=btns-4>",
            &format!("<ul bind=btns-4>{}", row(4, 20, "Allowed", "33323333")),
        )
        .replace("<input id=vlanNum value=1>", "<input id=vlanNum value=2>");
    assert_eq!(
        data(run(
            &[
                "--protect-port",
                "1",
                "--apply",
                "vlan",
                "create",
                "20",
                "--name",
                "Allowed",
                "--untagged",
                "4",
            ],
            vec![
                get(VLAN, &before_create),
                get(VLAN, &before_create),
                post(
                    "/8021qAdvanced.cgi",
                    &[
                        ("ACTION", "Add"),
                        ("VLAN_ID", "20"),
                        ("VLAN_NAME", "Allowed"),
                        ("vlanNum", "1"),
                        ("hiddenMem", "33323333"),
                        ("voiceVLANID", "0"),
                        ("voiceVlanCos", "6"),
                    ],
                ),
                get(VLAN, &after_create),
            ],
        ))["outcome"],
        "verified"
    );
    // Unchanged settings set uses kebab-case feature name and "no changes"
    let led = include_str!("fixtures/led-settings.html");
    let unchanged = data(run(
        &["settings", "set", "leds", "enabled=off"],
        vec![get("/leds.cgi", led)],
    ));
    assert_eq!(unchanged["outcome"], "unchanged");
    assert_eq!(unchanged["details"], "leds no changes");
}
