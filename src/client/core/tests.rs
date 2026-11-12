use super::session::replay_lines;
use super::view::View;
use super::*;
use crate::{
    layout::{Node, PaneId, TileLayout},
    protocol::ClientSurfaceSize,
};
use ratatui::layout::{Direction, Rect};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn advertised_core_actions_are_internal_and_exclude_unimplemented_ui_actions() {
    let names: std::collections::HashSet<_> =
        crate::actions::Action::NAMES.iter().copied().collect();
    let implemented: std::collections::HashSet<_> = methods::METHODS.iter().copied().collect();
    assert_eq!(implemented.len(), methods::METHODS.len());
    assert!(implemented.is_subset(&names));
    for unavailable in [
        "release_notes.dismiss",
        "command.invoke",
        "pane.clear",
        "pane.edit_scrollback",
        "pane.input.set",
        "pane.link.activate",
        "pane.link.resolve",
        "server.reload_config",
        "workspace.list",
        "workspace.get",
        "tab.list",
        "tab.get",
    ] {
        assert!(!implemented.contains(unavailable), "{unavailable}");
    }
}

#[test]
fn hc_socket_selects_an_external_core_without_creating_a_session_socket() {
    // No directory creation or existence check: `hc connect` handles the target.
    let socket = PathBuf::from("/external/nonexistent/runtime/other.sock");
    assert_eq!(
        selected_socket(Some(socket.clone()), "default", false).unwrap(),
        socket
    );
    // Match hc's support for paths relative to the caller's working directory.
    let relative = PathBuf::from("other.sock");
    assert_eq!(
        selected_socket(Some(relative.clone()), "default", false).unwrap(),
        relative
    );
}

#[test]
fn hc_socket_rejects_empty_paths_and_private_session_conflicts() {
    for (socket, isolate) in [
        (PathBuf::new(), false),
        (PathBuf::from("/external/core.sock"), true),
    ] {
        assert_eq!(
            selected_socket(Some(socket), "private", isolate)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

#[test]
fn relay_receives_the_selected_socket_without_private_xdg_overrides() {
    let socket = PathBuf::from("/external/core.sock");
    let command = transport::command(
        std::path::Path::new("herdr-core"),
        std::path::Path::new("/client/state"),
        &socket,
        false,
    );
    let env = command.get_envs().collect::<HashMap<_, _>>();
    assert_eq!(
        env.get(std::ffi::OsStr::new("HC_SOCKET")),
        Some(&Some(socket.as_os_str()))
    );
    for key in ["XDG_RUNTIME_DIR", "XDG_STATE_HOME", "XDG_CONFIG_HOME"] {
        assert!(!env.contains_key(std::ffi::OsStr::new(key)));
    }
}

fn endpoint() -> (CoreEndpoint, mpsc::Receiver<Command>) {
    let (sender, receiver) = mpsc::sync_channel(64);
    (
        CoreEndpoint {
            sender,
            wake: Arc::default(),
            pending: None,
            queued: Arc::new(AtomicUsize::new(0)),
            stopped: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
            options: None,
            events: tokio::sync::mpsc::channel(1).0,
        },
        receiver,
    )
}

#[test]
fn diagnostic_capture_is_a_barrier_even_when_pane_input_is_stopped() {
    let (mut endpoint, receiver) = endpoint();
    endpoint.options = Some(Options {
        binary: "hc".into(),
        home: "/unused".into(),
        socket: "/unused/socket".into(),
        isolate: false,
        client_id: None,
    });
    endpoint.stopped.store(true, Ordering::Release);
    let source = endpoint
        .diagnostic_source("pane-1")
        .expect("diagnostics remain available during outage");
    let Command::Diagnostics { pane, done, .. } = receiver.try_recv().unwrap() else {
        panic!("diagnostic command, not input")
    };
    assert_eq!(pane, "pane-1");
    assert!(endpoint.pending.is_none());
    assert_eq!(endpoint.queued.load(Ordering::Acquire), 0);
    done.send(json!({"closed":false})).unwrap();
    assert_eq!(source.local.unwrap().recv().unwrap()["closed"], false);
    assert!(receiver.try_recv().is_err());
}

#[test]
fn optional_core_labels_disable_only_pane_rename_not_the_connection_or_other_actions() {
    for hello in [
        json!({"methods":[]}),
        json!({"methods":["terminal.get", "doc.set"]}),
    ] {
        let methods = methods::advertised(&hello);
        assert!(!methods.iter().any(|method| method == "pane.rename"));
        assert!(methods.iter().any(|method| method == "pane.close"));
        assert!(methods.iter().any(|method| method == "workspace.create"));
    }
    assert!(methods::advertised(&json!({"methods":["terminal.label"]}))
        .iter()
        .any(|method| method == "pane.rename"));
}

#[test]
fn original_pane_zoom_and_swap_actions_are_available_in_the_core_client() {
    for action in ["pane.zoom", "pane.swap"] {
        assert!(
            methods::advertised(&json!({}))
                .iter()
                .any(|method| method == action),
            "{action}"
        );
    }
}

#[test]
fn configured_shell_is_an_executable_not_a_command_string_or_runtime_fallback() {
    let mut config = crate::config::Config::default();
    assert!(session::configured_shell(&config).is_empty());
    for shell in ["fish", "/a path/fish", "powershell.exe"] {
        config.terminal.default_shell = shell.into();
        assert_eq!(session::configured_shell(&config), vec![shell.to_owned()]);
    }
}

#[test]
fn tiny_input_bursts_are_batched_and_keep_order() {
    let (mut endpoint, receiver) = endpoint();
    for i in 0..1000 {
        endpoint
            .send(&ClientMessage::ClientShellFocus {
                focused: i % 2 == 0,
            })
            .unwrap();
    }
    let mut count = 0;
    let mut batches = 0;
    while let Ok(Command::Batch(batch)) = receiver.try_recv() {
        let batch = std::mem::take(&mut *batch.lock().unwrap());
        endpoint.queued.fetch_sub(batch.bytes, Ordering::AcqRel);
        for message in batch.messages {
            assert!(
                matches!(message.message,ClientMessage::ClientShellFocus{focused} if focused==(count%2==0))
            );
            count += 1;
        }
        batches += 1;
    }
    assert_eq!(count, 1000);
    assert!(batches < 64);
    assert_eq!(endpoint.queued.load(Ordering::Acquire), 0);
}

#[test]
fn recovery_drain_does_not_leave_new_input_in_an_orphaned_batch() {
    let (mut endpoint, receiver) = endpoint();
    endpoint
        .send(&ClientMessage::ClientShellFocus { focused: true })
        .unwrap();
    assert_eq!(drain_input(&receiver, &endpoint.queued), 1);
    assert_eq!(endpoint.queued.load(Ordering::Acquire), 0);
    endpoint
        .send(&ClientMessage::ClientShellFocus { focused: false })
        .unwrap();
    let Command::Batch(batch) = receiver
        .try_recv()
        .expect("new input must enqueue a live batch after recovery")
    else {
        panic!("input batch")
    };
    let batch = std::mem::take(&mut *batch.lock().unwrap());
    assert_eq!(batch.messages.len(), 1);
    assert!(matches!(
        batch.messages[0].message,
        ClientMessage::ClientShellFocus { focused: false }
    ));
    assert_eq!(endpoint.queued.load(Ordering::Acquire), batch.bytes);
}

#[test]
fn input_memory_and_flush_barriers_are_bounded() {
    let (mut endpoint, receiver) = endpoint();
    endpoint
        .send(&ClientMessage::ClientShellFocus { focused: true })
        .unwrap();
    assert!(endpoint.flush(Instant::now()).is_err());
    endpoint
        .send(&ClientMessage::ClientShellFocus { focused: false })
        .unwrap();
    let Command::Batch(first) = receiver.recv().unwrap() else {
        panic!("first batch")
    };
    assert_eq!(first.lock().unwrap().messages.len(), 1);
    assert!(matches!(receiver.recv().unwrap(), Command::Flush(_)));
    let Command::Batch(last) = receiver.recv().unwrap() else {
        panic!("last batch")
    };
    assert_eq!(last.lock().unwrap().messages.len(), 1);
    let paste = ClientMessage::ClientShellPaneInput {
        pane_id: "opaque/terminal".into(),
        events: vec![crate::protocol::ClientPaneInputEvent::Paste(
            "a".repeat(512 * 1024),
        )],
    };
    let mut accepted = 0;
    while endpoint.send(&paste).is_ok() {
        accepted += 1;
        assert!(accepted < 9);
    }
    assert!(accepted > 0);
    assert!(endpoint.queued.load(Ordering::Acquire) <= 4 * 1024 * 1024);
}

#[test]
fn client_presentation_identity_is_scoped_and_cannot_escape_its_directory() {
    let a = presentation_identity(None, Some("/dev/pts/4".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        Some(a.clone()),
        presentation_identity(None, Some("/dev/pts/4".into())).unwrap()
    );
    assert_ne!(
        Some(a),
        presentation_identity(None, Some("/dev/pts/5".into())).unwrap()
    );
    assert_eq!(presentation_identity(None, None).unwrap(), None);
    assert_eq!(
        presentation_identity(Some("client-A".into()), None)
            .unwrap()
            .as_deref(),
        Some("client-A")
    );
    for invalid in ["", "..", "../other", "/tmp/other", "one/two", "a\n"] {
        assert!(presentation_identity(Some(invalid.into()), None).is_err());
    }
    let root = std::env::temp_dir().join(format!(
        "herdr-client-prefs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    session::ensure_private_directory(&root).unwrap();
    let first = session::client_preferences(&root, Some("client-A")).unwrap();
    let second = session::client_preferences(&root, Some("client-B")).unwrap();
    assert!(session::client_preferences(&root, Some("client-A")).is_err());
    assert!(
        !root.join("client.lock").exists(),
        "no runtime-wide client lock"
    );
    drop(first);
    assert!(session::client_preferences(&root, Some("client-A")).is_ok());
    assert!(session::client_preferences(&root, None).unwrap().is_none());
    drop(second);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn desired_geometry_never_infers_the_authoritative_grid_or_fights_another_client() {
    let mut view = view::View::new(80, 24, 2).unwrap();
    let revision = view.revision;
    assert_eq!(view.request_size((100, 30, 8, 16)), (true, true));
    assert_eq!(
        view.size,
        (80, 24, 0, 0),
        "a desired size is never the parser grid"
    );
    assert_eq!(view.revision, revision);
    assert_eq!(view.resize_request(), [100, 0, 30, 0]);
    view.display_cells(8, 16);
    assert_eq!(view.size, (80, 24, 8, 16), "host pixels are not a grid");
    // Another client wins the pane: only its RESIZE may move the parser.
    view.authoritative_size((60, 12, 8, 16));
    assert_eq!(view.size, (60, 12, 8, 16));
    assert_eq!(view.requested, (100, 30, 8, 16));
    assert_eq!(
        view.request_size((100, 30, 8, 16)),
        (false, false),
        "do not echo a feedback resize when another viewer wins"
    );
    assert_eq!(view.size, (60, 12, 8, 16));
    assert_eq!(view.request_size((110, 30, 8, 16)), (true, false));
    assert_eq!(view.size, (60, 12, 8, 16));
}

#[test]
fn ordered_frames_apply_resize_output_and_close_without_a_paint_boundary() {
    let id = PaneId::from_raw(1);
    let mut view = view::View::new(80, 24, 7).unwrap();
    // Malformed or zero geometry fails closed instead of adopting a bad grid.
    for payload in [&b""[..], b"\x50", b"\x50\x00\x18"] {
        assert!(
            view::apply_frame(
                &mut view::View::new(80, 24, 7).unwrap(),
                id,
                3,
                payload,
                (0, 0)
            )
            .is_err(),
            "{payload:?}"
        );
    }
    assert!(
        view::apply_frame(
            &mut view::View::new(80, 24, 7).unwrap(),
            id,
            3,
            &[0, 0, 24, 0],
            (0, 0)
        )
        .is_err(),
        "zero columns is not a grid"
    );
    // An authoritative RESIZE moves the parser even while it is the desired size.
    assert_eq!(view.size, (80, 24, 0, 0));
    view::apply_frame(&mut view, id, 3, &[60, 0, 12, 0], (8, 16)).unwrap();
    assert_eq!(view.size, (60, 12, 8, 16));
    // The first paint is ordinary DATA: it feeds the parser immediately.
    let outcome = view::apply_frame(&mut view, id, 0, b"HELLO", (8, 16)).unwrap();
    assert!(matches!(
        outcome,
        Some(view::FrameOutcome::Output(result)) if result.request_render
    ));
    assert_eq!(view.terminal.cursor_state().unwrap().x, 5);
    assert!(!view.closed);
    // CLOSE retires presentation and nothing afterwards is accepted.
    assert!(matches!(
        view::apply_frame(&mut view, id, 2, b"", (8, 16)).unwrap(),
        Some(view::FrameOutcome::Changed)
    ));
    assert!(view.closed);
    assert!(view::apply_frame(&mut view, id, 0, b"late", (8, 16)).is_err());
    assert!(
        view::apply_frame(&mut view::View::new(80, 24, 7).unwrap(), id, 6, b"", (0, 0)).is_err()
    );
}

#[test]
fn focus_claims_the_pane_grid_under_the_latest_policy() {
    let mut view = view::View::new(80, 24, 7).unwrap();
    assert_eq!(view.resize_request(), [80, 0, 24, 0]);
    // The layout changed while this pane was showing another client's grid.
    view.request_size((100, 30, 8, 16));
    assert_eq!(view.size, (80, 24, 0, 0), "requesting is not adopting");
    assert_eq!(
        view.resize_request(),
        [100, 0, 30, 0],
        "focus sends the reclaimed grid, not the old one"
    );
}

#[test]
fn core_directory_can_reopen_but_not_replace_a_file() {
    let path = std::env::temp_dir().join(format!(
        "herdr-core-directory-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    session::ensure_private_directory(&path).unwrap();
    session::ensure_private_directory(&path).unwrap();
    let file = path.join("not-a-directory");
    std::fs::write(&file, b"unchanged").unwrap();
    assert!(session::ensure_private_directory(&file).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"unchanged");
    std::fs::remove_dir_all(path).unwrap();
}

fn fixture(
    count: usize,
) -> (
    model::Model,
    HashMap<PaneId, view::View>,
    view::ChromeConfig,
) {
    fn balanced(ids: &[PaneId], depth: usize) -> Node {
        if ids.len() == 1 {
            return Node::Pane(ids[0]);
        }
        let middle = ids.len() / 2;
        Node::Split {
            direction: if depth.is_multiple_of(2) {
                Direction::Horizontal
            } else {
                Direction::Vertical
            },
            ratio: 0.5,
            first: Box::new(balanced(&ids[..middle], depth + 1)),
            second: Box::new(balanced(&ids[middle..], depth + 1)),
        }
    }
    let mut model = model::Model::new("core-test".into());
    let ids = (0..count)
        .map(|i| {
            model
                .insert(json!({"address":format!("opaque/t{i}"),"state":"live"}))
                .unwrap()
        })
        .collect::<Vec<_>>();
    model.layouts.insert(
        model::TAB.into(),
        TileLayout::from_saved(balanced(&ids, 0), ids[0]),
    );
    let chrome = view::ChromeConfig::from_config(&crate::config::Config::default());
    let mut views = HashMap::new();
    for (i, info) in view::pane_infos(&model, Rect::new(0, 0, 120, 40), &chrome)
        .into_iter()
        .enumerate()
    {
        let view = view::View::new(
            info.inner_rect.width.max(1),
            info.inner_rect.height.max(1),
            i as u32 + 2,
        )
        .unwrap();
        for n in 0..30 {
            view.terminal.process_pty_bytes(
                info.id,
                format!("\x1b[32mpane {i} populated {n}\x1b[0m\r\n").as_bytes(),
            );
        }
        views.insert(info.id, view);
    }
    (model, views, chrome)
}

#[test]
fn retained_projection_keeps_hidden_status_updates_from_freezing_client_overlays() {
    use crate::client::shell::{ClientShellConfig, ClientShellState};
    let (mut model, views, chrome) = fixture(1);
    let (_, hidden) = model
        .create_tab(
            model::WORKSPACE,
            json!({"address":"hidden","state":"live"}),
            None,
            false,
        )
        .unwrap();
    let mut renderer = view::Renderer::new().unwrap();
    let size = ClientSurfaceSize {
        cols: 120,
        rows: 40,
    };
    let first = renderer.compose(&model, &views, size, &chrome).unwrap();
    let config = toml::from_str("[keys]\nrename_workspace = 'f8'\n").unwrap();
    let mut shell = ClientShellState::new(ClientShellConfig::from_config(&config));
    shell.set_snapshot(Box::new(model.snapshot()));
    shell.set_pane_surface(first.clone());
    assert!(shell.compose(120, 40).is_some());
    model.records.get_mut(&hidden).unwrap().facts["status"] = json!({"state":"working"});
    model.revision += 1;
    shell.set_snapshot(Box::new(model.snapshot()));
    assert!(
        shell.compose(120, 40).is_none(),
        "snapshots require a matching surface revision"
    );
    // No views are passed: this path cannot traverse, lock or paint terminal cores.
    let retained = renderer.retain_projection(&model).unwrap();
    assert_eq!(first.frame, retained.frame);
    assert_eq!(first.panes, retained.panes);
    assert_eq!(retained.projection_revision, model.revision);
    assert!(retained.surface_revision > first.surface_revision);
    shell.set_pane_surface(retained);
    assert!(shell.handle_input_bytes(b"\x1b[19~").repaint);
    let frame = shell
        .compose(120, 40)
        .expect("matching retained projection must allow overlays");
    let text = frame
        .cells
        .iter()
        .map(|c| c.symbol.as_str())
        .collect::<String>();
    assert!(text.contains("rename workspace"));
}

#[test]
fn pane_name_updates_reuse_retained_surface_and_original_rename_dialog() {
    use crate::client::shell::{ClientShellConfig, ClientShellState};
    let (mut model, views, chrome) = fixture(1);
    let id = model.focused().unwrap();
    let mut renderer = view::Renderer::new().unwrap();
    let first = renderer
        .compose(
            &model,
            &views,
            ClientSurfaceSize {
                cols: 120,
                rows: 40,
            },
            &chrome,
        )
        .unwrap();
    model.records.get_mut(&id).unwrap().facts["labels"]["herdr.display_name"] =
        json!("manual-pane-name");
    model.revision += 1;
    // No terminal views/locks or paint are available to this metadata-only path.
    let retained = renderer.retain_projection(&model).unwrap();
    assert_eq!(retained.frame, first.frame);
    assert_eq!(retained.panes, first.panes);
    let config = toml::from_str("[keys]\nrename_pane = 'f8'\n").unwrap();
    let mut shell = ClientShellState::new(ClientShellConfig::from_config(&config));
    shell.set_snapshot(Box::new(model.snapshot()));
    shell.set_pane_surface(retained);
    assert!(shell.handle_input_bytes(b"\x1b[19~").repaint);
    let frame = shell.compose(120, 40).unwrap();
    let text = frame
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(text.contains("rename pane") && text.contains("manual-pane-name"));
}

#[test]
fn reused_renderer_retains_semantic_cells_not_backend_diff_residue() {
    let (model, mut views, mut chrome) = fixture(1);
    chrome.pane_borders = crate::config::PaneBordersConfig::Off;
    let id = model.focused().unwrap();
    let size = ClientSurfaceSize {
        cols: 120,
        rows: 40,
    };
    let mut renderer = view::Renderer::new().unwrap();
    for text in ["abcdefgh", "中文测试", "a中b文c", "中文测试", "abcdefgh"] {
        views
            .get_mut(&id)
            .unwrap()
            .process_output(id, format!("\x1b[2J\x1b[H{text}").as_bytes());
        let surface = renderer.compose(&model, &views, size, &chrome).unwrap();
        let fresh = view::Renderer::new()
            .unwrap()
            .compose(&model, &views, size, &chrome)
            .unwrap();
        assert_eq!(surface.frame, fresh.frame, "{text}");
        let retained = renderer.retain_projection(&model).unwrap();
        assert_eq!(surface.frame, retained.frame, "metadata-only: {text}");
    }
}

#[test]
fn hidden_output_updates_parser_without_presentation_invalidation() {
    let (model, mut views, _) = fixture(1);
    let id = model.focused().unwrap();
    let view = views.get_mut(&id).unwrap();
    view.visible = false;
    let revision = view.revision;
    let result = view.process_output(id, b"\rhidden-output");
    assert!(!result.request_render);
    assert_eq!(view.terminal.cursor_state().unwrap().x, 13);
    assert!(view.revision > revision);
    view.visible = true;
    assert!(view.process_output(id, b"visible-output").request_render);
}

#[test]
fn presentation_surface_preserves_geometry_and_cannot_infer_agent_status() {
    let (model, views, chrome) = fixture(15);
    let mut renderer = view::Renderer::new().unwrap();
    let size = ClientSurfaceSize {
        cols: 120,
        rows: 40,
    };
    let first = renderer.compose(&model, &views, size, &chrome).unwrap();
    assert_eq!(first.frame.cells.len(), 4800);
    assert_eq!(first.panes.len(), 15);
    assert_eq!(first.splits.len(), 14);
    assert!(first
        .panes
        .iter()
        .all(|p| p.rect.width > 0 && p.rect.height > 0));
    assert!(first.frame.cells.iter().any(|cell| cell.symbol == "p"));
    let second = renderer.compose(&model, &views, size, &chrome).unwrap();
    assert_eq!(first.frame, second.frame);
    assert_eq!(first.projection_revision, second.projection_revision);
    assert!(second.surface_revision > first.surface_revision);
    assert!(model
        .records
        .values()
        .all(|record| model::status(&record.facts) == crate::actions::AgentStatus::Unknown));
}

#[test]
#[ignore = "manual fixed-geometry scaling profile; run just bench-render-scale"]
fn render_scale_profile_core() {
    for (count, hidden_tabs, named, zoom_requested) in [
        (1, 0, false, false),
        (15, 0, false, false),
        (1, 14, false, false),
        (15, 14, false, false),
        (1, 0, true, false),
        (15, 0, true, false),
        (1, 14, true, false),
        (15, 14, true, false),
        (1, 0, false, true),
        (15, 0, false, true),
        (1, 14, false, true),
        (15, 14, false, true),
    ] {
        let (mut model, mut views, chrome) = fixture(count);
        for index in 0..hidden_tabs {
            let (_, id) = model
                .create_tab(
                    model::WORKSPACE,
                    json!({"address":format!("background/{index}"),"state":"live"}),
                    None,
                    false,
                )
                .unwrap();
            let mut view = view::View::new(120, 40, 200 + index).unwrap();
            view.visible = false;
            for row in 0..30 {
                view.terminal.process_pty_bytes(
                    id,
                    format!("background {index} populated {row}\r\n").as_bytes(),
                );
            }
            views.insert(id, view);
        }
        if named {
            for record in model.records.values_mut() {
                record.facts["labels"] =
                    json!({"herdr":"tui", "herdr.display_name":format!("pane {}",record.address)});
            }
        }
        if zoom_requested {
            model
                .zoom_pane(model.focused().unwrap(), crate::actions::PaneZoomMode::On)
                .unwrap();
        }
        let zoomed = model.active_zoomed();
        let mut renderer = view::Renderer::new().unwrap();
        let size = ClientSurfaceSize {
            cols: 120,
            rows: 40,
        };
        for _ in 0..20 {
            std::hint::black_box(renderer.compose(&model, &views, size, &chrome).unwrap());
        }
        let start = Instant::now();
        for _ in 0..200 {
            std::hint::black_box(renderer.compose(&model, &views, size, &chrome).unwrap());
        }
        eprintln!(
            "core_render_scale panes={count} hidden_tabs={hidden_tabs} named={named} zoom_requested={zoom_requested} zoomed={zoomed} geometry=120x40 mean_us={:.2}",
            start.elapsed().as_secs_f64() * 1e6 / 200.0
        );
        let start = Instant::now();
        for _ in 0..200 {
            std::hint::black_box(renderer.retain_projection(&model).unwrap());
        }
        eprintln!("core_retained_projection panes={count} hidden_tabs={hidden_tabs} named={named} zoom_requested={zoom_requested} zoomed={zoomed} geometry=120x40 mean_us={:.2}", start.elapsed().as_secs_f64() * 1e6 / 200.0);
    }
}

#[test]
#[ignore = "explicit render-stage profiling, no production instrumentation"]
fn render_stage_profile() {
    for count in [1, 15] {
        let (model, views, chrome) = fixture(count);
        let mut renderer = view::Renderer::new().unwrap();
        let size = ClientSurfaceSize {
            cols: 120,
            rows: 40,
        };
        for _ in 0..20 {
            std::hint::black_box(renderer.compose(&model, &views, size, &chrome).unwrap());
        }
        renderer.profile_ns = [0; 5];
        let mut drop_ns = 0;
        for _ in 0..1000 {
            let frame =
                std::hint::black_box(renderer.compose(&model, &views, size, &chrome).unwrap());
            let start = std::time::Instant::now();
            drop(frame);
            drop_ns += start.elapsed().as_nanos();
        }
        eprintln!(
            "render_frame_drop panes={count} mean_us={}",
            drop_ns as f64 / 1_000_000.0
        );
        eprintln!(
            "render_stages panes={count} geometry/links/paint/frame/metadata_graphics mean_us={:?}",
            renderer.profile_ns.map(|n| n as f64 / 1_000_000.0)
        );
    }
}

#[test]
fn a_finished_screen_replays_each_row_from_column_zero() {
    // Core joins rows with a bare newline; a terminal would keep the column and
    // staircase the pane, which is what the client used to draw.
    assert_eq!(replay_lines("first\nsecond"), b"first\r\nsecond");
    assert_eq!(replay_lines("only"), b"only");
    assert_eq!(replay_lines(""), b"");
    assert_eq!(replay_lines("a\n\nb"), b"a\r\n\r\nb");
}

#[test]
fn a_replayed_finished_screen_keeps_rows_at_column_zero() {
    let id = PaneId::from_raw(1);
    let mut view = View::new(40, 8, 0).expect("view");
    view.process_output(id, &replay_lines("AAA\nBBB\nCCC"));
    let text = view.terminal.visible_text();
    let rows: Vec<&str> = text.lines().take(3).collect();
    assert_eq!(rows[0].trim_end(), "AAA");
    assert_eq!(
        rows[1].trim_end(),
        "BBB",
        "row two must start at column zero"
    );
    assert_eq!(
        rows[2].trim_end(),
        "CCC",
        "row three must start at column zero"
    );
}

#[test]
fn raw_newline_separated_rows_would_staircase_without_the_replay_fix() {
    // Characterizes the bug: a bare `\n` keeps the cursor column, so each row
    // drifts right. This is what the client drew before replay_lines.
    let id = PaneId::from_raw(1);
    let mut view = View::new(40, 8, 0).expect("view");
    view.process_output(id, b"AAA\nBBB");
    let text = view.terminal.visible_text();
    let rows: Vec<&str> = text.lines().take(2).collect();
    assert_eq!(rows[0].trim_end(), "AAA");
    assert!(
        rows[1].starts_with("   BBB"),
        "an LF-only row keeps the column: {:?}",
        rows[1]
    );
}
