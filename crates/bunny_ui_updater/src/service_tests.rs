//! The service on this test's own UI thread: its tasks driven by hand,
//! its clock moved by hand, its network and install faked.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bunny_ui_core::task;
use bunny_ui_formats::base64;
use bunny_ui_formats::ed25519::SigningKey;
use bunny_ui_release::{InstallProgress, Installed, ProductId, Transfer, UpdateError};

use crate::apply::ApplyStrategy;
use crate::{Feed, Fetch, HttpReply, Schedule, UpdateState, UpdaterConfig, Verified, service};

/// Answers each check with the next reply, and serves one artifact.
#[derive(Default)]
struct Feed2 {
    replies: Mutex<VecDeque<HttpReply>>,
    artifact: Vec<u8>,
    asked: Mutex<usize>,
}

impl Fetch for Feed2 {
    fn get(&self, _url: &str) -> Result<HttpReply, UpdateError> {
        *self.asked.lock().unwrap() += 1;
        self.replies.lock().unwrap().pop_front().ok_or_else(|| UpdateError::Transport("no route".into()))
    }

    fn download(&self, _url: &str, to: &Path, received: &mut dyn FnMut(Transfer)) -> Result<u16, UpdateError> {
        std::fs::write(to, &self.artifact).unwrap();
        received(Transfer::new(self.artifact.len() as u64, Some(self.artifact.len() as u64)));
        Ok(200)
    }
}

/// Installs nothing, and says the build is in place.
struct Pretend;

impl ApplyStrategy for Pretend {
    fn apply(&self, artifact: &Verified) -> Result<Installed, UpdateError> {
        assert!(artifact.path().exists());
        Ok(Installed::RelaunchAt(artifact.path().to_path_buf()))
    }
}

fn key() -> SigningKey {
    SigningKey::from_seed(&[5; 32])
}

fn release(signature: &str) -> HttpReply {
    HttpReply::new(
        200,
        format!(
            r#"{{"product":"app","version":"1.1.0","url":"https://dl/a.dmg","signature":"{signature}","format":"dmg"}}"#
        ),
    )
}

fn start(name: &str, replies: Vec<HttpReply>, artifact: &[u8], schedule: Schedule) -> Arc<Feed2> {
    service::stop();
    let cache = std::env::temp_dir().join(format!("bunny-service-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    let config = UpdaterConfig::new(Feed::Server("https://api".into()), ProductId::parse("app").unwrap(), "1.0.0".parse().unwrap())
        .cache(&cache)
        .key(key().verifying_key());
    let feed = Arc::new(Feed2 { replies: Mutex::new(replies.into()), artifact: artifact.to_vec(), ..Feed2::default() });
    service::start_with_strategy(config, feed.clone(), Arc::new(Pretend), schedule);
    feed
}

/// Runs this thread's tasks until `done`, or fails after five seconds.
fn drive_until(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        task::poll_ready();
        assert!(start.elapsed() < Duration::from_secs(5), "never: {what}; the state is {:?}", service::current());
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn asked(feed: &Feed2) -> usize {
    *feed.asked.lock().unwrap()
}

#[test]
fn no_updater_has_no_state_and_a_click_does_nothing() {
    service::stop();
    assert!(!service::is_running());
    assert!(service::state().is_none());
    assert!(matches!(service::current(), UpdateState::Idle));
    service::click();
    service::check_now();
    task::poll_ready();
}

#[test]
fn the_schedule_waits_then_asks_then_asks_again_much_later() {
    let feed = start("schedule", vec![release("c2ln"), release("c2ln")], b"", Schedule::Automatic);
    task::poll_ready();
    assert_eq!(asked(&feed), 0, "nothing at start: the first paint goes first");
    task::advance(crate::FIRST_CHECK_DELAY.as_secs_f64());
    drive_until("the first check lands", || matches!(service::current(), UpdateState::Available(_)));
    assert_eq!(asked(&feed), 1);
    task::advance(60.0);
    task::poll_ready();
    assert_eq!(asked(&feed), 1, "an offer is not asked again a minute later");
    task::advance(crate::POLL_INTERVAL.as_secs_f64());
    drive_until("the second check", || asked(&feed) == 2);
    service::stop();
}

#[test]
fn a_manual_schedule_asks_only_when_told() {
    let feed = start("manual", vec![HttpReply::new(204, "")], b"", Schedule::Manual);
    task::advance(crate::POLL_INTERVAL.as_secs_f64() * 2.0);
    task::poll_ready();
    assert_eq!(asked(&feed), 0);
    service::check_now();
    drive_until("the check", || asked(&feed) == 1);
    drive_until("the check lands", || !service::with_checking());
    assert!(matches!(service::current(), UpdateState::Idle), "up to date looks like nothing");
    service::stop();
}

#[test]
fn a_failed_check_goes_to_the_log_and_not_to_the_screen() {
    let feed = start("check-fails", vec![], b"", Schedule::Manual);
    service::check_now();
    drive_until("the check", || asked(&feed) == 1);
    drive_until("the check lands", || !service::with_checking());
    assert!(matches!(service::current(), UpdateState::Idle));
    let log = service::log_path().unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("1.0.0 check failed: update transport error: no route"), "{text}");
    service::stop();
}

#[test]
fn a_click_installs_the_offer_and_waits_for_a_restart() {
    let artifact = b"the new build".to_vec();
    let signature = base64::encode(&key().sign(&artifact));
    let _feed = start("install", vec![release(&signature)], &artifact, Schedule::Manual);
    service::check_now();
    drive_until("the offer", || matches!(service::current(), UpdateState::Available(_)));
    let seen = Arc::new(Mutex::new(Vec::new()));
    service::click();
    assert!(matches!(service::current(), UpdateState::Installing(InstallProgress::Downloading(_))), "at once");
    drive_until("the install", || {
        let now = service::current();
        if let UpdateState::Installing(stage) = &now {
            seen.lock().unwrap().push(*stage);
        }
        matches!(now, UpdateState::ReadyToRestart { .. })
    });
    let UpdateState::ReadyToRestart { version, outcome } = service::current() else { unreachable!() };
    assert_eq!(version.to_string(), "1.1.0");
    assert!(matches!(*outcome, Installed::RelaunchAt(_)));
    // a second click now restarts, and a check that lands does not undo it
    service::check_now();
    task::poll_ready();
    assert!(matches!(service::current(), UpdateState::ReadyToRestart { .. }));
    service::stop();
}

#[test]
fn a_bad_signature_fails_the_install_and_logs_why() {
    let forged = base64::encode(&SigningKey::from_seed(&[9; 32]).sign(b"the new build"));
    let _feed = start("install-bad", vec![release(&forged)], b"the new build", Schedule::Manual);
    service::check_now();
    drive_until("the offer", || matches!(service::current(), UpdateState::Available(_)));
    service::install();
    drive_until("the failure", || matches!(service::current(), UpdateState::Failed));
    let text = std::fs::read_to_string(service::log_path().unwrap()).unwrap();
    assert!(text.contains("install of 1.1.0 failed: signature verification failed"), "{text}");
    service::stop();
}

#[test]
fn a_restart_asks_the_app_first() {
    let artifact = b"build".to_vec();
    let signature = base64::encode(&key().sign(&artifact));
    let _feed = start("restart", vec![release(&signature)], &artifact, Schedule::Manual);
    let mut ended = None;
    service::restart_with(|outcome| ended = Some(outcome.clone()));
    assert!(ended.is_none(), "nothing to restart yet");
    service::check_now();
    drive_until("the offer", || matches!(service::current(), UpdateState::Available(_)));
    service::install();
    drive_until("the install", || matches!(service::current(), UpdateState::ReadyToRestart { .. }));
    service::set_before_restart(|| false);
    service::restart_with(|outcome| ended = Some(outcome.clone()));
    assert!(ended.is_none(), "the app said no: unsaved work");
    assert!(matches!(service::current(), UpdateState::ReadyToRestart { .. }), "the restart still waits");
    service::set_before_restart(|| true);
    service::restart_with(|outcome| ended = Some(outcome.clone()));
    assert!(matches!(ended, Some(Installed::RelaunchAt(_))));
    service::stop();
}

#[test]
fn a_second_start_changes_nothing() {
    let feed = start("twice", vec![HttpReply::new(204, "")], b"", Schedule::Manual);
    let first = service::state().unwrap();
    let other = UpdaterConfig::new(Feed::Server("https://other".into()), ProductId::parse("other").unwrap(), "9.0.0".parse().unwrap());
    service::start_with(other, feed.clone(), Schedule::Manual);
    assert_eq!(service::log_path().unwrap().parent().unwrap().file_name().unwrap(), format!("bunny-service-{}-twice", std::process::id()).as_str());
    first.set(UpdateState::Failed);
    assert!(matches!(service::current(), UpdateState::Failed), "the same state");
    service::stop();
}

#[test]
fn the_window_that_opens_at_launch_does_not_run_ahead_of_the_first_wait() {
    let _feed = start("launch-return", vec![], b"", Schedule::Automatic);
    // a return now, as the first window becomes active, asks nothing:
    // the start counts as the last answer
    let since = service::asked_ago().unwrap();
    assert!(!crate::ask_on_return(&service::current(), since), "asked {since:?} ago");
    service::stop();
}
