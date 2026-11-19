//! What the update control shows for a state: pure, so every state's
//! words, ink and bar are tested with no window.

use bunny_ui_core::catalog::{Strings, fill};
use bunny_ui_release::{InstallProgress, Installed, Transfer};

use super::words::Word;
use crate::cycle::UpdateState;

/// Which ink the control's words take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// A click does the update's next step: the theme's accent.
    Action,
    /// News with no step to take, or a failure: the secondary ink.
    Quiet,
}

/// What the control shows: its words, their ink, how much of the bar is
/// full, and whether a click does something.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub label: String,
    pub tone: Tone,
    /// The share of the bar, while an install runs and the share is
    /// known. The check and the install after the download keep it full:
    /// all the bytes are there, and the words say which step runs.
    pub filled: Option<f32>,
    pub clickable: bool,
}

/// The reading of `state`, or `None` for [`UpdateState::Idle`].
#[must_use]
pub fn reading(state: &UpdateState, words: &Strings<Word, { Word::ALL.len() }>) -> Option<Reading> {
    if !state.is_visible() {
        return None;
    }
    let actionable = state.is_actionable() && !matches!(state, UpdateState::Failed);
    Some(Reading {
        label: label(state, words),
        tone: if actionable { Tone::Action } else { Tone::Quiet },
        filled: match state {
            UpdateState::Installing(InstallProgress::Downloading(arrived)) => {
                arrived.percent().map(|percent| f32::from(percent) / 100.0)
            }
            UpdateState::Installing(_) => Some(1.0),
            _ => None,
        },
        // a failure is no update step, but a click shows its reason
        clickable: state.is_actionable(),
    })
}

/// The words for `state`, in the language of `words`.
#[must_use]
pub fn label(state: &UpdateState, words: &Strings<Word, { Word::ALL.len() }>) -> String {
    let with = |word: Word, name: &str, value: &str| fill(words.get(word), &[(name, value)]);
    match state {
        UpdateState::Idle => String::new(),
        UpdateState::Available(offer) => with(Word::UpdateTo, "version", &offer.version.to_string()),
        UpdateState::Installing(InstallProgress::Downloading(arrived)) => match arrived.percent() {
            Some(percent) => with(Word::DownloadingPercent, "percent", &percent.to_string()),
            None => with(Word::DownloadingSize, "size", &megabytes(*arrived)),
        },
        UpdateState::Installing(InstallProgress::Verifying) => words.get(Word::Verifying).to_owned(),
        UpdateState::Installing(InstallProgress::Applying) => words.get(Word::Installing).to_owned(),
        UpdateState::ReadyToRestart { outcome, .. } => match **outcome {
            // the helper waits for the app to QUIT: "restart" would read
            // as stuck
            Installed::PendingHelper => words.get(Word::QuitToFinish).to_owned(),
            _ => words.get(Word::RestartToUpdate).to_owned(),
        },
        UpdateState::Failed => words.get(Word::Failed).to_owned(),
        UpdateState::Retired { minimum, upgrade: Some(_) } => {
            with(Word::UpdateToKeepWorking, "minimum", &minimum.to_string())
        }
        UpdateState::Retired { minimum, upgrade: None } => {
            with(Word::NoLongerSupported, "minimum", &minimum.to_string())
        }
    }
}

/// The bytes so far in decimal megabytes, one place: precise enough to
/// be seen moving.
#[must_use]
pub fn megabytes(arrived: Transfer) -> String {
    let tenths = arrived.received() / 100_000;
    format!("{}.{} MB", tenths / 10, tenths % 10)
}
