//! Where an app is in the update cycle, and when it asks the feed again.
//!
//! Nothing here names a view. It is a state machine and a cadence; the
//! pill of bunny-ui and an app's own view both read it.

use std::time::Duration;

use bunny_ui_release::{InstallProgress, Installed, Offer, Version};

/// Where an app is in the update cycle.
#[derive(Debug, Clone, Default)]
pub enum UpdateState {
    /// Nothing to show: the first state, and the one after a check that
    /// found nothing. It looks the same as "up to date" on purpose: a
    /// build that is current has nothing to say in each window.
    #[default]
    Idle,
    /// A newer build, with all that installing it needs.
    Available(Offer),
    /// The download, the check or the install, and which of the three, so a
    /// slow download reads as moving and not as stuck.
    Installing(InstallProgress),
    /// Installed. The process is still the old build, so a restart is
    /// what is left.
    ReadyToRestart {
        version: Version,
        /// What the install said, because "restart" means three things:
        /// start this path again, start another path, or only quit and
        /// let the Windows helper finish.
        outcome: Box<Installed>,
    },
    /// The last attempt failed. The detail is in the log, not on screen:
    /// a transport error asks the user to debug the release pipeline.
    Failed,
    /// This build is below the floor of its channel: not an offer, a stop.
    ///
    /// A view that shows it must not go away, and the app goes read-only:
    /// a build is retired because it is wrong. The app must never refuse
    /// to start: a window that opens can still show the button that
    /// fixes it.
    Retired {
        minimum: Version,
        /// The way out, when this platform has one.
        upgrade: Option<Box<Offer>>,
    },
}

impl UpdateState {
    /// Whether there is anything to show. Only [`UpdateState::Idle`] is
    /// nothing at all.
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        !matches!(self, UpdateState::Idle)
    }

    /// Whether this build is stopped: the one question an app asks to go
    /// read-only.
    #[must_use]
    pub const fn is_retired(&self) -> bool {
        matches!(self, UpdateState::Retired { .. })
    }

    /// Whether a click does something. An install in flight is not: a
    /// second click would start a second download, and a person clicks
    /// again at a bar that seems stuck.
    #[must_use]
    pub const fn is_actionable(&self) -> bool {
        matches!(
            self,
            UpdateState::Available(_)
                | UpdateState::ReadyToRestart { .. }
                | UpdateState::Failed
                | UpdateState::Retired { upgrade: Some(_), .. }
        )
    }

    /// The build a click installs, when the state offers one.
    #[must_use]
    pub fn offer(&self) -> Option<&Offer> {
        match self {
            UpdateState::Available(offer) => Some(offer),
            UpdateState::Retired { upgrade: Some(offer), .. } => Some(offer),
            _ => None,
        }
    }
}

/// How often the feed is asked while the app runs.
pub const POLL_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// The wait before the first check: a check at start competes with the
/// first paint, and twenty seconds late to an offer is better than a slow
/// start.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);

/// How old the last answer must be before a return to the app asks
/// again. A release published while a build sits open would otherwise
/// wait up to [`POLL_INTERVAL`].
pub const RETURN_RECHECK_AFTER: Duration = Duration::from_secs(30 * 60);

/// When to ask again after `state`, or never in this run.
///
/// Nothing yet, or an offer not taken: keep asking, so a retirement still
/// reaches someone who ignores an update. All the rest stops: an install
/// in flight, a restart that waits, a failure (a timer that offers again
/// is a loop the user cannot leave), a retirement.
#[must_use]
pub const fn next_check(state: &UpdateState) -> Option<Duration> {
    match state {
        UpdateState::Idle | UpdateState::Available(_) => Some(POLL_INTERVAL),
        UpdateState::Installing(_) | UpdateState::ReadyToRestart { .. } | UpdateState::Failed | UpdateState::Retired { .. } => {
            None
        }
    }
}

/// Whether a return to the app (a window brought forward, a machine that
/// woke) asks the feed now: only from [`UpdateState::Idle`], and only when
/// the last answer is `since_asked` old or more than
/// [`RETURN_RECHECK_AFTER`].
#[must_use]
pub const fn ask_on_return(state: &UpdateState, since_asked: Duration) -> bool {
    matches!(state, UpdateState::Idle) && since_asked.as_nanos() >= RETURN_RECHECK_AFTER.as_nanos()
}
