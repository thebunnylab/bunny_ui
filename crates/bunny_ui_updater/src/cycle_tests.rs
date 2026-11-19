use std::time::Duration;

use bunny_ui_release::{Format, InstallProgress, Installed, Offer, ReleaseAsset, Transfer};

use crate::cycle::*;

fn offer() -> Offer {
    Offer {
        version: "1.1.0".parse().unwrap(),
        asset: ReleaseAsset { url: "u".into(), signature: "s".into(), format: Format::Dmg, size: None, sha256: None },
        notes: None,
    }
}

fn every_state() -> Vec<UpdateState> {
    vec![
        UpdateState::Idle,
        UpdateState::Available(offer()),
        UpdateState::Installing(InstallProgress::Downloading(Transfer::new(1, Some(2)))),
        UpdateState::Installing(InstallProgress::Verifying),
        UpdateState::ReadyToRestart { version: "1.1.0".parse().unwrap(), outcome: Box::new(Installed::ReadyOnRestart) },
        UpdateState::Failed,
        UpdateState::Retired { minimum: "1.0.0".parse().unwrap(), upgrade: Some(Box::new(offer())) },
        UpdateState::Retired { minimum: "1.0.0".parse().unwrap(), upgrade: None },
    ]
}

#[test]
fn idle_is_the_only_state_with_nothing_to_show() {
    for state in every_state() {
        assert_eq!(state.is_visible(), !matches!(state, UpdateState::Idle), "{state:?}");
    }
}

#[test]
fn a_click_does_something_except_while_installing_or_with_no_way_out() {
    for state in every_state() {
        let expected = !matches!(
            state,
            UpdateState::Idle | UpdateState::Installing(_) | UpdateState::Retired { upgrade: None, .. }
        );
        assert_eq!(state.is_actionable(), expected, "{state:?}");
    }
}

#[test]
fn the_offer_is_there_to_take_or_to_escape_with() {
    assert!(UpdateState::Available(offer()).offer().is_some());
    assert!(UpdateState::Retired { minimum: "1.0.0".parse().unwrap(), upgrade: Some(Box::new(offer())) }.offer().is_some());
    assert!(UpdateState::Retired { minimum: "1.0.0".parse().unwrap(), upgrade: None }.offer().is_none());
    assert!(UpdateState::Failed.offer().is_none());
    assert!(UpdateState::Retired { minimum: "1.0.0".parse().unwrap(), upgrade: None }.is_retired());
}

#[test]
fn the_cadence_keeps_asking_only_while_nothing_or_an_offer_is_shown() {
    for state in every_state() {
        let keeps = matches!(state, UpdateState::Idle | UpdateState::Available(_));
        assert_eq!(next_check(&state), keeps.then_some(POLL_INTERVAL), "{state:?}");
    }
    assert!(FIRST_CHECK_DELAY < POLL_INTERVAL);
}

#[test]
fn a_return_asks_only_from_idle_and_only_when_the_answer_is_old() {
    assert!(ask_on_return(&UpdateState::Idle, RETURN_RECHECK_AFTER));
    assert!(ask_on_return(&UpdateState::Idle, Duration::MAX));
    assert!(!ask_on_return(&UpdateState::Idle, RETURN_RECHECK_AFTER - Duration::from_secs(1)));
    for state in every_state().into_iter().skip(1) {
        assert!(!ask_on_return(&state, Duration::MAX), "{state:?}");
    }
}
