//! The Effect level (T2-APP-118): a block asks the host to fetch a URL on
//! its behalf and hears what came back — the guest opens no socket. A
//! shell that grants no network answers the fetch inside the guest, and a
//! fixture page answers it from what it holds.

#![cfg(feature = "engine")]

extern crate bunny_ui_core as bunny_ui;

use bunny_ui::prelude::*;
use bunny_ui_shell::guest::{Fixture, Guest, Page};
use bunny_ui_shell::host::TestHost;
use bunny_ui_shell::protocol::{
    self, Capabilities, Fetch, Fetched, Message, Modifiers, PointerButton, PointerPhase, Response, Role, Scheme,
    ToGuest, Versions, Viewport,
};

const RATES: &str = "https://api.example.com/rates";

/// A block that fetches the day's rates on a press and shows what came
/// back.
#[derive(Clone, Copy)]
struct Rates {
    page: Page,
    asked: State<Option<u64>>,
}

impl Component for Rates {
    fn body(self) -> impl View {
        let page = self.page;
        let shown = match self.asked.get().and_then(|fetch| page.fetched(fetch)) {
            None => "nothing yet".to_string(),
            Some(Fetched::Answered(response)) => {
                format!("{}: {}", response.status, String::from_utf8_lossy(&response.body))
            }
            Some(Fetched::Refused(why)) => format!("refused: {why}"),
            Some(Fetched::Failed(why)) => format!("failed: {why}"),
        };
        let asked = self.asked;
        vstack!(text(shown), button(text("Fetch the rates"), move || asked.set(Some(page.fetch(RATES)))))
            .alignment(HorizontalAlignment::Leading)
            .padding()
    }
}

fn rates(page: Page) -> Rates {
    Rates { page, asked: State::new(None) }
}

const VIEWPORT: Viewport = Viewport { width: 320.0, height: 120.0, scale: 1.0, scheme: Scheme::Light };

const EFFECT: Capabilities = Capabilities::from_bits(
    Capabilities::SCENE.bits()
        | Capabilities::POINTER.bits()
        | Capabilities::ACCESSIBILITY.bits()
        | Capabilities::FETCHES.bits(),
);

fn turn(guest: &mut Guest<Rates>, host: &mut TestHost, messages: &[ToGuest]) {
    let mut bytes = Vec::new();
    for message in messages {
        bytes.extend(protocol::encode(&Message::ToGuest(message.clone())));
    }
    host.receive(&guest.call(&bytes)).expect("the guest speaks the protocol");
}

fn mounted(capabilities: Capabilities) -> (Guest<Rates>, TestHost) {
    let mut guest = Guest::over(rates);
    let mut host = TestHost::new();
    turn(&mut guest, &mut host, &[ToGuest::Offer { versions: Versions::THIS, capabilities, viewport: VIEWPORT }]);
    (guest, host)
}

fn labels(host: &TestHost) -> Vec<String> {
    host.semantics().iter().map(|node| node.label.clone()).collect()
}

fn press(guest: &mut Guest<Rates>, host: &mut TestHost) {
    let button = host.semantics().iter().find(|node| node.role == Role::Button).expect("a button").bounds;
    let (x, y) = (button.0 + button.2 / 2.0, button.1 + button.3 / 2.0);
    let pointer =
        |phase| ToGuest::Pointer { phase, x, y, button: PointerButton::Primary, clicks: 1, modifiers: Modifiers::NONE };
    turn(guest, host, &[pointer(PointerPhase::Down), pointer(PointerPhase::Up)]);
}

#[test]
fn a_fetch_crosses_as_a_question_and_the_answer_reaches_the_view() {
    let (mut guest, mut host) = mounted(EFFECT);
    assert_eq!(host.session().unwrap().capabilities, EFFECT);
    press(&mut guest, &mut host);
    assert_eq!(host.fetches(), [Fetch { fetch: 1, url: RATES.into() }], "the guest asks; it opens nothing");
    assert!(labels(&host).contains(&"nothing yet".to_string()), "{:?}", labels(&host));

    let answer = Response { status: 200, content_type: Some("application/json".into()), body: b"5.1".to_vec() };
    turn(&mut guest, &mut host, &[ToGuest::Fetched { fetch: 1, fetched: Fetched::Answered(answer) }]);
    assert!(labels(&host).contains(&"200: 5.1".to_string()), "{:?}", labels(&host));
}

#[test]
fn a_refusal_by_name_reaches_the_view() {
    let (mut guest, mut host) = mounted(EFFECT);
    press(&mut guest, &mut host);
    let refused = Fetched::Refused("the org has not granted api.example.com".into());
    turn(&mut guest, &mut host, &[ToGuest::Fetched { fetch: 1, fetched: refused }]);
    assert!(
        labels(&host).contains(&"refused: the org has not granted api.example.com".to_string()),
        "{:?}",
        labels(&host)
    );
}

#[test]
fn a_shell_that_grants_no_network_is_answered_inside_the_guest() {
    let (mut guest, mut host) = mounted(Capabilities::SCENE | Capabilities::POINTER | Capabilities::ACCESSIBILITY);
    press(&mut guest, &mut host);
    assert!(host.fetches().is_empty(), "an ungranted fetch never crosses");
    assert!(
        labels(&host).contains(&"refused: this shell grants this block no network".to_string()),
        "the view hears the refusal: {:?}",
        labels(&host)
    );
}

#[test]
fn an_answer_of_an_ungranted_kind_is_dropped() {
    let (mut guest, mut host) = mounted(Capabilities::SCENE | Capabilities::POINTER | Capabilities::ACCESSIBILITY);
    let answer = Fetched::Answered(Response { status: 200, content_type: None, body: b"x".to_vec() });
    let bytes = protocol::encode(&Message::ToGuest(ToGuest::Fetched { fetch: 1, fetched: answer }));
    assert!(guest.call(&bytes).is_empty(), "a host that granted no network has nothing to answer");
    press(&mut guest, &mut host);
    assert!(!labels(&host).iter().any(|label| label.starts_with("200")), "{:?}", labels(&host));
}

#[test]
fn a_fetch_and_a_request_share_one_counter() {
    let page = Fixture::new().page();
    let request = page.request("send_to_review", None);
    let fetch = page.fetch(RATES);
    assert_ne!(request, fetch, "a number names one question");
}

#[test]
fn a_fixture_page_answers_a_fetch_from_what_it_holds() {
    let rates = Fetched::Answered(Response { status: 200, content_type: None, body: b"5.1".to_vec() });
    let page = Fixture::new().fetches(RATES, Fetched::Failed("slow".into())).fetches(RATES, rates.clone()).page();
    let held = page.fetch(RATES);
    assert_eq!(page.fetched(held), Some(rates), "the URL's answer replaces the one held for it");
    let elsewhere = page.fetch("https://elsewhere.example.com/");
    assert_eq!(page.fetched(elsewhere), Some(Fetched::Refused(Fixture::OFFLINE.to_string())));
    assert_eq!(page.fetched(elsewhere + 1), None, "a fetch never asked has no answer");
}
