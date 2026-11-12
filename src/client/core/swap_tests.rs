use super::*;
use serde_json::json;

fn fixture() -> (Model, PaneId, PaneId) {
    let mut model = Model::new("original-swap".into());
    let first = model.insert(json!({"address":"opaque/first"})).unwrap();
    let second = model.insert(json!({"address":"opaque/second"})).unwrap();
    model.focus(first).unwrap();
    (model, first, second)
}
fn explicit(source: &str, target: &str) -> actions::PaneSwapParams {
    actions::PaneSwapParams {
        source_pane_id: Some(source.into()),
        target_pane_id: Some(target.into()),
        ..Default::default()
    }
}
fn area() -> Rect {
    Rect::new(0, 0, 100, 28)
}

#[test]
fn original_directional_swap_uses_hidden_bsp_keeps_zoom_and_follows_source_identity() {
    let (mut model, first, second) = fixture();
    model.zoom_pane(first, actions::PaneZoomMode::On).unwrap();
    let before = model.layout().unwrap().panes(area());
    let params = actions::PaneSwapParams {
        direction: Some(actions::PaneDirection::Right),
        ..Default::default()
    };
    let params = pin_source(&model, params).unwrap();
    assert_eq!(params.pane_id.as_deref(), Some("opaque/first"));
    let result = model.swap_panes(&params, area()).unwrap();
    assert!(result.changed && result.reason.is_none() && result.layout.zoomed);
    assert_eq!(result.source_pane_id, "opaque/first");
    assert_eq!(result.target_pane_id.as_deref(), Some("opaque/second"));
    assert_eq!(result.focused_pane_id, "opaque/first");
    assert_eq!(model.focused(), Some(first));
    let after = model.layout().unwrap().panes(area());
    assert_eq!(after[0].id, second);
    assert_eq!(after[1].id, first);
    assert_eq!(after[0].rect, before[0].rect);
    assert_eq!(after[1].rect, before[1].rect);
    assert!(model.active_zoomed());
    assert_eq!(model.records.len(), 2);
}

#[test]
fn original_explicit_swap_activates_source_tab_without_losing_stable_ids() {
    let (mut model, first, second) = fixture();
    let tab = model.active_tab().unwrap().to_owned();
    let (_, other) = model
        .create_tab("core", json!({"address":"other/tab"}), None, true)
        .unwrap();
    assert_eq!(model.focused(), Some(other));
    let result = model
        .swap_panes(&explicit("opaque/second", "opaque/first"), area())
        .unwrap();
    assert!(result.changed);
    assert_eq!(model.active_tab(), Some(tab.as_str()));
    assert_eq!(model.focused(), Some(second));
    assert_eq!(model.id("opaque/first"), Some(first));
    assert_eq!(model.id("opaque/second"), Some(second));
}

#[test]
fn original_explicit_swap_noop_reasons_preserve_raw_ids_and_context_without_focus_changes() {
    let (mut model, first, _) = fixture();
    let original_tab = model.active_tab().unwrap().to_owned();
    let (_, other) = model
        .create_tab("core", json!({"address":"other/tab"}), None, true)
        .unwrap();
    let active_tab = model.active_tab().unwrap().to_owned();
    for (source, target, reason, context) in [
        (
            "opaque/first",
            "opaque/first",
            actions::PaneSwapReason::SamePane,
            original_tab.as_str(),
        ),
        (
            "opaque/first",
            "other/tab",
            actions::PaneSwapReason::CrossTab,
            original_tab.as_str(),
        ),
        (
            "missing/source",
            "opaque/first",
            actions::PaneSwapReason::NotFound,
            original_tab.as_str(),
        ),
        (
            "opaque/first",
            "missing/target",
            actions::PaneSwapReason::NotFound,
            original_tab.as_str(),
        ),
        (
            "missing/source",
            "missing/target",
            actions::PaneSwapReason::NotFound,
            active_tab.as_str(),
        ),
    ] {
        let context = context.to_owned();
        let doc = model.shared().unwrap();
        let before = model.revision;
        let result = model.swap_panes(&explicit(source, target), area()).unwrap();
        assert!(!result.changed);
        assert_eq!(result.reason, Some(reason));
        assert_eq!(result.source_pane_id, source);
        assert_eq!(result.target_pane_id.as_deref(), Some(target));
        assert_eq!(result.layout.tab_id, context);
        assert_eq!(model.shared().unwrap(), doc);
        assert_eq!(model.revision, before);
        assert_eq!(model.focused(), Some(other));
    }
    assert_eq!(model.id("opaque/first"), Some(first));
}

#[test]
fn original_swap_rejects_invalid_shapes_and_missing_directional_sources() {
    let (mut model, _, _) = fixture();
    for (params, code) in [
        (actions::PaneSwapParams::default(), "invalid_pane_swap"),
        (
            actions::PaneSwapParams {
                direction: Some(actions::PaneDirection::Right),
                ..explicit("opaque/first", "opaque/second")
            },
            "invalid_pane_swap",
        ),
        (
            actions::PaneSwapParams {
                source_pane_id: Some("opaque/first".into()),
                ..Default::default()
            },
            "invalid_pane_swap",
        ),
        (
            actions::PaneSwapParams {
                target_pane_id: Some("opaque/second".into()),
                ..Default::default()
            },
            "invalid_pane_swap",
        ),
        (
            actions::PaneSwapParams {
                pane_id: Some("missing".into()),
                direction: Some(actions::PaneDirection::Right),
                ..Default::default()
            },
            "pane_not_found",
        ),
    ] {
        let before = model.description();
        let error = model.swap_panes(&params, area()).unwrap_err();
        assert_eq!(error_code(&error), Some(code));
        assert_eq!(before, model.description());
    }
}

#[test]
fn original_no_neighbor_swap_is_a_noop_and_pinned_source_never_redirects_after_retirement() {
    let (mut model, first, second) = fixture();
    let params = pin_source(
        &model,
        actions::PaneSwapParams {
            direction: Some(actions::PaneDirection::Left),
            ..Default::default()
        },
    )
    .unwrap();
    let before = model.description();
    let result = model.swap_panes(&params, area()).unwrap();
    assert!(!result.changed);
    assert_eq!(result.reason, Some(actions::PaneSwapReason::NoNeighbor));
    assert!(result.target_pane_id.is_none());
    assert_eq!(before, model.description());
    model.retire(first).unwrap();
    assert_eq!(model.focused(), Some(second));
    assert_eq!(
        error_code(&model.swap_panes(&params, area()).unwrap_err()),
        Some("pane_not_found")
    );
}
