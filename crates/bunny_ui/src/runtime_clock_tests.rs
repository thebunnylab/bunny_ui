//! Injected monotonic observations exercise the production wall-driver path.
use super::Runtime;
use std::time::{Duration, Instant};

fn fresh(test: fn()) {
    // Every case owns a fresh thread-local executor, without sleeps or a reset API.
    std::thread::spawn(test).join().expect("clock regression");
}
fn injected_epoch() -> Instant {
    // Test observations are ahead of ambient time. Sleep's production-time
    // synchronization is therefore an out-of-order observation and cannot
    // perturb these exact deterministic intervals.
    Instant::now() + Duration::from_hours(24)
}
fn at(base: Instant, millis: u64) -> Instant {
    base + Duration::from_millis(millis)
}
fn wait_for(millis: u64) -> motor::task::Spawned {
    let task = motor::task::spawn(async move {
        motor::task::sleep(Duration::from_millis(millis)).await;
    });
    motor::task::poll_ready();
    task
}
fn remaining(seconds: f64) {
    let left = motor::task::next_timer_in().expect("sleeper must still be pending");
    assert!(
        (left - seconds).abs() < 1e-9,
        "expected {seconds}, observed {left}"
    );
}
#[test]
fn three_wall_drivers_count_an_overlapping_interval_once() {
    fresh(|| {
        let base = injected_epoch();
        let scenes = [
            Runtime::scene("a"),
            Runtime::scene("b"),
            Runtime::scene("c"),
        ];
        for scene in &scenes {
            scene.drive_tasks_at(base);
        }
        let sleeper = wait_for(300);
        for scene in &scenes {
            scene.advance_tasks_at(at(base, 100));
            remaining(0.2);
        }
        for scene in &scenes {
            scene.advance_tasks_at(at(base, 299));
            remaining(0.001);
        }
        assert!(scenes[2].advance_tasks_at(at(base, 300)));
        assert!(!scenes[0].advance_tasks_at(at(base, 300)));
        assert_eq!(motor::task::next_timer_in(), None);
        motor::task::poll_ready();
        drop(sleeper);
    });
}
#[test]
fn late_registration_keeps_the_existing_deadline_and_removal_keeps_the_clock() {
    fresh(|| {
        let base = injected_epoch();
        let first = Runtime::scene("first");
        first.drive_tasks_at(base);
        let sleeper = wait_for(300);
        first.advance_tasks_at(at(base, 100));
        remaining(0.2);
        let second = Runtime::scene("second");
        second.drive_tasks_at(at(base, 150));
        remaining(0.15);
        first.advance_tasks_at(at(base, 200));
        second.advance_tasks_at(at(base, 200));
        remaining(0.1);
        drop(first);
        second.advance_tasks_at(at(base, 299));
        remaining(0.001);
        assert!(second.advance_tasks_at(at(base, 300)));
        drop(sleeper);
    });
}
#[test]
fn backwards_or_duplicate_observations_do_not_count_recovery_twice() {
    fresh(|| {
        let base = injected_epoch();
        let scene = Runtime::new();
        scene.drive_tasks_at(base);
        let sleeper = wait_for(300);
        scene.advance_tasks_at(at(base, 200));
        remaining(0.1);
        assert!(!scene.advance_tasks_at(at(base, 100)));
        remaining(0.1);
        assert!(!scene.advance_tasks_at(at(base, 200)));
        remaining(0.1);
        scene.advance_tasks_at(at(base, 250));
        remaining(0.05);
        drop(sleeper);
        assert_eq!(motor::task::next_timer_in(), None);
    });
}
#[test]
fn a_new_scene_synchronizes_before_its_new_timer_is_created() {
    fresh(|| {
        let base = injected_epoch();
        let first = Runtime::scene("first");
        first.drive_tasks_at(base);
        first.advance_tasks_at(at(base, 400));
        let second = Runtime::scene("second");
        second.drive_tasks_at(at(base, 500));
        let sleeper = wait_for(300);
        first.advance_tasks_at(at(base, 600));
        second.advance_tasks_at(at(base, 600));
        remaining(0.2);
        second.advance_tasks_at(at(base, 799));
        remaining(0.001);
        assert!(first.advance_tasks_at(at(base, 801)));
        drop(sleeper);
    });
}
#[test]
fn manual_ticks_remain_deterministic_and_wall_driver_registration_is_idempotent() {
    fresh(|| {
        let scene = Runtime::new();
        let sleeper = wait_for(300);
        assert!(!scene.advance_tasks_to_now());
        remaining(0.3);
        scene.tick(0.1);
        remaining(0.2);
        scene.tick_clocked(0.05, 10.0);
        remaining(0.15);
        drop(sleeper);
        let base = injected_epoch();
        scene.drive_tasks_at(base);
        let sleeper = wait_for(300);
        scene.drive_tasks_at(at(base, 100));
        remaining(0.2);
        scene.drive_tasks_at(at(base, 100));
        remaining(0.2);
        drop(sleeper);
    });
}

#[test]
fn the_last_wall_driver_releases_automatic_time_and_a_new_host_starts_fresh() {
    fresh(|| {
        let base = injected_epoch();
        let first = Runtime::new();
        first.drive_tasks_at(base);
        let second = Runtime::scene("other");
        second.drive_tasks_at(base);
        let sleeper = wait_for(300);
        first.advance_tasks_at(at(base, 100));
        drop(first);
        second.advance_tasks_at(at(base, 200));
        remaining(0.1);
        drop(second);
        drop(sleeper);
        let manual = Runtime::new();
        let sleeper = wait_for(300);
        manual.tick(0.1);
        remaining(0.2);
        let next = Runtime::scene("new-host");
        next.drive_tasks_at(at(base, 10_000));
        remaining(0.2);
        next.advance_tasks_at(at(base, 10_100));
        remaining(0.1);
        drop(sleeper);
    });
}
