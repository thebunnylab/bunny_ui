use std::time::{Duration, UNIX_EPOCH};

use bunny_ui_release::{UpdateError, Version};

use crate::log::{Failure, LIMIT, UpdateLog, line, newest_lines, utc};

#[test]
fn the_time_is_rfc_3339_in_utc() {
    for (seconds, text) in [
        (0, "1970-01-01T00:00:00Z"),
        (951_782_400, "2000-02-29T00:00:00Z"),
        (1_763_583_204, "2025-11-19T20:13:24Z"),
        (4_102_444_799, "2099-12-31T23:59:59Z"),
    ] {
        assert_eq!(utc(UNIX_EPOCH + Duration::from_secs(seconds)), text);
    }
}

#[test]
fn a_line_names_the_time_the_build_and_what_failed() {
    let running = Version::new(0, 4, 1);
    let check = UpdateError::Transport("no route\nto host".into());
    assert_eq!(
        line("T", &running, Failure::Check(&check)),
        "T 0.4.1 check failed: update transport error: no route to host"
    );
    let to = Version::new(0, 4, 2);
    assert_eq!(
        line("T", &running, Failure::Install { to: &to, error: &UpdateError::BadSignature }),
        "T 0.4.1 install of 0.4.2 failed: signature verification failed"
    );
}

#[test]
fn the_oldest_whole_lines_go_first() {
    assert_eq!(newest_lines("a\nb\nc\n", 100), "a\nb\nc\n");
    assert_eq!(newest_lines("aaa\nbbb\nccc\n", 6), "ccc\n");
    assert_eq!(newest_lines("aaa\nbbb\nccc\n", 7), "ccc\n");
    // a cut at the start of a line keeps it whole
    assert_eq!(newest_lines("aaa\nbbb\nccc\n", 8), "bbb\nccc\n");
    assert_eq!(newest_lines("aaa\nbbb\nccc\n", 9), "bbb\nccc\n");
    assert_eq!(newest_lines("aaa\nbbb\nccc\n", 11), "bbb\nccc\n");
    // a cut inside a character moves to the next one
    assert_eq!(newest_lines("ééé\nok\n", 6), "ok\n");
}

#[test]
fn the_file_grows_by_lines_and_stays_bounded() {
    let folder = std::env::temp_dir().join(format!("bunny-updater-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    let log = UpdateLog::new(folder.join("deep").join("update.log"));
    let running = Version::new(1, 0, 0);
    let error = UpdateError::Transport("x".repeat(200));
    log.record(&running, Failure::Check(&error));
    let one = std::fs::read_to_string(log.path()).unwrap();
    assert_eq!(one.lines().count(), 1);
    assert!(one.ends_with(&format!("1.0.0 check failed: update transport error: {}\n", "x".repeat(200))));
    for _ in 0..600 {
        log.record(&running, Failure::Check(&error));
    }
    let text = std::fs::read_to_string(log.path()).unwrap();
    assert!(text.len() <= LIMIT, "{} bytes", text.len());
    assert!(text.lines().all(|line| line.ends_with(&"x".repeat(200))), "only whole lines stay");
    std::fs::remove_dir_all(&folder).unwrap();
}
