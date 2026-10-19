use bunny_ui_motor::identity::{self, DepKey};
use bunny_ui_motor::state::{
    State,
    observation::{self, Evidence, Value},
};

fn observed(state: State<i32>) -> observation::ObservedState {
    let snapshot = observation::snapshot(&[DepKey::State(state.version().0)]).unwrap();
    match &snapshot[0].evidence {
        Evidence::State(value) => value.clone(),
        other => panic!("expected live state, got {other:?}"),
    }
}

#[test]
fn values_and_birth_sites_are_read_without_entering_the_reactive_graph() {
    let birth = line!() + 1;
    let first = State::new(7_i32);
    let second = State::new(11_i32);
    let before = identity::registry_counts();
    let version = first.version();
    let value = observed(first);
    assert_eq!(value.value, Value::Signed(7));
    assert_eq!(value.type_name, "i32");
    assert_eq!(value.constructor.line(), birth);
    assert_eq!(value.constructor.file(), file!());
    assert_eq!(first.version(), version);
    assert_eq!(identity::registry_counts(), before);
    let shared = first;
    assert_eq!(observed(first).version, observed(shared).version);
    first.set(8);
    assert_eq!(observed(first).value, Value::Signed(8));
    assert_eq!(observed(second).value, Value::Signed(11));
    assert_ne!(first.version().0, second.version().0);
    assert_eq!(observed(first).constructor, value.constructor);
}

#[test]
fn unknown_values_and_large_text_are_explicit_without_formatting_user_code() {
    struct Opaque;
    impl Clone for Opaque {
        fn clone(&self) -> Self {
            panic!("inspection must not clone application values")
        }
    }
    impl std::fmt::Display for Opaque {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("inspection must not execute application formatting")
        }
    }
    let opaque = State::new(Opaque);
    let large = State::new("x".repeat(observation::TEXT_LIMIT + 1));
    let text = State::new(String::from("Likes: {7} 🐇"));
    let snapshot = observation::snapshot(&[
        DepKey::State(opaque.version().0),
        DepKey::State(large.version().0),
        DepKey::State(text.version().0),
        DepKey::Store(999),
        DepKey::State(u64::MAX),
    ])
    .unwrap();
    let values: Vec<_> = snapshot.iter().map(|row| &row.evidence).collect();
    assert!(matches!(values[0], Evidence::State(value) if value.value == Value::Opaque));
    assert!(
        matches!(values[1], Evidence::State(value) if value.value == Value::TooLarge { bytes: observation::TEXT_LIMIT + 1 })
    );
    assert!(
        matches!(values[2], Evidence::State(value) if value.value == Value::Text("Likes: {7} 🐇".into()))
    );
    assert_eq!(values[3], &Evidence::Unsupported);
    assert_eq!(values[4], &Evidence::Missing);
}

#[test]
fn inspecting_during_an_update_is_explicit_and_never_subscribes_a_probe() {
    let state = State::new(7_i32);
    state.update(|value| {
        assert_eq!(observed(state).value, Value::Updating);
        *value = 9;
    });
    let _probe = identity::begin_probe();
    assert_eq!(observed(state).value, Value::Signed(9));
    assert!(!identity::probe_read_anything());
}

#[test]
fn numeric_extremes_keep_their_exact_type_and_range() {
    let signed = State::new(i128::MIN);
    let unsigned = State::new(u128::MAX);
    let boolean = State::new(true);
    let scalar = State::new('🐇');
    let values = observation::snapshot(&[
        DepKey::State(signed.version().0),
        DepKey::State(unsigned.version().0),
        DepKey::State(boolean.version().0),
        DepKey::State(scalar.version().0),
    ])
    .unwrap();
    let actual: Vec<_> = values
        .into_iter()
        .map(|value| match value.evidence {
            Evidence::State(value) => value.value,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        actual,
        [
            Value::Signed(i128::MIN),
            Value::Unsigned(u128::MAX),
            Value::Bool(true),
            Value::Char('🐇')
        ]
    );
}

#[test]
fn retained_state_keeps_its_birth_and_retired_slots_never_reuse_identity() {
    fn mount(initial: i32) -> State<i32> {
        identity::begin_pass();
        let state = {
            let _view = identity::enter_view("ObservationFixture");
            State::new(initial)
        };
        identity::end_pass();
        state
    }
    let first = mount(7);
    let id = first.version().0;
    let birth = observed(first).constructor;
    first.set(9);
    let retained = mount(100);
    assert_eq!(retained.version().0, id);
    assert_eq!(observed(retained).value, Value::Signed(9));
    assert_eq!(observed(retained).constructor, birth);
    identity::reset_scene("ObservationFixture");
    assert_eq!(
        observation::snapshot(&[DepKey::State(id)]).unwrap()[0].evidence,
        Evidence::Missing
    );
    let remounted = mount(11);
    assert_ne!(remounted.version().0, id);
    assert_eq!(observed(remounted).value, Value::Signed(11));
    identity::reset_scene("ObservationFixture");
}

#[test]
fn observation_is_bounded_and_deduplicates_without_reordering() {
    let state = State::new(7_i32);
    let dep = DepKey::State(state.version().0);
    assert_eq!(observation::snapshot(&[dep, dep]).unwrap().len(), 1);
    assert_eq!(
        observation::snapshot(&vec![dep; observation::DEPENDENCY_LIMIT + 1]),
        Err(observation::Unavailable::TooManyDependencies)
    );
}
