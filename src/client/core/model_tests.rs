#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use ratatui::layout::Rect;
    fn assert_model_invariants(model: &Model) {
        let addresses = model
            .records
            .values()
            .map(|r| r.address.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(addresses.len(), model.records.len());
        let ids = model.all_layouts().flat_map(|(_, layout)| layout.pane_ids()).collect::<Vec<_>>();
        assert_eq!(ids.len(), model.records.len());
        assert_eq!(
            ids.into_iter().collect::<HashSet<_>>(),
            model.records.keys().copied().collect()
        );
        assert_eq!(model.focused().is_none(), model.records.is_empty());
        if let Some(focus) = model.focused() {
            assert!(model.records.contains_key(&focus));
        }
        assert_eq!(model.snapshot().panes.len(), model.records.len());
    }

    #[test]
    fn opaque_identity_survives_close_insert_restore_and_rejected_changes() {
        let mut model = Model::new("identity-characterization".into());
        let mut ids = Vec::new();
        for address in ["t1", "remote/t1", "t01", "other/opaque-identity"] {
            ids.push(model.insert(json!({"address":address})).unwrap());
            assert_model_invariants(&model);
        }
        model.focus(ids[1]).unwrap();
        let saved = model.description().unwrap();
        model.remove(ids[2]);
        model.insert(json!({"address":"new/t1"})).unwrap();
        model.restore(&saved).unwrap();
        assert_model_invariants(&model);
        assert_eq!(
            model.records[&model.focused().unwrap()].address,
            "remote/t1"
        );
        let before = model.description();
        assert!(model.focus(ids[2]).is_err());
        model.remove(ids[2]);
        assert_eq!(before, model.description());
        let mut corrupt = model.description().unwrap();
        let pane = actions::LayoutNode::Pane {
            pane: actions::LayoutPane {
                pane_id: Some("t1".into()),
                ..Default::default()
            },
        };
        corrupt.root = actions::LayoutNode::Split {
            direction: actions::SplitDirection::Right,
            ratio: 0.5,
            first: Box::new(pane.clone()),
            second: Box::new(pane),
        };
        assert!(model.restore(&corrupt).is_err());
        assert_eq!(before, model.description());
        assert_model_invariants(&model);
    }

    #[test]
    fn shared_organization_preserves_local_focus_and_opaque_identity() {
        let mut a = Model::new("a".into());
        let first = a.insert(json!({"address":"opaque/root"})).unwrap();
        let (tab, second) = a.create_tab(WORKSPACE, json!({"address":"remote/second"}), Some("Second".into()), true).unwrap();
        let (workspace, third) = a.create_workspace(json!({"address":"other/root"}), Some("Other".into()), true).unwrap();
        assert_model_invariants(&a);
        let doc = a.shared().unwrap();
        let facts = a.records.values().map(|r| (r.address.clone(), r.facts.clone())).collect();
        let mut b = Model::new("b".into());
        b.install(doc.clone(), &facts).unwrap();
        b.focus_workspace(WORKSPACE).unwrap();
        let b_first = b.id("opaque/root").unwrap();
        b.focus(b_first).unwrap();
        assert_eq!(b.shared().unwrap(), doc, "client focus must not become a shared mutation");
        a.focus(second).unwrap();
        assert_eq!(a.active_tab(), Some(tab.as_str()));
        assert_ne!(a.focused(), Some(first));
        a.close_workspace(&workspace).unwrap();
        assert!(!a.records.contains_key(&third));
        let doc = a.shared().unwrap();
        b.install(doc.clone(), &facts).unwrap();
        assert_eq!(b.focused(), Some(b_first));
        assert_eq!(b.id("opaque/root"), Some(b_first));
        assert_eq!(b.shared().unwrap(), doc);
        assert_model_invariants(&b);
        assert_eq!(b.snapshot().workspaces.len(), 1);
        assert_eq!(b.snapshot().tabs.len(), 2);
        assert_eq!(b.snapshot().panes.iter().filter(|p| p.focused).count(), 1);
    }

    #[test]
    fn retired_membership_cannot_be_resurrected_and_invalid_install_is_atomic() {
        let mut a = Model::new("a".into());
        let first = a.insert(json!({"address":"t1"})).unwrap();
        a.insert(json!({"address":"remote/t1"})).unwrap();
        a.retire(first).unwrap();
        assert!(a.insert(json!({"address":"t1"})).is_err());
        let before = a.shared().unwrap();
        let mut bad = before.clone();
        bad.workspaces[0].tabs[0].root = Branch::Pane { terminal: "t1".into() };
        assert!(a.install(bad, &HashMap::new()).is_err());
        assert_eq!(a.shared().unwrap(), before);
        let mut bad = before.clone();
        bad.workspaces[0].tabs[0].root = Branch::Pane { terminal: "missing".into() };
        assert!(a.install(bad, &HashMap::new()).is_err());
        assert_eq!(a.shared().unwrap(), before);
        assert_model_invariants(&a);
    }

    #[test]
    fn workspace_and_tab_ids_are_not_reused_after_close() {
        let mut a = Model::new("a".into());
        a.insert(json!({"address":"t1"})).unwrap();
        let (tab, _) = a.create_tab(WORKSPACE, json!({"address":"t2"}), None, false).unwrap();
        a.close_tab(&tab).unwrap();
        let (next, _) = a.create_tab(WORKSPACE, json!({"address":"t3"}), None, false).unwrap();
        assert_ne!(tab, next);
        let (workspace, _) = a.create_workspace(json!({"address":"t4"}), None, false).unwrap();
        a.close_workspace(&workspace).unwrap();
        let (next, _) = a.create_workspace(json!({"address":"t5"}), None, false).unwrap();
        assert_ne!(workspace, next);
        assert_model_invariants(&a);
        a.shared().unwrap().validate().unwrap();
    }

    #[test]
    fn addresses_are_unique_and_restore_preserves_focus_and_split_geometry() {
        let mut a = Model::new("boot-a".into());
        let first = a
            .insert(json!({"address":"machine/t7","status":{"state":"working"}}))
            .unwrap();
        a.insert(json!({"address":"t9"})).unwrap();
        a.insert(json!({"address":"t9"})).unwrap();
        assert_eq!(a.records.len(), 2);
        a.focus(first).unwrap();
        let saved = a.description().unwrap();
        let mut b = Model::new("boot-b".into());
        b.insert(json!({"address":"t9"})).unwrap();
        b.insert(json!({"address":"machine/t7"})).unwrap();
        b.restore(&saved).unwrap();
        assert_eq!(a.description().unwrap(), b.description().unwrap());
        assert_eq!(
            a.layout_snapshot(Rect::new(0, 0, 80, 24)).unwrap(),
            b.layout_snapshot(Rect::new(0, 0, 80, 24)).unwrap()
        );
    }
    #[test]
    fn pane_manual_name_is_shared_metadata_not_core_identity_or_agent_status() {
        let mut model = Model::new("pane-names".into());
        let mut facts = json!({"address":"t1", "name":"core-program-identity", "labels":{"herdr":"tui"}});
        model.insert(facts.clone()).unwrap();
        assert_eq!(model.snapshot().panes[0].label, None);
        facts["labels"]["herdr.display_name"] = json!("  build worker  ");
        model.insert(facts.clone()).unwrap();
        assert_eq!(model.snapshot().panes[0].label.as_deref(), Some("build worker"));
        assert_eq!(model.records.values().next().unwrap().facts["name"], "core-program-identity");
        assert!(model.snapshot().agents.is_empty());
        for label in [json!(""), json!("  "), json!(null)] {
            facts["labels"]["herdr.display_name"] = label;
            model.insert(facts.clone()).unwrap();
            assert_eq!(model.snapshot().panes[0].label, None);
        }
        assert_model_invariants(&model);
    }

    #[test]
    fn original_tab_names_follow_position_not_lifetime_identity() {
        let mut model = Model::new("original-names".into());
        model.insert(json!({"address":"one"})).unwrap();
        let (second, _) = model.create_tab(WORKSPACE, json!({"address":"two"}), None, false).unwrap();
        let (named, _) = model.create_tab(WORKSPACE, json!({"address":"named"}), Some("notes".into()), false).unwrap();
        assert_eq!(model.snapshot().tabs.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["1", "2", "notes"]);
        assert!(!model.snapshot().tabs[0].custom_label);
        model.close_tab(&second).unwrap();
        let (last, _) = model.create_tab(WORKSPACE, json!({"address":"last"}), None, false).unwrap();
        assert_eq!(model.snapshot().tabs.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["1", "notes", "3"]);
        assert_eq!(model.organization.workspaces[0].tabs[2].number, 4);
        model.organization.workspaces[0].tabs.swap(0, 2);
        assert_eq!(model.snapshot().tabs.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["1", "notes", "3"]);
        assert_eq!(model.snapshot().tabs[0].tab_id, last);
        assert_eq!(model.snapshot().tabs[1].tab_id, named);
        assert_model_invariants(&model);
    }

    #[test]
    fn completed_programs_close_only_their_panes_then_empty_tabs_and_workspaces() {
        let mut model = Model::new("exit".into());
        let first = model.insert(json!({"address":"first"})).unwrap();
        let split = model.insert(json!({"address":"split"})).unwrap();
        let (other_tab, other) = model.create_tab(WORKSPACE, json!({"address":"other"}), None, true).unwrap();
        for (id, reason) in [(split, "exited"), (other, "killed"), (first, "exited")] {
            let record = model.records.get_mut(&id).unwrap();
            record.facts["state"] = json!("finished");
            record.facts["reason"] = json!(reason);
            // Core retains the original PID on a finished record; do not require null.
            record.facts["pid"] = json!(123);
            model.retire_completed_programs().unwrap();
            assert_model_invariants(&model);
            model.shared().unwrap().validate().unwrap();
            if id == split {
                assert_eq!(model.organization.workspaces[0].tabs.len(), 2);
                assert_eq!(model.active_tab(), Some(other_tab.as_str()));
            } else if id == other {
                assert_eq!(model.organization.workspaces[0].tabs.len(), 1);
                assert_eq!(model.focused(), Some(first));
            }
        }
        assert!(model.organization.workspaces.is_empty());
        assert_eq!(model.organization.retiring.len(), 3);
        model.retire_completed_programs().unwrap();
        assert!(model.insert(json!({"address":"first"})).is_err());
        let (new_workspace, _) = model.create_workspace(json!({"address":"new"}), None, true).unwrap();
        assert_ne!(new_workspace, WORKSPACE);
        assert_model_invariants(&model);
    }

    #[test]
    fn loss_restart_unknown_and_stream_closure_are_not_confirmed_program_exit() {
        for reason in ["exited", "killed", "signaled"] {
            assert!(super::super::model::program_completed(&json!({"state":"finished", "reason":reason})));
            assert!(!super::super::model::program_completed(&json!({"state":"live", "reason":reason, "pid":null})));
        }
        for facts in [
            json!({"state":"live", "reason":"exited", "pid":123}),
            json!({"state":"live", "reason":"holder_lost", "pid":null}),
            json!({"state":"finished", "reason":"holder_lost"}),
            json!({"state":"finished", "reason":"server_stop"}),
            json!({"state":"finished", "reason":"stopped_by_restart"}),
            json!({"state":"finished", "reason":"host_shutdown"}),
            json!({"state":"finished"}),
            json!({"reason":"removed"}),
        ] {
            let mut model = Model::new("not-completed".into());
            let id = model.insert(json!({"address":"preserved"})).unwrap();
            model.records.get_mut(&id).unwrap().facts = facts;
            let before = model.shared().unwrap();
            model.retire_completed_programs().unwrap();
            assert_eq!(model.shared().unwrap(), before);
            assert_model_invariants(&model);
        }
    }

    #[test]
    fn no_screen_or_agent_name_can_create_status_and_error_keeps_its_raw_label() {
        let mut model = Model::new("boot".into());
        model
            .insert(json!({"address":"t1","title":"Claude working blocked"}))
            .unwrap();
        assert!(model.snapshot().agents.is_empty());
        model
            .insert(json!({"address":"t1","status":{"state":"error","app":"demo"}}))
            .unwrap();
        let a = model.snapshot().agents.remove(0);
        assert_eq!(a.agent_status, actions::AgentStatus::Blocked);
        assert_eq!(a.state_labels, vec![("OSC 7501".into(), "error".into())]);
        model.insert(json!({"address":"t1","status":null})).unwrap();
        assert!(model.snapshot().agents.is_empty());
    }

    #[test]
    fn initial_public_foreground_cwd_names_the_workspace_without_a_later_event() {
        let mut model = Model::new("initial-cwd".into());
        let root = model.insert(json!({
            "address":"t1", "cwd":"/home/user", "state":"live", "pid":41,
            "foreground":{"pid":41,"cwd":"/home/user/code/herdr","name":"bash","argv":["bash"]}
        })).unwrap();
        // This is Core's public subscription/get shape: last_cwd is private.
        assert!(model.records[&root].facts.get("last_cwd").is_none());
        let organization = model.shared().unwrap();
        let snapshot = model.snapshot();
        assert_eq!(model.workspace_info(WORKSPACE).unwrap().label, "herdr");
        assert_eq!(snapshot.workspaces[0].label, "herdr");
        assert_eq!(snapshot.workspaces[0].new_workspace_cwd, "/home/user/code/herdr");
        assert_eq!(snapshot.panes[0].cwd.as_deref(), Some("/home/user"));
        assert_eq!(snapshot.panes[0].foreground_cwd.as_deref(), Some("/home/user/code/herdr"));
        assert_eq!(model.records[&root].facts["cwd"], "/home/user");
        assert_eq!(model.shared().unwrap(), organization);
    }

    #[test]
    fn initial_workspace_name_uses_first_tab_but_creation_cwd_uses_active_tab() {
        let mut model = Model::new("first-tab-cwd".into());
        model.insert(json!({"address":"t1","cwd":"/launch",
            "foreground":{"cwd":"/home/user/code/herdr"}})).unwrap();
        model.create_tab(WORKSPACE, json!({"address":"t2","cwd":"/another-launch",
            "foreground":{"cwd":"/work/other"}}), None, true).unwrap();
        let snapshot = model.snapshot();
        assert_eq!(snapshot.workspaces[0].label, "herdr");
        assert_eq!(snapshot.workspaces[0].new_workspace_cwd, "/work/other");
        model.organization.workspaces[0].label = "manual".into();
        assert_eq!(model.snapshot().workspaces[0].label, "manual");
    }

    #[test]
    fn workspace_name_is_the_first_tabs_root_cwd_until_a_custom_name_is_set() {
        // Original `Workspace::display_name_from_terminals`: no custom name means
        // the name is derived from the first tab's root pane cwd, and it follows
        // a `cd` even after the tab is split.
        let mut model = Model::new("workspace-naming".into());
        model
            .insert(json!({"address":"t1","cwd":"/home/user/projects/herdr"}))
            .unwrap();
        let workspace = model.active_workspace.clone();
        let info = |model: &Model| model.workspace_info(&workspace).unwrap();
        assert_eq!(info(&model).label, "herdr");
        assert!(!model.organization.workspaces[0].is_custom_named());

        // A split must not change which pane names the workspace.
        let root = model.focused().unwrap();
        model
            .insert_at(
                json!({"address":"t2","cwd":"/home/user/other"}),
                Some(root),
                ratatui::layout::Direction::Horizontal,
                0.5,
                false,
            )
            .unwrap();
        assert_eq!(info(&model).label, "herdr");

        // The root shell reporting a new directory renames it.
        model.records.get_mut(&root).unwrap().facts["last_cwd"] = json!("/home/user");
        assert_eq!(info(&model).label, "user");

        // An explicit name wins and is not rewritten.
        model.organization.workspaces[0].label = "notes".into();
        assert_eq!(info(&model).label, "notes");
        model.organization.workspaces[0].label = String::new();
        assert_eq!(info(&model).label, "user");
    }

    #[test]
    fn workspace_numbers_are_positions_and_tab_numbers_are_stable() {
        // The original reported `index + 1` for workspaces, but a stable public
        // number for tabs (its own test asserts label "2" beside number 3).
        let mut model = Model::new("workspace-numbering".into());
        model.insert(json!({"address":"t1","cwd":"/tmp"})).unwrap();
        let (second, _) = model
            .create_workspace(json!({"address":"t2","cwd":"/tmp"}), None, false)
            .unwrap();
        assert_eq!(model.workspace_info(&second).unwrap().number, 2);

        let (tab, _) = model
            .create_tab(WORKSPACE, json!({"address":"t3","cwd":"/tmp"}), None, false)
            .unwrap();
        assert_eq!(model.tab_info(&tab).unwrap().number, 2);
        model.close_tab(&tab).unwrap();
        let (later, _) = model
            .create_tab(WORKSPACE, json!({"address":"t4","cwd":"/tmp"}), None, false)
            .unwrap();
        let later_info = model.tab_info(&later).unwrap();
        assert_eq!(later_info.number, 3, "tab numbers are never reused");

        // Close the first workspace: the survivor reports position 1 again.
        let first = model.organization.workspaces[0].id.clone();
        model.close_workspace(&first).unwrap();
        assert_eq!(model.workspace_info(&second).unwrap().number, 1);
    }

    #[test]
    fn closing_the_active_workspace_keeps_its_position_and_clamps_to_the_last() {
        // Original `close_workspaces`: selection keeps the closed index, clamped.
        let mut model = Model::new("workspace-close-position".into());
        model.insert(json!({"address":"t1","cwd":"/tmp"})).unwrap();
        let mut workspaces = vec![model.active_workspace.clone()];
        for address in ["t2", "t3"] {
            let (id, _) = model
                .create_workspace(json!({"address":address,"cwd":"/tmp"}), None, true)
                .unwrap();
            workspaces.push(id);
        }
        assert_eq!(model.active_workspace, workspaces[2]);

        // Closing the middle one while a later one is active keeps the active id.
        model.close_workspace(&workspaces[1]).unwrap();
        assert_eq!(model.active_workspace, workspaces[2]);

        // Closing the active last workspace selects the new last one.
        model.close_workspace(&workspaces[2]).unwrap();
        assert_eq!(model.active_workspace, workspaces[0]);

        // Closing the active first workspace selects whatever took its position.
        let (fourth, _) = model
            .create_workspace(json!({"address":"t4","cwd":"/tmp"}), None, false)
            .unwrap();
        model.close_workspace(&workspaces[0]).unwrap();
        assert_eq!(model.active_workspace, fourth, "the survivor at index 0 wins");
    }
}
