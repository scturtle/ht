//! Cross-layer atomic-paint regressions: real parser/renderer, mock Core sink.
use super::*;
use crate::client::core::transport::{DATA, RESIZE};
use std::time::Duration;

fn fixture(count: usize) -> (Session, Vec<PaneId>) {
    let mut model = Model::new("synchronized-test".into());
    let ids = (0..count)
        .map(|i| {
            model
                .insert(json!({
                    "address":format!("p{i}"), "state":"live", "pid":7+i
                }))
                .unwrap()
        })
        .collect::<Vec<_>>();
    model.focus(ids[0]).unwrap();
    let config = crate::config::Config::default();
    let mut ui = view::ChromeConfig::from_config(&config);
    ui.pane_borders = crate::config::PaneBordersConfig::Off;
    ui.pane_gaps = false;
    let infos = view::pane_infos(&model, Rect::new(0, 0, 80, 24), &ui);
    let views = infos
        .iter()
        .map(|p| {
            let channel = ids.iter().position(|id| *id == p.id).unwrap() as u32 + 1;
            (
                p.id,
                View::new(
                    p.inner_rect.width.max(1),
                    p.inner_rect.height.max(1),
                    channel,
                )
                .unwrap(),
            )
        })
        .collect::<HashMap<_, _>>();
    let channels = views.iter().map(|(id, v)| (v.channel, *id)).collect();
    let (_send, receive) = std::sync::mpsc::channel();
    let mut renderer = Renderer::new().unwrap();
    renderer.set_cell_size((8, 16));
    (
        Session {
            core: super::super::transport::test_core(receive),
            options: None,
            model,
            shell: Vec::new(),
            scrollback: config.advanced.clone(),
            views,
            channels,
            document_version: 1,
            size: ClientSurfaceSize { cols: 80, rows: 24 },
            cells: (8, 16),
            ui,
            renderer,
            dirty: true,
            snapshot_dirty: true,
            synchronized_deadline: None,
            focused: true,
            last_focus: None,
            home: None,
            _lock: None,
            events: Vec::new(),
            seq: 0,
            theme: Default::default(),
            display: json!({}),
        },
        ids,
    )
}

#[test]
fn turn_status_does_not_close_a_live_pane_or_corrupt_its_lifecycle() {
    for state in [
        json!("done"),
        json!("blocked"),
        json!("error"),
        json!("idle"),
        json!(null),
    ] {
        let (mut session, ids) = fixture(1);
        let id = ids[0];
        session.core.events_channel = Some(90);
        data(&mut session, id, b"COMMITTED_ANSWER");
        surface(&mut session).unwrap();
        let channel = session.views[&id].channel;
        session
            .consume(Frame {
                channel: 90,
                kind: DATA,
                payload: serde_json::to_vec(&json!({
                    "method":"event", "params": {
                        "seq":1, "terminal":"p0", "type":"turn",
                        "data":{"turn":1,"state":state}
                    }
                }))
                .unwrap()
                .into_iter()
                .chain([b'\n'])
                .collect(),
            })
            .unwrap();
        let facts = &session.model.records[&id].facts;
        assert_eq!(facts["state"], "live", "turn state is not process state");
        assert_eq!(facts["turn"], 1);
        assert_eq!(facts["pid"], 7);
        assert!(!session.views[&id].closed, "a completed task is not CLOSE");
        assert_eq!(session.views[&id].channel, channel);
        // If no footer DATA follows, ordinary input must still be admitted;
        // previously it incorrectly reported "Core program has finished".
        let input = super::super::input::for_target(
            &session.model,
            &session.views,
            "p0",
            &[crate::protocol::ClientPaneInputEvent::Paste("x".into())],
        )
        .unwrap();
        assert_eq!(input, Some((channel, b"x".to_vec())));
        // Pi's final footer often follows the turn event. It must not cause a
        // transport failure/recovery that wipes and replays the committed pane.
        data(&mut session, id, b" FINAL_FOOTER");
        let frame = surface(&mut session).unwrap();
        assert!(text(&frame).contains("COMMITTED_ANSWER FINAL_FOOTER"));
    }
}

#[test]
fn foreground_event_updates_nested_facts_not_the_program_identity() {
    let (mut session, ids) = fixture(1);
    let id = ids[0];
    session.core.events_channel = Some(90);
    session.model.records.get_mut(&id).unwrap().facts["cwd"] = json!("/launch");
    session.model.records.get_mut(&id).unwrap().facts["argv"] = json!(["shell"]);
    for (i, foreground) in [
        json!({"pid":42,"cwd":"/working","argv":["pi"],"name":"pi"}),
        json!(null),
    ]
    .into_iter()
    .enumerate()
    {
        session
            .consume(Frame {
                channel: 90,
                kind: DATA,
                payload: serde_json::to_vec(&json!({
                    "method":"event", "params": {
                        "seq":i+1, "terminal":"p0", "type":"foreground",
                        "data":foreground
                    }
                }))
                .unwrap()
                .into_iter()
                .chain([b'\n'])
                .collect(),
            })
            .unwrap();
        let facts = &session.model.records[&id].facts;
        assert_eq!(facts["foreground"], foreground);
        assert_eq!(facts["last_cwd"], "/working");
        assert_eq!(facts["pid"], 7);
        assert_eq!(facts["cwd"], "/launch");
        assert_eq!(facts["argv"], json!(["shell"]));
        assert_eq!(facts["state"], "live");
        assert!(!session.views[&id].closed);
    }
}

fn data(session: &mut Session, id: PaneId, bytes: &[u8]) -> Vec<ServerMessage> {
    session
        .consume(Frame {
            channel: session.views[&id].channel,
            kind: DATA,
            payload: bytes.to_vec(),
        })
        .unwrap()
}
fn surface(session: &mut Session) -> Option<PaneSurfaceFrame> {
    session
        .publish()
        .unwrap()
        .into_iter()
        .find_map(|m| match m {
            ServerMessage::PaneSurface(s) => Some(s),
            _ => None,
        })
}
fn text(frame: &PaneSurfaceFrame) -> String {
    frame
        .frame
        .cells
        .iter()
        .map(|c| c.symbol.as_str())
        .collect()
}

#[test]
fn pending_dirty_cannot_publish_synchronized_intermediate_clear() {
    for pending in [false, true] {
        let (mut s, ids) = fixture(1);
        let id = ids[0];
        data(&mut s, id, b"BEFORE_SYNCHRONIZED_PAINT");
        let before = surface(&mut s).unwrap();
        if pending {
            data(&mut s, id, b"\x1b[0m");
        }
        data(&mut s, id, b"\x1b[?2026h\x1b[2J\x1b[H");
        if let Some(held) = surface(&mut s) {
            assert_eq!(held.frame, before.frame);
            assert_eq!(
                held.panes[0].content_revision,
                before.panes[0].content_revision
            );
        }
        assert!(!s.dirty, "holding must not busy-loop the worker");
        assert!(s.synchronized_deadline.is_some());
        data(&mut s, id, b"AFTER_SYNCHRONIZED_PAINT\x1b[?2026l");
        assert!(text(&surface(&mut s).unwrap()).contains("AFTER_SYNCHRONIZED_PAINT"));
        assert!(s.synchronized_deadline.is_none());
    }
}

#[test]
fn synchronized_pane_does_not_block_other_panes_or_snapshot_revisions() {
    let (mut s, ids) = fixture(2);
    data(&mut s, ids[0], b"FIRST_COMMITTED");
    data(&mut s, ids[1], b"SECOND_OLD");
    let before = surface(&mut s).unwrap();
    data(&mut s, ids[0], b"\x1b[?2026h\x1b[2J\x1b[HFIRST_PARTIAL");
    data(&mut s, ids[1], b"\rSECOND_NEW");
    let held = surface(&mut s).unwrap();
    assert!(text(&held).contains("FIRST_COMMITTED"));
    assert!(!text(&held).contains("FIRST_PARTIAL"));
    assert!(text(&held).contains("SECOND_NEW"));
    assert_eq!(
        held.panes
            .iter()
            .find(|p| p.pane_id == "p0")
            .unwrap()
            .content_revision,
        before
            .panes
            .iter()
            .find(|p| p.pane_id == "p0")
            .unwrap()
            .content_revision
    );
    s.model.revision += 1;
    s.snapshot_dirty = true;
    let metadata = surface(&mut s).unwrap();
    assert_eq!(metadata.projection_revision, s.model.revision);
    assert_eq!(metadata.frame, held.frame);
    data(&mut s, ids[0], b"\x1b[?2026l");
    assert!(text(&surface(&mut s).unwrap()).contains("FIRST_PARTIAL"));
}

#[test]
fn synchronized_cursor_links_and_styles_stay_committed_until_end() {
    let (mut s, ids) = fixture(1);
    let id = ids[0];
    data(
        &mut s,
        id,
        b"\x1b[31m\x1b]8;;https://old.example\x07OLD_LINK\x1b]8;;\x07\x1b[6 q",
    );
    let before = surface(&mut s).unwrap();
    assert_eq!(before.frame.hyperlinks, vec!["https://old.example"]);
    data(&mut s, id, b"\x1b[0m");
    data(&mut s,id,b"\x1b[?2026h\x1b[2J\x1b[H\x1b[32m\x1b]8;;https://new.example\x07NEW_LINK\x1b]8;;\x07\x1b[2 q");
    let held = surface(&mut s).unwrap();
    assert_eq!(held.frame, before.frame);
    data(&mut s, id, b"\x1b[?2026l");
    let after = surface(&mut s).unwrap();
    assert_eq!(after.frame.hyperlinks, vec!["https://new.example"]);
    assert!(text(&after).contains("NEW_LINK"));
    assert_ne!(after.frame.cursor, before.frame.cursor);
}

#[test]
fn synchronized_geometry_uses_latest_grid_and_clips_wide_cells_without_leaking() {
    let (mut s, ids) = fixture(1);
    let id = ids[0];
    data(&mut s, id, "A中文".as_bytes());
    surface(&mut s).unwrap();
    data(&mut s, id, b"\x1b[?2026h\x1b[2J\x1b[H");
    s.consume(Frame {
        channel: s.views[&id].channel,
        kind: RESIZE,
        payload: [2u16.to_le_bytes(), 5u16.to_le_bytes()].concat(),
    })
    .unwrap();
    s.size = ClientSurfaceSize { cols: 2, rows: 5 };
    let held = surface(&mut s).unwrap();
    assert_eq!((held.frame.width, held.frame.height), (2, 5));
    assert_eq!((s.views[&id].size.0, s.views[&id].size.1), (2, 5));
    assert_eq!(held.frame.cells[0].symbol, "A");
    assert_eq!(
        held.frame.cells[1].symbol, " ",
        "cannot leave half a wide glyph"
    );
    data(&mut s, id, b"OK\x1b[?2026l");
    assert!(text(&surface(&mut s).unwrap()).contains("OK"));
}

#[test]
fn synchronized_timeout_wakes_and_releases_only_the_local_paint_hold() {
    let (mut s, ids) = fixture(1);
    let id = ids[0];
    data(&mut s, id, b"COMMITTED");
    surface(&mut s).unwrap();
    data(&mut s, id, b"\x1b[0m");
    data(&mut s, id, b"\x1b[?2026h\x1b[2J\x1b[HPARTIAL");
    surface(&mut s).unwrap();
    let until = s.synchronized_deadline.unwrap();
    assert!(until.saturating_duration_since(Instant::now()) <= Duration::from_secs(1));
    assert!(s.wait_deadline().is_some_and(|t| t <= until));
    assert!(!s.dirty);
    s.expire_synchronized(until);
    assert!(s.dirty);
    assert!(s.synchronized_deadline.is_none());
    assert!(!s.views[&id].terminal.ghostty.synchronized_output_state().0);
    assert!(text(&surface(&mut s).unwrap()).contains("PARTIAL"));
    data(&mut s, id, b"_CONTINUED");
    assert!(s.synchronized_deadline.is_none());
    assert!(text(&surface(&mut s).unwrap()).contains("PARTIAL_CONTINUED"));
    data(&mut s, id, b"\x1b[?2026l");
    surface(&mut s).unwrap();
    data(&mut s, id, b"\x1b[?2026h");
    assert!(
        s.synchronized_deadline.is_some(),
        "a new update gets its own bounded wait"
    );
}

#[test]
fn close_and_replaced_attach_cannot_keep_a_frozen_pane_or_stale_timer() {
    let (mut s, ids) = fixture(1);
    let id = ids[0];
    data(&mut s, id, b"OLD_ATTACHMENT");
    surface(&mut s).unwrap();
    data(&mut s, id, b"\x1b[0m");
    data(&mut s, id, b"\x1b[?2026h\x1b[2J\x1b[H");
    assert!(text(&surface(&mut s).unwrap()).contains("OLD_ATTACHMENT"));
    s.consume(Frame {
        channel: s.views[&id].channel,
        kind: CLOSE,
        payload: vec![],
    })
    .unwrap();
    surface(&mut s).unwrap();
    assert!(s.synchronized_deadline.is_none());
    s.views.insert(id, View::new(80, 24, 42).unwrap());
    s.channels.clear();
    s.channels.insert(42, id);
    data(&mut s, id, b"\x1b[?2026h\x1b[2J\x1b[HUNCOMMITTED");
    s.dirty = true;
    let first = surface(&mut s).unwrap();
    assert!(!text(&first).contains("OLD_ATTACHMENT"));
    assert!(!text(&first).contains("UNCOMMITTED"));
    data(&mut s, id, b"\x1b[?2026l");
    assert!(text(&surface(&mut s).unwrap()).contains("UNCOMMITTED"));
}

#[test]
fn synchronized_effects_and_input_protocols_are_not_gated_on_paint() {
    let (mut s, ids) = fixture(1);
    let id = ids[0];
    data(&mut s, id, b"COMMITTED");
    surface(&mut s).unwrap();
    data(&mut s, id, b"\x1b[0m");
    let effects = data(
        &mut s,
        id,
        b"\x1b[?2026h\x1b[2J\x1b[H\x07\x1b]52;c;Y29weQ==\x07\x1b[?1000h",
    );
    assert!(effects
        .iter()
        .any(|m| matches!(m, ServerMessage::TerminalBell { .. })));
    assert!(effects
        .iter()
        .any(|m| matches!(m, ServerMessage::Clipboard { .. })));
    let messages = s.publish().unwrap();
    assert!(messages
        .iter()
        .any(|m| matches!(m, ServerMessage::MouseCapture { enabled: true, .. })));
    assert!(!s.dirty);
}

#[test]
fn synchronized_images_keep_the_committed_scene_without_reuploading_or_losing_delivery() {
    crate::kitty_graphics::set_enabled(true);
    let (mut s, ids) = fixture(1);
    let id = ids[0];
    data(
        &mut s,
        id,
        b"\x1b_Ga=T,f=32,i=1,p=1,s=1,v=1,c=1,r=1,q=2;/wAA/w==\x1b\\",
    );
    let before = surface(&mut s).unwrap();
    assert_eq!(before.graphics.assets.len(), 1);
    assert_eq!(before.graphics.placements.len(), 1);
    data(&mut s, id, b"\x1b[0m");
    data(&mut s,id,b"\x1b[?2026h\x1b[2J\x1b[H\x1b_Ga=d,d=A,q=2;\x1b\\\x1b_Ga=T,f=32,i=2,p=1,s=1,v=1,c=1,r=1,q=2;AP8A/w==\x1b\\");
    let held = surface(&mut s).unwrap();
    assert_eq!(held.graphics.placements, before.graphics.placements);
    assert!(held.graphics.assets.is_empty());
    s.dirty = true;
    let again = surface(&mut s).unwrap();
    assert_eq!(again.graphics.placements, before.graphics.placements);
    assert!(again.graphics.assets.is_empty());
    data(&mut s, id, b"\x1b[?2026l");
    let after = surface(&mut s).unwrap();
    assert_eq!(after.graphics.assets.len(), 1);
    assert_eq!(after.graphics.placements.len(), 1);
    assert_ne!(
        after.graphics.placements[0].asset,
        before.graphics.placements[0].asset
    );
}

#[test]
fn synchronized_hidden_output_keeps_early_exit_and_reveal_uses_completed_content() {
    let (mut s, ids) = fixture(2);
    data(&mut s, ids[0], b"FIRST_OLD");
    data(&mut s, ids[1], b"SECOND_ONLY");
    surface(&mut s).unwrap();
    let tab = s.model.active_tab().unwrap().to_owned();
    s.model.focus(ids[1]).unwrap();
    s.model.zoomed_tabs.insert(tab.clone());
    s.views.get_mut(&ids[0]).unwrap().visible = false;
    s.dirty = true;
    assert!(text(&surface(&mut s).unwrap()).contains("SECOND_ONLY"));
    data(&mut s, ids[0], b"\x1b[?2026h\x1b[2J\x1b[HFIRST_FINISHED");
    assert!(surface(&mut s).is_none());
    data(&mut s, ids[0], b"\x1b[?2026l");
    assert!(surface(&mut s).is_none());
    assert!(s.synchronized_deadline.is_none());
    s.model.zoomed_tabs.remove(&tab);
    s.model.focus(ids[0]).unwrap();
    s.views.get_mut(&ids[0]).unwrap().visible = true;
    s.dirty = true;
    assert!(text(&surface(&mut s).unwrap()).contains("FIRST_FINISHED"));
}

#[test]
fn complete_sync_in_one_data_frame_needs_no_wait_and_recovery_clears_frozen_content() {
    let (mut s, ids) = fixture(1);
    let id = ids[0];
    data(&mut s, id, b"\x1b[?2026hCOMPLETE\x1b[?2026l");
    assert!(text(&surface(&mut s).unwrap()).contains("COMPLETE"));
    assert!(s.synchronized_deadline.is_none());
    data(&mut s, id, b"\x1b[0m");
    data(&mut s, id, b"\x1b[?2026h\x1b[2J\x1b[HUNCOMMITTED");
    assert!(text(&surface(&mut s).unwrap()).contains("COMPLETE"));
    s.renderer.resume_connection();
    s.dirty = true;
    let fresh = surface(&mut s).unwrap();
    assert!(!text(&fresh).contains("COMPLETE"));
    assert!(!text(&fresh).contains("UNCOMMITTED"));
    data(&mut s, id, b"\x1b[?2026l");
    assert!(text(&surface(&mut s).unwrap()).contains("UNCOMMITTED"));
}

#[test]
fn reserved_graphics_slots_do_not_upload_or_spin_on_unrepresentable_assets() {
    use crate::kitty_graphics::{surface::*, HostCellSize};
    crate::kitty_graphics::set_enabled(true);
    let (mut s, ids) = fixture(1);
    let id = ids[0];
    data(
        &mut s,
        id,
        b"\x1b_Ga=T,f=32,i=1,p=1,s=1,v=1,c=1,r=1,q=2;/wAA/w==\x1b\\",
    );
    let panes = [ScenePane {
        id,
        public_id: "p0".into(),
        inner_rect: Rect::new(0, 0, 80, 24),
        terminal: &s.views[&id].terminal,
    }];
    let cells = HostCellSize {
        width_px: 8,
        height_px: 16,
    };
    let (none, cache, _) = collect_scene_with_limit(&panes, &[], cells, &Default::default(), 0);
    assert!(none.placements.is_empty() && none.assets.is_empty());
    assert!(!cache.has_pending());
    let (one, _, _) = collect_scene_with_limit(&panes, &[], cells, &cache, 1);
    assert_eq!(one.placements.len(), 1);
    assert_eq!(one.assets.len(), 1);
}

#[test]
fn synchronized_image_moves_and_clips_with_latest_pane_geometry() {
    crate::kitty_graphics::set_enabled(true);
    let (mut s, ids) = fixture(2);
    let id = ids[1];
    data(
        &mut s,
        id,
        b"\x1b[1;18H\x1b_Ga=T,f=32,i=1,p=1,s=1,v=1,c=4,r=1,q=2;/wAA/w==\x1b\\",
    );
    let before = surface(&mut s).unwrap();
    assert_eq!(before.graphics.placements[0].x, 57);
    data(&mut s, id, b"\x1b[?2026h\x1b[2J\x1b_Ga=d,d=A,q=2;\x1b\\");
    s.consume(Frame {
        channel: s.views[&id].channel,
        kind: RESIZE,
        payload: [20u16.to_le_bytes(), 24u16.to_le_bytes()].concat(),
    })
    .unwrap();
    s.size.cols = 40;
    let held = surface(&mut s).unwrap();
    assert_eq!(held.graphics.placements.len(), 1);
    let image = &held.graphics.placements[0];
    assert_eq!(image.asset, before.graphics.placements[0].asset);
    assert_eq!((image.x, image.cols), (37, 3));
    assert!(u32::from(image.x) + image.cols <= 40);
    assert!(held.graphics.assets.is_empty());
    s.size.cols = 20;
    s.dirty = true;
    let cropped = surface(&mut s).unwrap();
    assert!(cropped.graphics.placements.is_empty());
    assert!(cropped.graphics.retained_assets.contains(&image.asset));
    s.size.cols = 40;
    s.dirty = true;
    let restored = surface(&mut s).unwrap();
    assert_eq!(restored.graphics.placements, held.graphics.placements);
    assert!(restored.graphics.assets.is_empty());
    data(&mut s, id, b"\x1b[?2026l");
    assert!(surface(&mut s).unwrap().graphics.placements.is_empty());
}
