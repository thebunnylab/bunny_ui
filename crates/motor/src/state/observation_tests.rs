use super::*;

#[test]
fn borrowed_storage_refuses_the_complete_snapshot_without_panicking() {
    let state = super::super::State::new(7_i32);
    let dep = DepKey::State(state.version().0);
    super::super::with_arena::<i32, _>(|_| {
        assert_eq!(snapshot(&[dep]), Err(Unavailable::Borrowed));
    });
    assert!(
        matches!(&snapshot(&[dep]).unwrap()[0].evidence, Evidence::State(value) if value.value == Value::Signed(7))
    );
}

#[test]
fn float_observation_equality_preserves_bits_without_nan_churn() {
    assert_eq!(Value::Float(f64::NAN), Value::Float(f64::NAN));
    assert_ne!(Value::Float(0.0), Value::Float(-0.0));
    assert_ne!(
        Value::Float(f64::from_bits(0x7ff8_0000_0000_0001)),
        Value::Float(f64::from_bits(0x7ff8_0000_0000_0002))
    );
}

#[test]
#[ignore = "release-mode observation cost diagnostic"]
fn inspect_one_dependency_among_ten_thousand_states() {
    let states: Vec<_> = (0..10_000).map(super::super::State::new).collect();
    let dependency = DepKey::State(states[5_000].version().0);
    let start = std::time::Instant::now();
    for _ in 0..1_000 {
        std::hint::black_box(snapshot(&[dependency]).unwrap());
    }
    eprintln!(
        "1000 selected-state snapshots among 10000 slots: {:?}; i32 slot bytes: {}",
        start.elapsed(),
        std::mem::size_of::<super::super::TypedSlot<i32>>()
    );
}
