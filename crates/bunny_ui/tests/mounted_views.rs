//! Mounted identity follows actual rendering, independently of source spelling.
use bunny_ui_core::layout::{Proposal, Size};
use bunny_ui_core::prelude::*;
use bunny_ui_core::provenance::{MountedId, NodeKind};
use bunny_ui_core::runtime::Runtime;
use std::collections::{BTreeMap, HashSet};

fn layout(runtime: &Runtime, view: &impl View, width: f64) {
    runtime.layout(
        view,
        Proposal::exact(Size {
            width,
            height: 300.0,
        }),
    );
    let ids: Vec<_> = runtime
        .sites()
        .iter()
        .map(|site| site.mounted.expect("active pass"))
        .collect();
    assert_eq!(
        ids.len(),
        ids.iter().collect::<HashSet<_>>().len(),
        "no repeated mounted identity"
    );
}
fn text_ids(runtime: &Runtime) -> BTreeMap<String, MountedId> {
    runtime
        .sites()
        .into_iter()
        .filter(|s| s.kind == NodeKind::Text)
        .map(|s| (s.path.unwrap().to_string(), s.mounted.unwrap()))
        .collect()
}

#[derive(Clone)]
struct Row {
    count: State<i32>,
}
impl Component for Row {
    fn body(self) -> impl View {
        text!("Likes: {}", self.count)
            .padding_length(3.0)
            .padding_length(2.0)
    }
}
#[derive(Clone)]
struct Rows {
    keys: State<Vec<i32>>,
    count: State<i32>,
}
impl Component for Rows {
    fn body(self) -> impl View {
        vstack(for_each(self.keys, i32::to_string, move |_| Row {
            count: self.count,
        }))
    }
}

#[test]
fn keyed_instances_survive_reorder_resize_cache_and_reactive_text() {
    let runtime = Runtime::scene("keyed_mounts");
    runtime.record_sites(true);
    let view = Rows {
        keys: State::new(vec![1, 2, 3]),
        count: State::new(0),
    };
    layout(&runtime, &view, 300.0);
    let before = text_ids(&runtime);
    assert_eq!(before.len(), 3);
    layout(&runtime, &view, 300.0);
    assert_eq!(text_ids(&runtime), before);
    layout(&runtime, &view, 400.0);
    assert_eq!(text_ids(&runtime), before);
    view.count.set(42);
    layout(&runtime, &view, 400.0);
    assert_eq!(text_ids(&runtime), before);
    view.keys.set(vec![3, 1, 2]);
    layout(&runtime, &view, 400.0);
    assert_eq!(text_ids(&runtime), before);
    view.keys.set(vec![3, 1]);
    layout(&runtime, &view, 400.0);
    let removed = text_ids(&runtime);
    assert_eq!(removed.len(), 2);
    view.keys.set(vec![3, 1, 2]);
    layout(&runtime, &view, 400.0);
    let after = text_ids(&runtime);
    for (key, id) in &before {
        if removed.contains_key(key) {
            assert_eq!(after[key], *id);
        } else {
            assert_ne!(after[key], *id);
        }
    }
}

#[test]
fn cloned_views_and_independent_scenes_do_not_share_mounted_identity() {
    let label = text("same source");
    let view = vstack((label.clone(), label));
    let first = Runtime::scene("mounted_first");
    first.record_sites(true);
    layout(&first, &view, 300.0);
    let ids: HashSet<_> = first
        .sites()
        .into_iter()
        .map(|s| s.mounted.unwrap())
        .collect();
    let second = Runtime::scene("mounted_second");
    second.record_sites(true);
    layout(&second, &view, 300.0);
    assert!(
        second
            .sites()
            .iter()
            .all(|s| !ids.contains(&s.mounted.unwrap()))
    );
    drop(first);
    let recreated = Runtime::scene("mounted_first");
    recreated.record_sites(true);
    layout(&recreated, &view, 300.0);
    assert!(
        recreated
            .sites()
            .iter()
            .all(|s| !ids.contains(&s.mounted.unwrap()))
    );
}

/// CPU-side provenance diagnostic, not a native canvas/OS frame-rate claim.
#[test]
#[ignore = "release-mode provenance cost diagnostic"]
fn twenty_scenes_ten_thousand_nodes_cost() {
    #[derive(Clone)]
    struct Board {
        rows: State<Vec<u32>>,
    }
    impl Component for Board {
        fn body(self) -> impl View {
            vstack(for_each(self.rows, u32::to_string, |row| {
                text(row.to_string())
            }))
        }
    }
    for recording in [false, true] {
        let scenes: Vec<_> = (0..20)
            .map(|index| {
                let runtime = Runtime::scene(format!("diagnostic_{recording}_{index}"));
                runtime.record_sites(recording);
                (
                    runtime,
                    Board {
                        rows: State::new((0..500).collect()),
                    },
                )
            })
            .collect();
        let begin = std::time::Instant::now();
        for (runtime, view) in &scenes {
            runtime.layout(
                view,
                Proposal::exact(Size {
                    width: 400.0,
                    height: 800.0,
                }),
            );
        }
        let mount = begin.elapsed();
        let sites: usize = scenes
            .iter()
            .map(|(runtime, _)| runtime.sites().len())
            .sum();
        let mut samples = Vec::new();
        for sample in 0..32 {
            let begin = std::time::Instant::now();
            for (runtime, view) in &scenes {
                runtime.layout(
                    view,
                    Proposal::exact(Size {
                        width: 400.0 + f64::from(sample % 2),
                        height: 800.0,
                    }),
                );
            }
            samples.push(begin.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "recording={recording} sites={sites} mount_ms={:.3} resize20_p50_ms={:.3} p95_ms={:.3} max_ms={:.3}",
            mount.as_secs_f64() * 1000.0,
            samples[16],
            samples[30],
            samples[31]
        );
        if recording {
            assert_eq!(sites, 10_020);
        } else {
            assert_eq!(sites, 0);
        }
    }
}
