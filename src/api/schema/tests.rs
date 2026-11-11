use std::collections::HashMap;

use super::*;

#[test]
fn request_uses_dot_method_names() {
    let request = Request {
        id: "req_1".into(),
        method: Method::WorkspaceCreate(WorkspaceCreateParams {
            source_workspace_id: None,
            cwd: Some("/tmp".into()),
            focus: true,
            label: Some("api".into()),
            env: Default::default(),
        }),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "workspace.create");
}

#[test]
fn workspace_close_group_intent_defaults_false_and_round_trips() {
    let request: Request = serde_json::from_value(serde_json::json!({
        "id": "close",
        "method": "workspace.close",
        "params": { "workspace_id": "w1" }
    }))
    .unwrap();
    assert!(matches!(
        request.method,
        Method::WorkspaceClose(WorkspaceCloseParams {
            close_group: false,
            ..
        })
    ));

    let explicit = Request {
        id: "close-group".into(),
        method: Method::WorkspaceClose(WorkspaceCloseParams {
            workspace_id: "w1".into(),
            close_group: true,
        }),
    };
    let json = serde_json::to_value(&explicit).unwrap();
    assert_eq!(json["params"]["close_group"], true);
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), explicit);
}

#[test]
fn request_round_trips_for_server_reload_config() {
    let request = Request {
        id: "req_reload".into(),
        method: Method::ServerReloadConfig(EmptyParams::default()),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "server.reload_config");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, request);
}

#[test]
fn integration_list_request_and_response_round_trip() {
    let request = Request {
        id: "req_integrations".into(),
        method: Method::IntegrationList(EmptyParams::default()),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "integration.list");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

    let response = SuccessResponse {
        id: "req_integrations".into(),
        result: ResponseResult::IntegrationList {
            integrations: vec![IntegrationInfo {
                target: IntegrationTarget::Codex,
                label: "codex".into(),
                command: "codex".into(),
                available: true,
                state: IntegrationState::Outdated,
            }],
        },
    };
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["result"]["type"], "integration_list");
    assert_eq!(json["result"]["integrations"][0]["state"], "outdated");
    assert_eq!(
        serde_json::from_value::<SuccessResponse>(json).unwrap(),
        response
    );
}

#[test]
fn command_invoke_request_round_trips_without_command_text() {
    let request = Request {
        id: "req_command".into(),
        method: Method::CommandInvoke(CommandInvokeParams {
            command_id: "cmd_0123456789abcdef0123456789abcdef".into(),
            workspace_id: Some("w1".into()),
            tab_id: Some("w1:t1".into()),
            pane_id: Some("w1:p1".into()),
            selection: None,
        }),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "command.invoke");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
}

#[test]
fn unknown_method_is_rejected() {
    let json = r#"{"id":"req_1","method":"nope","params":{}}"#;
    let err = serde_json::from_str::<Request>(json)
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown variant"));
}

#[test]
fn missing_required_params_are_rejected() {
    let json = r#"{"id":"req_1","method":"pane.send_text","params":{"pane_id":"p_1"}}"#;
    let err = serde_json::from_str::<Request>(json)
        .unwrap_err()
        .to_string();
    assert!(err.contains("text"));
}

#[test]
fn agent_status_request_values_remain_strict() {
    assert!(serde_json::from_str::<AgentStatus>(r#""working""#).is_ok());
    assert!(serde_json::from_str::<AgentStatus>(r#""future_status""#).is_err());
}

#[test]
fn success_response_round_trips() {
    let response = SuccessResponse {
        id: "req_1".into(),
        result: ResponseResult::Ok {},
    };

    let json = serde_json::to_string(&response).unwrap();
    let restored: SuccessResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}

#[test]
fn worktree_request_and_response_round_trip() {
    let request = Request {
        id: "req_worktree".into(),
        method: Method::WorktreeCreate(WorktreeCreateParams {
            workspace_id: Some("1".into()),
            branch: Some("worktree/api".into()),
            base: Some("HEAD".into()),
            focus: true,
            trust_repository: true,
            ..WorktreeCreateParams::default()
        }),
    };
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"trust_repository\":true"));
    let restored: Request = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, request);

    let response = SuccessResponse {
        id: "req_worktree".into(),
        result: ResponseResult::WorktreeCreated {
            workspace: WorkspaceInfo {
                workspace_id: "w_1".into(),
                number: 2,
                label: "herdr".into(),
                focused: true,
                pane_count: 1,
                tab_count: 1,
                active_tab_id: "w_1:1".into(),
                agent_status: AgentStatus::Unknown,
                tokens: HashMap::new(),
                worktree: Some(WorkspaceWorktreeInfo {
                    repo_key: "/repo/herdr/.git".into(),
                    repo_name: "herdr".into(),
                    repo_root: "/repo/herdr".into(),
                    checkout_path: "/worktrees/herdr/worktree-api".into(),
                    is_linked_worktree: true,
                }),
            },
            tab: TabInfo {
                tab_id: "w_1:1".into(),
                workspace_id: "w_1".into(),
                number: 1,
                label: "herdr".into(),
                focused: true,
                pane_count: 1,
                agent_status: AgentStatus::Unknown,
            },
            root_pane: PaneInfo {
                pane_id: "w_1-1".into(),
                terminal_id: "term_1".into(),
                workspace_id: "w_1".into(),
                tab_id: "w_1:1".into(),
                focused: true,
                cwd: Some("/worktrees/herdr/worktree-api".into()),
                foreground_cwd: None,
                restore_error: None,
                label: None,
                agent: None,
                title: None,
                terminal_title: None,
                terminal_title_stripped: None,
                display_agent: None,
                agent_status: AgentStatus::Unknown,
                state_labels: HashMap::new(),
                tokens: HashMap::new(),
                agent_session: None,
                scroll: None,
                revision: 0,
            },
            worktree: WorktreeInfo {
                path: "/worktrees/herdr/worktree-api".into(),
                branch: Some("worktree/api".into()),
                is_bare: false,
                is_detached: false,
                is_prunable: false,
                is_linked_worktree: true,
                open_workspace_id: Some("w_1".into()),
                label: "herdr".into(),
            },
        },
    };
    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("\"type\":\"worktree_created\""));
    assert!(json.contains("\"worktree\""));
    let restored: SuccessResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}

#[test]
fn create_response_round_trips_with_root_pane() {
    let response = SuccessResponse {
        id: "req_2".into(),
        result: ResponseResult::TabCreated {
            tab: TabInfo {
                tab_id: "w_1:2".into(),
                workspace_id: "w_1".into(),
                number: 2,
                label: "review".into(),
                focused: false,
                pane_count: 1,
                agent_status: AgentStatus::Unknown,
            },
            root_pane: PaneInfo {
                pane_id: "w_1-3".into(),
                terminal_id: "term_example".into(),
                workspace_id: "w_1".into(),
                tab_id: "w_1:2".into(),
                focused: false,
                cwd: Some("/tmp/review".into()),
                foreground_cwd: None,
                restore_error: None,
                label: None,
                agent: None,
                title: None,
                terminal_title: None,
                terminal_title_stripped: None,
                display_agent: None,
                agent_status: AgentStatus::Unknown,
                state_labels: HashMap::new(),
                tokens: HashMap::new(),
                agent_session: None,
                scroll: None,
                revision: 0,
            },
        },
    };

    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("\"type\":\"tab_created\""));
    assert!(json.contains("\"root_pane\""));
    let restored: SuccessResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}

#[test]
fn error_response_round_trips() {
    let response = ErrorResponse {
        id: "req_1".into(),
        error: ErrorBody {
            code: "pane_not_found".into(),
            message: "pane p_1 not found".into(),
        },
    };

    let json = serde_json::to_string(&response).unwrap();
    let restored: ErrorResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}

#[test]
fn pane_link_activate_round_trips() {
    let request = Request {
        id: "req_pane_link".into(),
        method: Method::PaneLinkActivate(PaneLinkActivateParams {
            pane_id: "w1:p1".into(),
            viewport_row: 3,
            col: 7,
            content_revision: Some(42),
            offset_from_bottom: Some(5),
        }),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "pane.link.activate");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, request);

    let response = ResponseResult::PaneLinkActivated {
        url: Some("https://example.test".into()),
        handled: false,
    };
    let json = serde_json::to_string(&response).unwrap();
    let restored: ResponseResult = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}

#[test]
fn pane_link_resolve_round_trips() {
    let request: Request = serde_json::from_value(serde_json::json!({
        "id": "hover", "method": "pane.link.resolve",
        "params": {"pane_id": "pane-1", "viewport_row": 2, "col": 3}
    }))
    .unwrap();
    assert!(matches!(request.method, Method::PaneLinkResolve(_)));
    let result = ResponseResult::PaneLinkResolved {
        regions: vec![PaneLinkRegion {
            row: 2,
            start_col: 3,
            end_col: 9,
        }],
    };
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["type"], "pane_link_resolved");
    assert_eq!(json["regions"][0]["end_col"], 9);
    assert_eq!(
        serde_json::from_value::<ResponseResult>(json).unwrap(),
        result
    );
}

#[test]
fn read_choices_cover_every_api_value() {}
