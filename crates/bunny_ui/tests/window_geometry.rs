use bunny_ui_core::window::{
    DesktopPosition, DesktopSpace, Lifecycle, WindowError, WindowGeometry, WindowPlacement,
    WindowSize,
};
use std::{cell::RefCell, rc::Rc};

fn geometry(width: f64) -> WindowGeometry {
    WindowGeometry::new(WindowSize::new(width, 200.0).unwrap(), None, 1.5).unwrap()
}

#[test]
fn geometry_values_reject_invalid_persisted_input() {
    for value in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -1.0,
        0.0,
        f64::from(i32::MAX) + 1.0,
    ] {
        assert!(matches!(
            WindowSize::new(value, 100.0),
            Err(WindowError::InvalidGeometry(_))
        ));
        assert!(matches!(
            WindowSize::new(100.0, value),
            Err(WindowError::InvalidGeometry(_))
        ));
    }
    for value in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::from(i32::MIN) - 1.0,
    ] {
        assert!(DesktopPosition::new(value, 0.0, DesktopSpace::Win32Pixels).is_err());
        assert!(DesktopPosition::new(0.0, value, DesktopSpace::Win32Pixels).is_err());
    }
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(WindowGeometry::new(WindowSize::new(1.0, 1.0).unwrap(), None, scale).is_err());
    }
}

#[test]
fn placement_preserves_negative_origins_units_and_fractional_scale() {
    let size = WindowSize::new(720.5, 480.25).unwrap();
    let position = DesktopPosition::new(-1800.0, -120.0, DesktopSpace::AppKitPoints).unwrap();
    let observed = WindowGeometry::new(size, Some(position), 1.25).unwrap();
    let saved = observed.placement();
    assert_eq!(saved, WindowPlacement::new(size, Some(position)));
    assert_eq!(
        saved.position().unwrap().space(),
        DesktopSpace::AppKitPoints
    );
    assert_eq!(saved.position().unwrap().x(), -1800.0);
    assert_eq!(saved.position().unwrap().y(), -120.0);
    assert_eq!(saved.size().width(), 720.5);
    assert_eq!(saved.size().height(), 480.25);
    assert_eq!(observed.scale_factor(), 1.25);
    assert_eq!(WindowPlacement::new(size, None).position(), None);
}

#[test]
fn reentrant_observations_keep_order_and_only_deduplicate_adjacent_values() {
    let life = Rc::new(Lifecycle::default());
    let seen = Rc::new(RefCell::new(Vec::new()));
    life.geometry_events()
        .set_handler({
            let life = Rc::clone(&life);
            let seen = Rc::clone(&seen);
            move |id, value| {
                assert_eq!(id, life.id());
                seen.borrow_mut().push(value.size().width());
                if seen.borrow().len() == 1 {
                    for width in [320.0, 320.0, 450.0, 320.0] {
                        life.geometry_events().observe(geometry(width));
                    }
                }
            }
        })
        .unwrap();
    life.geometry_events().observe(geometry(200.0));
    assert_eq!(*seen.borrow(), [200.0, 320.0, 450.0, 320.0]);
    life.geometry_events().clear_handler();
}

#[test]
fn replacing_a_handler_drops_its_queued_observations() {
    let life = Rc::new(Lifecycle::default());
    let seen = Rc::new(RefCell::new(Vec::new()));
    life.geometry_events()
        .set_handler({
            let life = Rc::clone(&life);
            let seen = Rc::clone(&seen);
            move |_, _| {
                life.geometry_events().observe(geometry(350.0));
                let seen = Rc::clone(&seen);
                life.geometry_events()
                    .set_handler(move |_, geometry| seen.borrow_mut().push(geometry.size().width()))
                    .unwrap();
                life.geometry_events().observe(geometry(400.0));
            }
        })
        .unwrap();
    life.geometry_events().observe(geometry(300.0));
    assert_eq!(*seen.borrow(), [400.0]);
    life.geometry_events().clear_handler();
}

#[test]
fn closing_during_notification_discards_pending_and_future_observations() {
    let life = Rc::new(Lifecycle::default());
    let seen = Rc::new(RefCell::new(Vec::new()));
    life.geometry_events()
        .set_handler({
            let life = Rc::clone(&life);
            let seen = Rc::clone(&seen);
            move |_, geometry| {
                seen.borrow_mut().push(geometry);
                life.geometry_events().observe(crate::geometry(500.0));
                life.geometry_events().retire();
            }
        })
        .unwrap();
    life.geometry_events().observe(geometry(300.0));
    life.geometry_events().observe(geometry(600.0));
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(
        life.geometry_events().set_handler(|_, _| {}),
        Err(WindowError::StaleWindow)
    );
}

#[test]
fn deferred_native_observation_cannot_cross_subscription_replacement() {
    let life = Lifecycle::default();
    let seen = Rc::new(RefCell::new(Vec::new()));
    assert!(life.geometry_events().capture(geometry(100.0)).is_none());
    life.geometry_events()
        .set_handler(|_, _| panic!("retired callback"))
        .unwrap();
    let old = life.geometry_events().capture(geometry(300.0)).unwrap();
    assert_eq!(old.id(), life.id());
    life.geometry_events()
        .set_handler({
            let seen = Rc::clone(&seen);
            move |_, value| seen.borrow_mut().push(value.size().width())
        })
        .unwrap();
    life.geometry_events().observe_captured(&old);
    assert!(seen.borrow().is_empty());
    let current = life.geometry_events().capture(geometry(400.0)).unwrap();
    life.geometry_events().observe_captured(&current);
    assert_eq!(*seen.borrow(), [400.0]);
    let retired = life.geometry_events().capture(geometry(500.0)).unwrap();
    life.geometry_events().retire();
    life.geometry_events().observe_captured(&retired);
    assert_eq!(*seen.borrow(), [400.0]);
}
