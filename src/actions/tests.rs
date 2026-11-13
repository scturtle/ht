use super::*;

#[test]
fn internal_action_names_are_unique() {
    let names: std::collections::HashSet<_> = Action::NAMES.iter().copied().collect();
    assert_eq!(names.len(), Action::NAMES.len());
    for retired in [
        "release_notes.dismiss",
        "command.invoke",
        "integration.list",
        "integration.install",
        "worktree.create",
        "worktree.list",
        "worktree.open",
        "worktree.remove",
        "product_announcement.dismiss",
        "client_shell.surface.set",
    ] {
        assert!(!names.contains(retired), "{retired}");
    }
}

#[test]
fn action_results_do_not_inline_three_creation_metadata_records() {
    // Workspace creation may add a pointer to tab + pane metadata, but must not
    // make every action result carry a third inline record. Reserve a word for
    // that pointer and another for the enum tag instead of pinning an ABI size.
    let budget = std::mem::size_of::<(TabInfo, PaneInfo)>() + 2 * std::mem::size_of::<usize>();
    let actual = std::mem::size_of::<ActionResult>();
    assert!(
        actual <= budget,
        "ActionResult is {actual} bytes (budget: {budget})"
    );
}

#[test]
fn internal_request_keeps_its_correlation_id_and_typed_action() {
    let request = ActionRequest {
        id: "client-shell:42".into(),
        action: Action::WorkspaceClose(WorkspaceCloseParams {
            workspace_id: "w1".into(),
            close_group: false,
        }),
    };
    let cloned = request.clone();
    assert_eq!(cloned.id, request.id);
    assert_eq!(cloned.action.name(), "workspace.close");
    assert_eq!(cloned, request);
    let message = crate::protocol::ClientMessage::ClientShellEndpointRequest {
        boot_id: "projection-7".into(),
        request: Box::new(request),
    };
    let crate::protocol::ClientMessage::ClientShellEndpointRequest { boot_id, request } = message
    else {
        panic!("expected typed internal action");
    };
    assert_eq!(boot_id, "projection-7");
    assert_eq!(request, Box::new(cloned));
}

#[test]
fn workspace_close_group_intent_defaults_to_false() {
    let params: WorkspaceCloseParams =
        serde_json::from_value(serde_json::json!({"workspace_id": "w1"})).unwrap();
    assert!(!params.close_group);
    let explicit: WorkspaceCloseParams =
        serde_json::from_value(serde_json::json!({"workspace_id": "w1", "close_group": true}))
            .unwrap();
    assert!(explicit.close_group);
}

#[test]
fn agent_status_values_remain_strict() {
    assert!(serde_json::from_str::<AgentStatus>(r#""working""#).is_ok());
    assert!(serde_json::from_str::<AgentStatus>(r#""future_status""#).is_err());
}
