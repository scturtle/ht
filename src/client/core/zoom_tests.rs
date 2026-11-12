use super::super::{shared, view};
use super::*;
use crate::{
    config::{Config, PaneBordersConfig},
    protocol::ClientSurfaceSize,
};
use ratatui::{layout::Rect, widgets::Borders};
use serde_json::json;
use std::collections::HashMap;

fn split_model() -> (Model, PaneId, PaneId) {
    let mut model = Model::new("zoom-characterization".into());
    let first = model.insert(json!({"address":"opaque/root"})).unwrap();
    let second = model.insert(json!({"address":"remote/second"})).unwrap();
    model.focus(first).unwrap();
    (model, first, second)
}

#[test]
fn original_zoom_transition_preserves_single_pane_and_idempotence_reasons() {
    use actions::{PaneZoomMode as Mode, PaneZoomReason as Reason};
    for current in [false, true] {
        for mode in [Mode::Toggle, Mode::On, Mode::Off] {
            for focus_changed in [false, true] {
                let single = crate::app::actions::pane_zoom(current, 1, mode, focus_changed);
                assert!(!single.changed);
                assert_eq!(single.zoomed, current);
                assert_eq!(single.focus_changed, focus_changed);
                assert_eq!(single.reason, Some(Reason::SinglePane));
                let outcome = crate::app::actions::pane_zoom(current, 2, mode, focus_changed);
                let expected_reason = match (&mode, current) {
                    (Mode::On, true) => Some(Reason::AlreadyZoomed),
                    (Mode::Off, false) => Some(Reason::AlreadyUnzoomed),
                    _ => None,
                };
                assert_eq!(outcome.changed, expected_reason.is_none());
                assert_eq!(outcome.reason, expected_reason);
                assert_eq!(outcome.focus_changed, focus_changed);
                assert_eq!(
                    outcome.zoomed,
                    match mode {
                        Mode::Toggle => !current,
                        Mode::On => true,
                        Mode::Off => false,
                    }
                );
            }
        }
    }
}

#[test]
fn original_zoom_focuses_even_when_on_is_idempotent_but_noop_does_not_dirty_revision() {
    let (mut model, first, second) = split_model();
    let shared = model.shared().unwrap();
    let outcome = model.zoom_pane(first, actions::PaneZoomMode::On).unwrap();
    assert!(outcome.changed && !outcome.focus_changed && outcome.zoomed);
    let before = model.revision;
    let outcome = model.zoom_pane(first, actions::PaneZoomMode::On).unwrap();
    assert!(!outcome.changed && !outcome.focus_changed);
    assert_eq!(model.revision, before);
    let outcome = model.zoom_pane(second, actions::PaneZoomMode::On).unwrap();
    assert!(!outcome.changed && outcome.focus_changed && outcome.zoomed);
    assert_eq!(model.focused(), Some(second));
    assert_eq!(model.shared().unwrap(), shared);
    assert!(model.description().unwrap().zoomed);
    assert!(model.snapshot().tabs[0].zoomed);
    let layout = model.layout_snapshot(Rect::new(0, 0, 100, 28)).unwrap();
    assert!(layout.zoomed);
    assert_eq!(
        layout.panes.len(),
        2,
        "original layout API exposes the underlying BSP even while zoomed"
    );
    assert_eq!(layout.splits.len(), 1);
    assert!(
        !model
            .zoom_pane(first, actions::PaneZoomMode::Off)
            .unwrap()
            .zoomed
    );
    let before = model.description();
    assert!(model
        .zoom_pane(PaneId::from_raw(u32::MAX), actions::PaneZoomMode::Toggle)
        .is_err());
    assert_eq!(model.description(), before);
}

#[test]
fn original_zoom_single_pane_in_other_tab_still_focuses_without_zooming() {
    let (mut model, first, _) = split_model();
    let (_, third) = model
        .create_tab(
            super::super::model::WORKSPACE,
            json!({"address":"another/tab"}),
            None,
            false,
        )
        .unwrap();
    model.zoom_pane(first, actions::PaneZoomMode::On).unwrap();
    let result = model.zoom_pane(third, actions::PaneZoomMode::Off).unwrap();
    assert!(!result.changed && result.focus_changed && !result.zoomed);
    assert_eq!(result.reason, Some(actions::PaneZoomReason::SinglePane));
    assert_eq!(model.focused(), Some(third));
    model.focus(first).unwrap();
    assert!(
        model.active_zoomed(),
        "each tab retains its own zoom across tab activation"
    );
}

#[test]
fn zoom_only_paints_focused_pane_hides_dividers_and_reuses_original_outer_borders() {
    let (mut model, first, second) = split_model();
    let area = Rect::new(0, 0, 100, 28);
    let mut config = view::ChromeConfig::from_config(&Config::default());
    config.pane_borders = PaneBordersConfig::Off;
    let mut views = HashMap::new();
    let mut first_view = view::View::new(100, 28, 1).unwrap();
    first_view.process_output(first, b"FIRST_VISIBLE");
    views.insert(first, first_view);
    // Deliberately omit the hidden terminal: zoom geometry must not touch it.
    let mut renderer = view::Renderer::new().unwrap();
    model.zoom_pane(first, actions::PaneZoomMode::On).unwrap();
    let surface = renderer
        .compose(
            &model,
            &views,
            ClientSurfaceSize {
                cols: 100,
                rows: 28,
            },
            &config,
        )
        .unwrap();
    assert_eq!(surface.panes.len(), 1);
    assert_eq!(surface.panes[0].pane_id, "opaque/root");
    assert_eq!(surface.panes[0].rect, area.into());
    assert!(
        surface.splits.is_empty(),
        "hidden split boundaries must not intercept mouse input"
    );
    model.focus(second).unwrap();
    let panes = view::pane_infos(&model, area, &config);
    assert_eq!(panes.len(), 1);
    assert_eq!(
        panes[0].id, second,
        "original navigation changes the full-screen pane without unzooming"
    );
    for borders in [
        PaneBordersConfig::Off,
        PaneBordersConfig::Auto,
        PaneBordersConfig::Always,
    ] {
        for outer in [false, true] {
            config.pane_borders = borders;
            config.pane_outer_borders = outer;
            config.pane_gaps = true;
            let pane = view::pane_infos(&model, area, &config).remove(0);
            let expected = if borders != PaneBordersConfig::Off && outer {
                Borders::ALL
            } else {
                Borders::NONE
            };
            assert_eq!(pane.borders, expected);
            assert_eq!(pane.rect, area, "zoom does not retain inter-pane gaps");
            assert_eq!(pane.inner_rect, crate::ui::pane_inner_rect(area, expected));
        }
    }
    model.zoom_pane(second, actions::PaneZoomMode::Off).unwrap();
    let surface = renderer
        .compose(
            &model,
            &views,
            ClientSurfaceSize {
                cols: 100,
                rows: 28,
            },
            &config,
        )
        .unwrap();
    assert_eq!(
        surface.panes.len(),
        1,
        "only the fixture's one terminal has a View"
    );
    assert_eq!(
        surface.splits.len(),
        1,
        "unzoom restores the original BSP hit targets"
    );
}

#[test]
fn original_public_layout_geometry_applies_chrome_to_hidden_bsp_even_when_zoomed() {
    let (mut model, first, _) = split_model();
    model.zoom_pane(first, actions::PaneZoomMode::On).unwrap();
    let tab = model.active_tab().unwrap();
    let area = Rect::new(3, 2, 100, 28);
    let mut config = view::ChromeConfig::from_config(&Config::default());
    for borders in [PaneBordersConfig::Off, PaneBordersConfig::Always] {
        config.pane_borders = borders;
        config.pane_gaps = true;
        config.pane_outer_borders = true;
        let snapshot = view::api_layout_snapshot(&model, tab, area, &config).unwrap();
        assert!(snapshot.zoomed);
        assert_eq!(snapshot.panes.len(), 2);
        assert_eq!(snapshot.splits.len(), 1);
        let original = crate::ui::apply_pane_chrome(
            model.layout().unwrap().panes(area),
            config.pane_borders,
            config.pane_gaps,
            config.pane_outer_borders,
        );
        for (api, native) in snapshot.panes.iter().zip(&original) {
            assert_eq!(api.rect, native.rect.into());
            assert_eq!(api.focused, native.is_focused);
        }
    }
}

#[test]
fn zoom_is_client_local_and_survives_metadata_ratio_swap_but_not_membership_changes() {
    let (mut a, first, second) = split_model();
    let facts = a
        .records
        .values()
        .map(|r| (r.address.clone(), r.facts.clone()))
        .collect();
    let mut b = Model::new("other-client".into());
    b.install(a.shared().unwrap(), &facts).unwrap();
    a.zoom_pane(first, actions::PaneZoomMode::On).unwrap();
    assert_eq!(a.shared().unwrap(), b.shared().unwrap());
    assert!(!b.snapshot().tabs[0].zoomed);
    // Shared ratio changes and pane swaps keep each client's original zoom.
    let tab = a.active_tab().unwrap().to_owned();
    b.layouts.get_mut(&tab).unwrap().set_ratio_at(&[], 0.7);
    let b_first = b.id("opaque/root").unwrap();
    let b_second = b.id("remote/second").unwrap();
    b.layouts
        .get_mut(&tab)
        .unwrap()
        .swap_panes(b_first, b_second);
    a.install(b.shared().unwrap(), &facts).unwrap();
    assert!(a.active_zoomed());
    assert_eq!(a.focused(), Some(first));
    let third = b.insert(json!({"address":"third/member"})).unwrap();
    let facts = b
        .records
        .values()
        .map(|r| (r.address.clone(), r.facts.clone()))
        .collect();
    a.install(b.shared().unwrap(), &facts).unwrap();
    assert!(
        !a.active_zoomed(),
        "native split resets zoom, including remote insertions"
    );
    a.zoom_pane(second, actions::PaneZoomMode::On).unwrap();
    b.retire(third).unwrap();
    a.install(b.shared().unwrap(), &facts).unwrap();
    assert!(
        !a.active_zoomed(),
        "native close resets zoom, including remote removals"
    );
    a.zoom_pane(second, actions::PaneZoomMode::On).unwrap();
    a.retire(first).unwrap();
    assert!(a.zoomed_tabs.is_empty());
    assert!(
        !a.zoom_pane(second, actions::PaneZoomMode::Toggle)
            .unwrap()
            .zoomed
    );
}

#[test]
fn revealing_a_passive_pane_announces_its_grid_without_resizing_hidden_programs() {
    let mut view = view::View::new(50, 27, 1).unwrap();
    assert_eq!(
        view.request_visible_size(true, (50, 27, 0, 0)),
        Some((false, false))
    );
    assert_eq!(view.request_visible_size(false, (100, 27, 8, 16)), None);
    assert_eq!(
        view.requested,
        (50, 27, 0, 0),
        "hidden layouts must not overwrite remembered requests"
    );
    view.authoritative_size((100, 27, 0, 0));
    assert_eq!(
        view.request_visible_size(true, (50, 27, 0, 0)),
        Some((true, false)),
        "revealing after another client resized must send RESIZE even for an unchanged request"
    );
    assert_eq!(
        view.size,
        (100, 27, 0, 0),
        "only Core's ordered RESIZE updates the parser grid"
    );
    assert_eq!(
        view.request_visible_size(true, (50, 27, 0, 0)),
        Some((false, false)),
        "ordinary snapshots must not fight the geometry leader"
    );
}

#[test]
fn local_split_clears_zoom_but_refreshing_core_facts_does_not() {
    let (mut model, first, _) = split_model();
    model.zoom_pane(first, actions::PaneZoomMode::On).unwrap();
    model
        .insert(json!({"address":"opaque/root","status":{"state":"working"}}))
        .unwrap();
    assert!(model.active_zoomed());
    model.insert(json!({"address":"new/split"})).unwrap();
    assert!(!model.active_zoomed());
}

#[test]
fn zoom_preferences_restore_only_existing_multi_pane_tabs_and_old_preferences_remain_valid() {
    let (mut model, first, _) = split_model();
    let (single_tab, _) = model
        .create_tab(
            super::super::model::WORKSPACE,
            json!({"address":"single/tab"}),
            None,
            false,
        )
        .unwrap();
    let tab = model.context(first).unwrap().1.id.clone();
    let path = std::env::temp_dir().join(format!(
        "herdr-zoom-prefs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    std::fs::write(
        path.join("preferences.json"),
        serde_json::to_vec(&json!({"zoomed_tabs":[tab, single_tab, "deleted/tab", 1, null]}))
            .unwrap(),
    )
    .unwrap();
    shared::restore_preferences(&path, &mut model).unwrap();
    assert_eq!(model.zoomed_tabs, [tab].into_iter().collect());
    assert!(model.shared().unwrap().validate().is_ok());
    std::fs::write(path.join("preferences.json"), b"{}").unwrap();
    shared::restore_preferences(&path, &mut model).unwrap();
    assert!(
        model.active_zoomed(),
        "an old preference file must not reject the connection"
    );
    std::fs::remove_dir_all(path).unwrap();
}
