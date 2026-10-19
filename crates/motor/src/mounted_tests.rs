use super::*;

fn pass(root: &'static str, f: impl FnOnce()) {
    begin_pass();
    {
        let _root = enter(root);
        f();
    }
    end_pass();
}

#[test]
fn mounted_declarations_keep_identity_but_absent_sites_retire() {
    reset_world();
    assert!(mounted_site(Site::Named("text"), "text").is_none());
    let mut first = None;
    pass("root", || first = mounted_site(Site::Named("text"), "text"));
    assert!(first.is_some());
    pass("root", || {
        assert_eq!(first, mounted_site(Site::Named("text"), "text"))
    });
    pass("root", || {
        assert!(mounted_site(Site::Named("other"), "text").is_some());
    });
    pass("root", || {
        assert_ne!(first, mounted_site(Site::Named("text"), "text"))
    });
}

#[test]
fn repeated_sites_and_roles_are_distinct_and_stable() {
    reset_world();
    let mut ids = Vec::new();
    pass("root", || {
        for role in ["text", "text", "padding"] {
            ids.push(mounted_site(Site::Named("same"), role).unwrap());
        }
    });
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
    pass("root", || {
        for (role, expected) in ["text", "text", "padding"].into_iter().zip(ids) {
            assert_eq!(Some(expected), mounted_site(Site::Named("same"), role));
        }
    });
}

#[test]
fn mounted_ids_follow_keyed_scopes_and_retained_boundaries() {
    reset_world();
    let mut ids = Vec::new();
    pass("root", || {
        for key in ["[a]", "[b]"] {
            let _key = enter(key);
            ids.push(mounted_site(Site::Named("row"), "text").unwrap());
        }
    });
    pass("root", || {
        for (key, id) in ["[b]", "[a]"].into_iter().zip(ids.iter().rev()) {
            let _key = enter(key);
            assert_eq!(Some(*id), mounted_site(Site::Named("row"), "text"));
        }
    });
    pass("root", || {
        mark_skipped(&Rc::from("root/[a]"));
        mark_skipped(&Rc::from("root/[b]"));
    });
    pass("root", || {
        let _key = enter("[a]");
        assert_eq!(Some(ids[0]), mounted_site(Site::Named("row"), "text"));
    });
    pass("root", || {
        let _key = enter("[b]");
        assert_ne!(Some(ids[1]), mounted_site(Site::Named("row"), "text"));
    });
}

#[test]
fn scene_and_world_teardown_never_recycle_mounted_ids() {
    reset_world();
    let mut a = None;
    let mut b = None;
    pass("a", || a = mounted_site(Site::Named("row"), "text"));
    pass("b", || b = mounted_site(Site::Named("row"), "text"));
    assert_ne!(a, b);
    reset_scene("a");
    pass("b", || {
        assert_eq!(b, mounted_site(Site::Named("row"), "text"))
    });
    pass("a", || {
        assert_ne!(a, mounted_site(Site::Named("row"), "text"))
    });
    reset_world();
    pass("b", || {
        assert_ne!(b, mounted_site(Site::Named("row"), "text"))
    });
}

#[test]
fn a_rerun_inside_a_skipped_boundary_retires_only_its_absent_declarations() {
    reset_world();
    let mut first = None;
    pass("root", || {
        let _view = enter("parent/child");
        first = mounted_site(Site::Named("x"), "text");
    });
    pass("root", || {
        mark_skipped(&Rc::from("root/parent"));
        mark_reran(&Rc::from("root/parent/child"));
    });
    pass("root", || {
        let _view = enter("parent/child");
        assert_ne!(first, mounted_site(Site::Named("x"), "text"));
    });
}

#[test]
fn removing_a_repeated_occurrence_retires_it_in_a_surviving_scope() {
    reset_world();
    let mut ids = Vec::new();
    pass("root", || {
        for _ in 0..2 {
            ids.push(mounted_site(Site::Named("x"), "text").unwrap());
        }
    });
    pass("root", || {
        assert_eq!(Some(ids[0]), mounted_site(Site::Named("x"), "text"));
    });
    pass("root", || {
        assert_eq!(Some(ids[0]), mounted_site(Site::Named("x"), "text"));
        assert_ne!(Some(ids[1]), mounted_site(Site::Named("x"), "text"));
    });
}
