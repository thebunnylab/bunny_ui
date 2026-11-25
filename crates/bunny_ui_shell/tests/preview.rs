//! The preview harness (T2-APP-109): a block's `make(page)` driven natively
//! on a fixture page draws what the same block draws in the sandbox on the
//! same page, and its requests are answered by the fixture, never sent.

#![cfg(feature = "engine")]

extern crate bunny_ui_core as bunny_ui;

use bunny_ui::prelude::*;
use bunny_ui_shell::guest::{Fixture, Guest, Page};
use bunny_ui_shell::host::{TestHost, native};
use bunny_ui_shell::protocol::{
    self, Capabilities, Context, Filter, Message, Modifiers, Outcome, PointerButton, PointerPhase, Role, Row, Rows,
    Scheme, ToGuest, Versions, Viewport,
};

/// A block that shows its page: the open record, the first filter, the
/// rows it reads, and the answer to the one action it asks for.
#[derive(Clone, Copy)]
struct Review {
    page: Page,
    asked: State<Option<u64>>,
}

impl Component for Review {
    fn body(self) -> impl View {
        let page = self.page;
        let record = page.record().unwrap_or_else(|| "no record".to_string());
        let filter = page.filters().first().map_or_else(|| "no filter".to_string(), |f| format!("{} = {}", f.field, f.value));
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
            text(filter),
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

const VIEWPORT: Viewport = Viewport { width: 240.0, height: 170.0, scale: 1.0, scheme: Scheme::Light };

const SIGNED: Capabilities = Capabilities::from_bits(
    Capabilities::SCENE.bits()
        | Capabilities::POINTER.bits()
        | Capabilities::ACCESSIBILITY.bits()
        | Capabilities::CONTEXT.bits()
        | Capabilities::READS.bits(),
);

fn context() -> Context {
    Context { filters: vec![Filter { field: "stage".into(), value: "doing".into() }], record: Some("task_7".into()) }
}

fn work() -> Rows {
    Rows {
        view: "work".into(),
        columns: vec!["title".into()],
        rows: vec![
            Row { record: "task_7".into(), cells: vec![Some("Ship".into())] },
            Row { record: "task_8".into(), cells: vec![None] },
        ],
    }
}

fn fixture() -> Fixture {
    Fixture::new().context(context()).rows(work())
}

fn on_the_fixture() -> Review {
    review(fixture().page())
}

/// The block in the sandbox on the fixture's page, as a shell that grants
/// its signature sends it.
fn shelled() -> (Guest<Review>, TestHost) {
    let mut guest = Guest::over(review);
    let mut host = TestHost::new();
    let mut bytes = Vec::new();
    for message in [
        ToGuest::Offer { versions: Versions::THIS, capabilities: SIGNED, viewport: VIEWPORT },
        ToGuest::Context(context()),
        ToGuest::Rows(work()),
    ] {
        bytes.extend(protocol::encode(&Message::ToGuest(message)));
    }
    host.receive(&guest.call(&bytes)).expect("the guest speaks the protocol");
    (guest, host)
}

/// A press at the centre of the block's button, where the shell lays it.
fn press(host: &TestHost) -> [ToGuest; 2] {
    let button = host.semantics().iter().find(|node| node.role == Role::Button).expect("a button").bounds;
    let (x, y) = (button.0 + button.2 / 2.0, button.1 + button.3 / 2.0);
    let pointer =
        |phase| ToGuest::Pointer { phase, x, y, button: PointerButton::Primary, clicks: 1, modifiers: Modifiers::NONE };
    [pointer(PointerPhase::Down), pointer(PointerPhase::Up)]
}

#[test]
fn a_fixture_page_draws_what_the_shell_draws_on_the_same_page() {
    let [previewed, _] = native(on_the_fixture, VIEWPORT, &[]);
    let (_, host) = shelled();
    let shelled = host.paint().expect("every image arrived");
    assert_eq!(previewed.len(), shelled.len());
    assert!(previewed == shelled, "the preview is the block, pixel for pixel");
}

#[test]
fn a_fixture_page_reads_what_the_fixture_holds() {
    let page = Fixture::new()
        .filter("stage", "doing")
        .filter("owner", "ana")
        .record("task_7")
        .rows(Rows { view: "work".into(), columns: Vec::new(), rows: Vec::new() })
        .rows(work())
        .page();
    assert_eq!(page.record().as_deref(), Some("task_7"));
    assert_eq!(page.filters().len(), 2);
    assert_eq!(page.context().filters[1], Filter { field: "owner".into(), value: "ana".into() });
    assert_eq!(page.rows("work"), Some(work()), "a view's rows replace the ones held for it");
    assert_eq!(page.rows("elsewhere"), None, "a view the fixture holds no rows of");
}

#[test]
fn a_fixture_page_answers_every_request_at_once() {
    let refusing = Fixture::new().page();
    let asked = refusing.request("send_to_review", Some("task_7"));
    assert_eq!(asked, 1);
    assert_eq!(refusing.answer(asked), Some(Outcome::Refused(Fixture::REFUSAL.to_string())));

    let holding = Fixture::new().answer(Outcome::Held).page();
    let first = holding.request("send_to_review", None);
    let second = holding.request("send_to_review", None);
    assert_ne!(first, second, "each request has a number of its own");
    assert_eq!((holding.answer(first), holding.answer(second)), (Some(Outcome::Held), Some(Outcome::Held)));
}

#[test]
fn a_press_on_a_fixture_page_draws_the_fixtures_answer_and_sends_nothing() {
    fn held() -> Review {
        review(fixture().answer(Outcome::Held).page())
    }
    fn landed() -> Review {
        review(fixture().answer(Outcome::Landed).page())
    }
    let (_, host) = shelled();
    let press = press(&host);
    let [before, held_after] = native(held, VIEWPORT, &press);
    let [_, landed_after] = native(landed, VIEWPORT, &press);
    assert!(before != held_after, "the press redraws the block with its answer");
    assert!(held_after != landed_after, "the answer drawn is the fixture's");
    assert!(host.requests().is_empty(), "nothing crossed to a shell");
}
