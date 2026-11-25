use reqwest::blocking::Client;
use scraper::{Html, Selector};
use std::{thread, time::Duration};
use tiny_http::{Method, Response, Server};

const EDIT: &str = include_str!("fixtures/port-editor.html");
const STATS: &str = include_str!("fixtures/statistics.html");

#[derive(Debug, PartialEq, Eq)]
struct Editor {
    port: u8,
    label: String,
    speed: String,
    ingress: String,
    egress: String,
    flow: bool,
}

impl Editor {
    fn read(page: &str) -> Option<Self> {
        let html = Html::parse_fragment(page);
        let field = |selector: &str| {
            let selector = Selector::parse(selector).ok()?;
            let mut matches = html.select(&selector);
            let one = matches.next()?;
            matches.next().is_none().then_some(one)
        };
        let value = |s| field(s)?.attr("value").map(str::to_owned);
        Some(Self {
            port: value("#port_edit #portID")?
                .parse::<u8>()
                .ok()
                .filter(|p| (1..=8).contains(p))?,
            label: value("#port_edit #portName")?,
            speed: value("#port_edit #speedSelect option[selected]")?,
            ingress: value("#port_edit #IngressSelect option[selected]")?,
            egress: value("#port_edit #EgressSelect option[selected]")?,
            flow: field("#port_edit #flowControlCheck")?
                .attr("checked")
                .is_some(),
        })
    }
}

#[test]
fn dashboard_editor_keeps_nondefault_speed_limits_and_flow_with_a_renamed_port() {
    let before = Editor::read(EDIT).unwrap();
    assert_eq!(before.label, "Lab & bench");
    let after = Editor::read(&EDIT.replace("Lab &amp; bench", "Rack B")).unwrap();
    assert_eq!(
        after,
        Editor {
            label: "Rack B".into(),
            ..before
        }
    );
}

#[test]
fn incomplete_or_ambiguous_port_forms_cannot_supply_a_preimage() {
    for field in [
        "portName",
        "portID",
        "speedSelect",
        "IngressSelect",
        "EgressSelect",
        "flowControlCheck",
    ] {
        assert!(Editor::read(&EDIT.replace(field, "unrecognized")).is_none());
    }
    let invalid = EDIT.replace("value=\"7\"", "value=\"0\"");
    assert!(Editor::read(&invalid).is_none());
    assert!(Editor::read(&format!("{EDIT}{EDIT}")).is_none());
}

fn counter_words(body: &str) -> Vec<u32> {
    Html::parse_document(body)
        .select(&Selector::parse(".statistic-ul input[type=hidden]").unwrap())
        .map(|e| e.attr("value").unwrap().parse().unwrap())
        .collect()
}

#[test]
fn counter_clear_requires_the_mock_form_marker_and_clears_all_eight_rows() {
    // Only the loopback listener created here can be the destination. The token, reply
    // statuses and contents are synthetic. There is no API that accepts a switch URL.
    let server = Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}/portStatistics.cgi", server.server_addr());
    let worker = thread::spawn(move || {
        let mut cleared = false;
        for _ in 0..5 {
            let mut req = server
                .recv_timeout(Duration::from_secs(4))
                .unwrap()
                .expect("missing fixture request");
            assert_eq!(req.url(), "/portStatistics.cgi");
            let (status, body) = match req.method() {
                Method::Get => {
                    let mut body = STATS.to_owned();
                    if cleared {
                        for word in counter_words(STATS) {
                            body = body.replace(&format!("value=\"{word}\""), "value=\"0\"");
                        }
                    }
                    (200, body)
                }
                Method::Post => {
                    let mut form = String::new();
                    req.as_reader().read_to_string(&mut form).unwrap();
                    if form == "clearCounters=1&hash=fixture-marker" {
                        cleared = true;
                        (200, "fixture-accepted".into())
                    } else {
                        (400, "fixture-rejected".into())
                    }
                }
                _ => panic!("unexpected method"),
            };
            req.respond(Response::from_string(body).with_status_code(status))
                .unwrap();
        }
    });
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let read = || {
        counter_words(
            &client
                .get(&url)
                .send()
                .unwrap()
                .error_for_status()
                .unwrap()
                .text()
                .unwrap(),
        )
    };
    let before = read();
    assert_eq!(before.len(), 48);
    assert!(before.iter().any(|&w| w != 0));
    let submit = |marker| {
        client
            .post(&url)
            .form(&[("clearCounters", "1"), ("hash", marker)])
            .send()
            .unwrap()
            .status()
    };
    assert_eq!(submit("stale-marker"), 400);
    assert_eq!(read(), before);
    assert_eq!(submit("fixture-marker"), 200);
    assert_eq!(read(), vec![0; 48]);
    worker.join().unwrap();
}
