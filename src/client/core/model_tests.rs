use serde_json::json;

impl Model {
    /// A projection fixture, not a shared document edit.
    pub(super) fn add_test_tab(
        &mut self,
        space: &str,
        tab: &str,
        facts: impl IntoIterator<Item = Value>,
    ) -> Vec<PaneId> {
        let ids: Vec<_> = facts.into_iter().map(|facts| {
            let address = facts["address"].as_str().unwrap().to_owned();
            assert!(self.id(&address).is_none());
            let id = PaneId::alloc();
            self.records.insert(id, Record { address, facts, seq: 0 });
            id
        }).collect();
        if !self.organization.workspaces.iter().any(|w| w.id == space) {
            self.organization.workspaces.push(organization::Workspace {
                id: space.into(), label: String::new(), tabs: Vec::new(),
            });
        }
        self.organization.workspaces.iter_mut().find(|w| w.id == space).unwrap().tabs.push(organization::Tab {
            id: tab.into(), label: String::new(), panes: TabLayout::new(ids.clone(), None).unwrap(),
        });
        self.repair_focus(None);
        self.revision += 1;
        ids
    }
}

#[test]
fn snapshot_aggregates_status_in_pane_tab_and_space_order() {
    use actions::AgentStatus::*;
    let mut model = Model::new("projection".into());
    model.add_test_tab("workspace", "one", [
        json!({"address":"first","status":{"state":"working"}}),
        json!({"address":"no-agent"}),
        json!({"address":"third","status":{"state":"done"}}),
    ]);
    model.add_test_tab("workspace", "two", [json!({"address":"second-tab","status":{"state":"error"}})]);
    model.add_test_tab("other", "other-tab", [json!({"address":"elsewhere","status":{"state":"idle"}})]);
    model.focus_tab("two").unwrap();
    let snapshot = model.snapshot();
    assert_eq!(snapshot.workspaces.iter().map(|w| (w.agent_status, w.focused)).collect::<Vec<_>>(), [(Blocked, true), (Idle, false)]);
    assert_eq!(snapshot.tabs.iter().map(|t| (t.agent_status, t.focused)).collect::<Vec<_>>(), [(Working, false), (Blocked, true), (Idle, false)]);
    assert_eq!(snapshot.panes.iter().map(|p| p.pane_id.as_str()).collect::<Vec<_>>(), ["first", "no-agent", "third", "second-tab", "elsewhere"]);
    assert_eq!(snapshot.agents.iter().map(|a| (a.agent_status, a.focused)).collect::<Vec<_>>(), [(Working, false), (Done, false), (Blocked, true), (Idle, false)]);
    assert_eq!(snapshot.focused_pane_id.as_deref(), Some("second-tab"));
}

#[test]
fn workspace_name_uses_first_tab_cwd_but_creation_uses_active_tab_cwd() {
    let mut model = Model::new("cwd".into());
    model.add_test_tab("space", "first", [json!({"address":"t1","cwd":"/launch","foreground":{"cwd":"/home/user/code/herdr"}})]);
    model.add_test_tab("space", "next", [json!({"address":"t2","cwd":"/another-launch","foreground":{"cwd":"/work/other"}})]);
    model.focus_tab("next").unwrap();
    let snapshot = model.snapshot();
    assert_eq!(snapshot.workspaces[0].label, "herdr");
    assert_eq!(snapshot.workspaces[0].new_workspace_cwd, "/work/other");
    assert_eq!(snapshot.panes[0].cwd.as_deref(), Some("/launch"));
    assert_eq!(snapshot.panes[0].foreground_cwd.as_deref(), Some("/home/user/code/herdr"));
    model.organization.workspaces[0].label = "Workspace 1".into();
    assert_eq!(model.snapshot().workspaces[0].label, "Workspace 1");
}

#[test]
fn only_confirmed_program_exit_completes_membership() {
    for reason in ["exited", "killed", "signaled"] {
        assert!(program_completed(&json!({"state":"finished","reason":reason,"pid":123})));
        assert!(!program_completed(&json!({"state":"live","reason":reason,"pid":null})));
    }
    for reason in ["holder_lost", "server_stop", "stopped_by_restart", "host_shutdown", ""] {
        assert!(!program_completed(&json!({"state":"finished","reason":reason})));
    }
}

#[test]
fn invalid_projection_is_atomic_and_removing_last_pane_repairs_focus() {
    let mut model = Model::new("focus".into());
    let old = model.add_test_tab("space1", "tab1", [json!({"address":"remove"})])[0];
    let retained = model.add_test_tab("space2", "tab2", [json!({"address":"keep"})])[0];
    let revision = model.revision;
    let bad = Organization { workspaces: vec![organization::Workspace {
        id: "space1".into(), label: "edited".into(), tabs: vec![organization::Tab {
            id: "tab1".into(), label: String::new(), panes: vec!["new".into(), "missing".into()]
        }]
    }] };
    let facts = HashMap::from([("new".into(), json!({"address":"new"}))]);
    assert!(model.install_projection(bad, &facts).is_err());
    assert_eq!(model.revision, revision);
    assert_eq!(model.focused(), Some(old));
    assert!(model.id("new").is_none());
    assert_eq!(model.records.len(), 2);
    model.remove(old);
    assert_eq!(model.focused(), Some(retained));
    assert_eq!(model.organization.workspaces.len(), 1);
    model.remove(retained);
    assert!(model.snapshot().panes.is_empty());
    assert!(model.focused().is_none());
}
