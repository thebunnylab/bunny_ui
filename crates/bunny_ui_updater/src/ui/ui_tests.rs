use std::sync::Arc;

use bunny_ui_core::layout::{DrawCommand, Proposal, Rect, Size};
use bunny_ui_core::prelude::Locale;
use bunny_ui_core::runtime::Runtime;
use bunny_ui_core::theme::Theme;
use bunny_ui_release::{Format, InstallProgress, Installed, Offer, ProductId, ReleaseAsset, Transfer, UpdateError};

use super::model::{Reading, Tone, label, megabytes, reading};
use super::words::{WORDS, Word};
use bunny_ui_core::prelude::ViewExt;

use super::{HEIGHT, update_pill};
use crate::{Feed, Fetch, HttpReply, Schedule, UpdateState, UpdaterConfig};

fn offer() -> Offer {
    Offer {
        version: "1.1.0".parse().unwrap(),
        asset: ReleaseAsset { url: "u".into(), signature: "s".into(), format: Format::Dmg, size: None, sha256: None },
        notes: None,
    }
}

fn english() -> bunny_ui_core::catalog::Strings<Word, 12> {
    WORDS.pick(&Locale::new("en"))
}

// ---- the words ----

#[test]
fn every_language_has_every_word_with_the_same_names_to_fill() {
    let source = &WORDS.tables()[0];
    assert_eq!(WORDS.tables().len(), 16);
    for table in WORDS.tables() {
        for (word, (entry, english)) in Word::ALL.iter().zip(table.entries.iter().zip(source.entries.iter())) {
            assert!(!entry.is_empty(), "{} has no {word:?}", table.tag);
            for name in ["{version}", "{percent}", "{size}", "{minimum}"] {
                assert_eq!(entry.contains(name), english.contains(name), "{} {word:?} and {name}", table.tag);
            }
        }
    }
}

#[test]
fn the_locale_picks_the_language() {
    let state = UpdateState::Available(offer());
    assert_eq!(label(&state, &WORDS.pick(&Locale::new("pt-BR"))), "Atualizar para 1.1.0");
    assert_eq!(label(&state, &WORDS.pick(&Locale::new("pt-PT"))), "Atualizar para 1.1.0");
    assert_eq!(label(&state, &WORDS.pick(&Locale::new("ja"))), "1.1.0 にアップデート");
    assert_eq!(label(&state, &WORDS.pick(&Locale::new("zh-TW"))), "更新至 1.1.0");
    assert_eq!(label(&state, &WORDS.pick(&Locale::new("xx"))), "Update to 1.1.0", "an unknown language reads English");
}

// ---- the reading of each state ----

#[test]
fn idle_shows_nothing() {
    assert_eq!(reading(&UpdateState::Idle, &english()), None);
}

#[test]
fn an_offer_is_an_action() {
    let shown = reading(&UpdateState::Available(offer()), &english()).unwrap();
    assert_eq!(shown, Reading { label: "Update to 1.1.0".into(), tone: Tone::Action, filled: None, clickable: true });
}

#[test]
fn a_download_says_its_percent_and_fills_the_bar() {
    let half = UpdateState::Installing(InstallProgress::Downloading(Transfer::new(50, Some(100))));
    let shown = reading(&half, &english()).unwrap();
    assert_eq!((shown.label.as_str(), shown.filled, shown.clickable), ("Downloading 50%", Some(0.5), false));
    assert_eq!(shown.tone, Tone::Quiet, "nothing to click while it runs");
}

#[test]
fn a_download_of_no_known_size_counts_its_megabytes_and_has_no_bar() {
    let unknown = UpdateState::Installing(InstallProgress::Downloading(Transfer::new(12_345_678, None)));
    let shown = reading(&unknown, &english()).unwrap();
    assert_eq!((shown.label.as_str(), shown.filled), ("Downloading 12.3 MB", None));
    assert_eq!(megabytes(Transfer::new(0, None)), "0.0 MB");
    assert_eq!(megabytes(Transfer::new(99_999, None)), "0.0 MB");
    assert_eq!(megabytes(Transfer::new(1_000_000_000, None)), "1000.0 MB");
}

#[test]
fn the_steps_after_the_download_keep_the_bar_full() {
    for (stage, words) in [(InstallProgress::Verifying, "Verifying…"), (InstallProgress::Applying, "Installing…")] {
        let shown = reading(&UpdateState::Installing(stage), &english()).unwrap();
        assert_eq!((shown.label.as_str(), shown.filled), (words, Some(1.0)));
    }
}

#[test]
fn an_installed_build_asks_for_a_restart_or_on_windows_a_quit() {
    let ready = |outcome| UpdateState::ReadyToRestart { version: "1.1.0".parse().unwrap(), outcome: Box::new(outcome) };
    let restart = reading(&ready(Installed::ReadyOnRestart), &english()).unwrap();
    assert_eq!((restart.label.as_str(), restart.tone, restart.clickable), ("Restart to update", Tone::Action, true));
    assert_eq!(reading(&ready(Installed::PendingHelper), &english()).unwrap().label, "Quit to finish updating");
}

#[test]
fn a_failure_is_quiet_but_a_click_shows_the_log() {
    let shown = reading(&UpdateState::Failed, &english()).unwrap();
    assert_eq!((shown.label.as_str(), shown.tone, shown.clickable), ("Update failed", Tone::Quiet, true));
}

#[test]
fn a_retirement_names_the_version_to_get() {
    let way_out = UpdateState::Retired { minimum: "1.0.0".parse().unwrap(), upgrade: Some(Box::new(offer())) };
    let shown = reading(&way_out, &english()).unwrap();
    assert_eq!((shown.label.as_str(), shown.tone), ("Update to 1.0.0 or later to keep working", Tone::Action));
    let wall = UpdateState::Retired { minimum: "1.0.0".parse().unwrap(), upgrade: None };
    let shown = reading(&wall, &english()).unwrap();
    assert_eq!((shown.label.as_str(), shown.tone, shown.clickable), ("This build is no longer supported (1.0.0 or later)", Tone::Quiet, false));
}

// ---- the view ----

struct Offline;

impl Fetch for Offline {
    fn get(&self, _url: &str) -> Result<HttpReply, UpdateError> {
        Err(UpdateError::Transport("offline".into()))
    }
}

fn with_service(state: UpdateState) {
    crate::service::stop();
    let config =
        UpdaterConfig::new(Feed::Server("https://api".into()), ProductId::parse("app").unwrap(), "1.0.0".parse().unwrap());
    crate::start_with(config, Arc::new(Offline), Schedule::Manual);
    crate::state().unwrap().set(state);
}

fn draw(locale: &str) -> (Size, Vec<DrawCommand>) {
    let runtime = Runtime::new().theme(Theme::light());
    runtime.set_locale(Some(Locale::new(locale)));
    let laid = runtime.layout(&update_pill(), Proposal { width: Some(300.0), height: None });
    (laid.size, laid.display.iter().cloned().collect())
}

fn words_drawn(display: &[DrawCommand]) -> Vec<(String, bunny_ui_core::layout::Color)> {
    display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { content, range, color, .. } => Some((content[range.0..range.1].to_owned(), *color)),
            _ => None,
        })
        .collect()
}

fn fills(display: &[DrawCommand]) -> Vec<(Rect, bunny_ui_core::layout::Color)> {
    display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { rect, color, .. } => Some((*rect, *color)),
            _ => None,
        })
        .collect()
}

#[test]
fn with_no_updater_the_pill_takes_no_room() {
    crate::service::stop();
    let (size, display) = draw("en");
    assert_eq!((size.width, size.height), (0.0, 0.0));
    assert!(display.is_empty());
}

#[test]
fn an_offer_draws_the_words_in_the_accent_on_the_control_ground() {
    with_service(UpdateState::Available(offer()));
    let (size, display) = draw("en");
    let theme = Theme::light();
    assert_eq!(size.height, HEIGHT);
    assert_eq!(words_drawn(&display), [("Update to 1.1.0".to_owned(), theme.accent)]);
    assert!(fills(&display).iter().any(|(rect, color)| *color == theme.control && rect.size.width == size.width));
    let (_, portuguese) = draw("pt-BR");
    assert_eq!(words_drawn(&portuguese)[0].0, "Atualizar para 1.1.0");
    crate::service::stop();
}

#[test]
fn a_download_fills_the_pill_itself_and_the_pill_keeps_its_size() {
    with_service(UpdateState::Available(offer()));
    let (before, _) = draw("en");
    crate::state().unwrap().set(UpdateState::Installing(InstallProgress::Downloading(Transfer::new(1, Some(4)))));
    let (during, display) = draw("en");
    assert_eq!(during.height, before.height, "the fill is the pill's own ground");
    let theme = Theme::light();
    let well = fills(&display).into_iter().find(|(_, color)| *color == theme.control).expect("the well");
    let wash = bunny_ui_core::layout::Color { a: 70, ..theme.accent };
    let fill = fills(&display).into_iter().find(|(_, color)| *color == wash).expect("the fill");
    assert_eq!((well.0.size.width, well.0.size.height), (during.width, HEIGHT));
    let share = fill.0.size.width / well.0.size.width;
    assert!((share - 0.25).abs() < 0.01, "a quarter downloaded fills a quarter: {share}");
    assert_eq!(fill.0.origin.x, well.0.origin.x, "it fills from the leading edge");
    // the words paint after the ground, over it
    let words_at = display.iter().position(|command| matches!(command, DrawCommand::TextLine { .. })).unwrap();
    let fill_at = display.iter().position(|command| matches!(command, DrawCommand::FillRect { color, .. } if *color == wash)).unwrap();
    assert!(fill_at < words_at);
    assert_eq!(words_drawn(&display)[0], ("Downloading 25%".to_owned(), theme.fg_secondary));
    crate::service::stop();
}

#[test]
fn a_squeezed_pill_ends_in_an_ellipsis_on_one_line() {
    with_service(UpdateState::Retired { minimum: "1.0.1".parse().unwrap(), upgrade: Some(Box::new(offer())) });
    let runtime = Runtime::new().theme(Theme::light());
    let laid = runtime.layout(&update_pill().frame_width(120.0), Proposal { width: Some(120.0), height: None });
    let display: Vec<DrawCommand> = laid.display.iter().cloned().collect();
    let lines = words_drawn(&display);
    assert_eq!(lines.len(), 1, "one line: {lines:?}");
    assert!(lines[0].0.ends_with('…'), "{lines:?}");
    assert_eq!(laid.size.height, HEIGHT);
    crate::service::stop();
}
