//! A block with a signature (D723): the page it stands on reaches its
//! view, the rows it reads too, and the declared actions it asks for
//! cross to the host as requests — never as writes.

#![cfg(feature = "engine")]

extern crate bunny_ui_core as bunny_ui;

use bunny_ui::prelude::*;
use bunny_ui_shell::guest::{Guest, Page};
use bunny_ui_shell::host::TestHost;
use bunny_ui_shell::protocol::{
    self, Capabilities, Context, Filter, Message, Modifiers, Outcome, PointerButton, PointerPhase, Requested, Role,
    Row, Rows, Scheme, ToGuest, ToHost, Versions, Viewport,
};

/// A block that shows the open record and the count of rows it reads, and
/// asks for `send_to_review` on the open record.
#[derive(Clone, Copy)]
struct Review {
    page: Page,
    asked: State<Option<u64>>,
}

impl Component for Review {
    fn body(self) -> impl View {
        let page = self.page;
        let record = page.record().unwrap_or_else(|| "no record".to_string());
        let rows = page.rows("work").map_or(0, |rows| rows.rows.len());
        let answer = match self.asked.get().and_then(|request| page.answer(request)) {
            None => "unanswered".to_string(),
            Some(Outcome::Landed) => "landed".to_string(),
            Some(Outcome::Held) => "held".to_string(),
            Some(Outcome::Declined) => "declined".to_string(),
            Some(Outcome::Refused(why)) => format!("refused: {why}"),
        };
        let asked = self.asked;
        vstack!(
            text(format!("open: {record}")),
            text(format!("rows: {rows}")),
            text(answer),
            button(text("Send to review"), move || {
                let record = page.record();
                asked.set(Some(page.request("send_to_review", record.as_deref())));
            }),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding()
    }
}

fn review(page: Page) -> Review {
    Review { page, asked: State::new(None) }
}

const VIEWPORT: Viewport = Viewport { width: 240.0, height: 140.0, scale: 1.0, scheme: Scheme::Light };

const SIGNED: Capabilities = Capabilities::from_bits(
    Capabilities::SCENE.bits()
        | Capabilities::POINTER.bits()
        | Capabilities::ACCESSIBILITY.bits()
        | Capabilities::CONTEXT.bits()
        | Capabilities::READS.bits()
        | Capabilities::REQUESTS.bits(),
);

fn turn(guest: &mut Guest<Review>, host: &mut TestHost, messages: &[ToGuest]) {
    let mut bytes = Vec::new();
    for message in messages {
        bytes.extend(protocol::encode(&Message::ToGuest(message.clone())));
    }
    host.receive(&guest.call(&bytes)).expect("the guest speaks the protocol");
}

fn offer(capabilities: Capabilities) -> ToGuest {
    ToGuest::Offer { versions: Versions::THIS, capabilities, viewport: VIEWPORT }
}

fn labels(host: &TestHost) -> Vec<String> {
    host.semantics().iter().map(|node| node.label.clone()).collect()
}

fn press_the_button(guest: &mut Guest<Review>, host: &mut TestHost) {
    let button = host.semantics().iter().find(|node| node.role == Role::Button).expect("a button").bounds;
    let (x, y) = (button.0 + button.2 / 2.0, button.1 + button.3 / 2.0);
    let pointer = |phase| ToGuest::Pointer { phase, x, y, button: PointerButton::Primary, clicks: 1, modifiers: Modifiers::NONE };
    turn(guest, host, &[pointer(PointerPhase::Down), pointer(PointerPhase::Up)]);
}

fn context() -> Context {
    Context { filters: vec![Filter { field: "stage".into(), value: "doing".into() }], record: Some("task_7".into()) }
}

#[test]
fn the_page_reaches_the_view_and_the_request_crosses_as_a_question() {
    let mut guest = Guest::over(review);
    let mut host = TestHost::new();
    turn(&mut guest, &mut host, &[offer(SIGNED)]);
    assert_eq!(host.session().unwrap().capabilities, SIGNED);
    assert!(labels(&host).contains(&"open: no record".to_string()), "{:?}", labels(&host));

    let rows = Rows {
        view: "work".into(),
        columns: vec!["title".into()],
        rows: vec![Row { record: "task_7".into(), cells: vec![Some("Ship".into())] }],
    };
    turn(&mut guest, &mut host, &[ToGuest::Context(context()), ToGuest::Rows(rows)]);
    assert!(labels(&host).contains(&"open: task_7".to_string()), "{:?}", labels(&host));
    assert!(labels(&host).contains(&"rows: 1".to_string()), "{:?}", labels(&host));

    press_the_button(&mut guest, &mut host);
    assert_eq!(
        host.requests(),
        [Requested { request: 1, action: "send_to_review".into(), record: Some("task_7".into()) }],
        "the request names the declared action and the open record"
    );
    assert!(labels(&host).contains(&"unanswered".to_string()), "nothing is written by asking");

    turn(&mut guest, &mut host, &[ToGuest::Answer { request: 1, outcome: Outcome::Held }]);
    assert!(labels(&host).contains(&"held".to_string()), "{:?}", labels(&host));
}

#[test]
fn a_shell_that_grants_no_requests_is_answered_inside_the_guest() {
    let mut guest = Guest::over(review);
    let mut host = TestHost::new();
    let unsigned = Capabilities::SCENE | Capabilities::POINTER | Capabilities::ACCESSIBILITY;
    turn(&mut guest, &mut host, &[offer(unsigned)]);
    press_the_button(&mut guest, &mut host);
    assert!(host.requests().is_empty(), "an ungranted request never crosses");
    assert!(
        labels(&host).iter().any(|label| label.starts_with("refused: ")),
        "the view hears the refusal: {:?}",
        labels(&host)
    );
}

#[test]
fn the_signature_kinds_of_an_ungranted_capability_are_dropped() {
    let mut guest = Guest::over(review);
    let mut host = TestHost::new();
    turn(&mut guest, &mut host, &[offer(Capabilities::SCENE | Capabilities::ACCESSIBILITY)]);
    turn(&mut guest, &mut host, &[ToGuest::Context(context())]);
    assert!(labels(&host).contains(&"open: no record".to_string()), "{:?}", labels(&host));
}

#[test]
fn the_same_context_twice_answers_nothing() {
    let mut guest = Guest::over(review);
    let mut host = TestHost::new();
    turn(&mut guest, &mut host, &[offer(SIGNED), ToGuest::Context(context())]);
    let bytes = protocol::encode(&Message::ToGuest(ToGuest::Context(context())));
    assert!(guest.call(&bytes).is_empty(), "a turn that changed nothing answers nothing");
    let _ = ToHost::Pace { animating: false };
}
