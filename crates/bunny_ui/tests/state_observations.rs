use bunny_ui_core::layout::{Proposal, Size};
use bunny_ui_core::prelude::*;
use bunny_ui_core::provenance::{
    NodeKind, SitePlacement,
    observation::{Evidence, Value},
};
use bunny_ui_core::runtime::Runtime;

#[derive(Clone)]
struct Post {
    likes: State<i32>,
}
impl Component for Post {
    fn body(self) -> impl View {
        text!("Likes: {}", self.likes)
    }
}

fn layout(runtime: &Runtime, view: &impl View) -> Vec<SitePlacement> {
    runtime.layout(
        view,
        Proposal::exact(Size {
            width: 320.0,
            height: 300.0,
        }),
    );
    runtime
        .sites()
        .into_iter()
        .filter(|site| site.kind == NodeKind::Text)
        .collect()
}

fn value(site: &SitePlacement) -> i128 {
    let values = site.reading.as_ref().unwrap().observe().unwrap().unwrap();
    assert_eq!(values.len(), 1);
    match &values[0].evidence {
        Evidence::State(value) => match value.value {
            Value::Signed(value) => value,
            ref other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn two_posts_observe_actual_independent_state_through_their_text_binding() {
    let first = Post {
        likes: State::new(7),
    };
    let second = Post {
        likes: State::new(11),
    };
    let view = vstack((first.clone(), second.clone(), text("constant")));
    let runtime = Runtime::scene("state_observation_posts");
    runtime.record_sites(true);
    let sites = layout(&runtime, &view);
    assert_eq!((value(&sites[0]), value(&sites[1])), (7, 11));
    assert!(sites[2].reading.is_none());
    first.likes.set(8);
    let after = layout(&runtime, &view);
    assert_eq!((value(&after[0]), value(&after[1])), (8, 11));
    assert_eq!(sites[0].mounted, after[0].mounted);
    assert_ne!(after[0].mounted, after[1].mounted);
    assert_ne!(after[0].reading, after[1].reading);
    drop(runtime);
    assert!(
        after[0]
            .reading
            .as_ref()
            .unwrap()
            .observe()
            .unwrap()
            .is_none()
    );
}

#[test]
fn keyed_rows_keep_read_edges_on_reorder_and_old_rows_expire_on_remount() {
    #[derive(Clone)]
    struct Rows {
        keys: State<Vec<i32>>,
        shared: State<i32>,
    }
    impl Component for Rows {
        fn body(self) -> impl View {
            vstack(for_each(self.keys, i32::to_string, move |_| Post {
                likes: self.shared,
            }))
        }
    }
    let view = Rows {
        keys: State::new(vec![1, 2]),
        shared: State::new(7),
    };
    let runtime = Runtime::scene("state_observation_rows");
    runtime.record_sites(true);
    let before = layout(&runtime, &view);
    let old = before[0].reading.clone().unwrap();
    view.keys.set(vec![2, 1]);
    let reordered = layout(&runtime, &view);
    assert_eq!(reordered[1].reading.as_ref(), Some(&old));
    assert_eq!(
        old.observe().unwrap(),
        reordered[0].reading.as_ref().unwrap().observe().unwrap()
    );
    view.keys.set(vec![2]);
    layout(&runtime, &view);
    assert!(old.observe().unwrap().is_none());
    view.keys.set(vec![2, 1]);
    let remounted = layout(&runtime, &view);
    assert_ne!(remounted[1].reading.as_ref(), Some(&old));
    assert!(old.observe().unwrap().is_none());
    assert_eq!(value(&remounted[1]), 7);
}

#[test]
fn changed_dependencies_follow_evaluation_and_recording_revokes_old_tokens() {
    #[derive(Clone)]
    struct Conditional {
        choose: State<bool>,
        a: State<i32>,
        b: State<i32>,
    }
    impl Component for Conditional {
        fn body(self) -> impl View {
            text!(
                "{}",
                if self.choose.get() {
                    self.a.get()
                } else {
                    self.b.get()
                }
            )
        }
    }
    let view = Conditional {
        choose: State::new(true),
        a: State::new(7),
        b: State::new(11),
    };
    let runtime = Runtime::scene("state_observation_conditional");
    runtime.record_sites(true);
    let sites = layout(&runtime, &view);
    let reading = sites[0].reading.as_ref().unwrap();
    let before = reading.observe().unwrap().unwrap();
    assert_eq!(before.len(), 2);
    view.choose.set(false);
    layout(&runtime, &view);
    let after = reading.observe().unwrap().unwrap();
    assert_eq!(after.len(), 2);
    assert_ne!(before[1].dependency, after[1].dependency);
    assert!(
        matches!(&after[1].evidence, Evidence::State(value) if value.value == Value::Signed(11))
    );
    runtime.record_sites(false);
    assert!(reading.observe().unwrap().is_none());
    runtime.record_sites(true);
    let new = layout(&runtime, &view);
    assert!(reading.observe().unwrap().is_none());
    assert!(
        new[0]
            .reading
            .as_ref()
            .unwrap()
            .observe()
            .unwrap()
            .is_some()
    );
}
