//! Surface graphics collection tests using real terminal image uploads.

use super::*;
use crate::layout::PaneId;
use crate::pane::{GhosttyPaneTerminal, PaneTerminal};

/// One 2x1 RGBA image placed at the cursor, exactly as a program uploads it.
const UPLOAD: &[u8] = b"\x1b[H\x1b_Ga=T,f=32,i=7,p=1,s=1,v=1,c=1,r=1,q=2;/wAA/w==\x1b\\";
const CELL: HostCellSize = HostCellSize {
    width_px: 8,
    height_px: 16,
};

fn pane(width: u16, height: u16) -> PaneTerminal {
    let mut terminal = crate::ghostty::Terminal::new(width, height, 0).unwrap();
    terminal.enable_kitty_graphics().unwrap();
    // A real attach resizes the pane to the client grid with its cell pixels.
    terminal
        .resize(width, height, CELL.width_px, CELL.height_px)
        .unwrap();
    PaneTerminal::new(GhosttyPaneTerminal::new(terminal).unwrap())
}

fn collect_for(
    pane: &PaneTerminal,
    rect: Rect,
    delivered: &DeliveryCache,
) -> (SurfaceGraphicsScene, DeliveryCache) {
    let panes = [ScenePane {
        id: PaneId::from_raw(1),
        public_id: "t1".into(),
        inner_rect: rect,
        terminal: pane,
    }];
    let (scene, cache, _sources) = collect_scene(&panes, &[], CELL, delivered);
    (scene, cache)
}

#[test]
fn an_uploaded_image_becomes_one_asset_and_one_placement_at_its_pane_origin() {
    crate::kitty_graphics::set_enabled(true);
    let pane = pane(20, 5);
    pane.process_pty_bytes(PaneId::from_raw(1), UPLOAD);

    let (scene, cache) = collect_for(&pane, Rect::new(3, 2, 20, 5), &DeliveryCache::default());
    assert_eq!(scene.assets.len(), 1, "{scene:?}");
    assert_eq!(
        scene.assets[0].data.len() as u64,
        scene.assets[0].key.data_len
    );
    assert_eq!(scene.placements.len(), 1, "{scene:?}");
    let placement = &scene.placements[0];
    assert_eq!(placement.asset, scene.assets[0].key);
    assert_eq!(
        (placement.x, placement.y),
        (3, 2),
        "pane origin + viewport cell"
    );
    assert_eq!((placement.cols, placement.rows), (1, 1));
    assert_eq!(placement.logical_placement_id, 1);
    assert!(cache.assets.contains(&placement.asset));

    // The same scene again delivers geometry only, never the payload twice.
    let (again, cache) = collect_for(&pane, Rect::new(3, 2, 20, 5), &cache);
    assert!(again.assets.is_empty(), "{again:?}");
    assert_eq!(again.placements, scene.placements);
    assert!(!cache.has_pending());
}

#[test]
fn a_pane_leaving_the_surface_retains_its_delivered_image() {
    crate::kitty_graphics::set_enabled(true);
    let pane = pane(20, 5);
    pane.process_pty_bytes(PaneId::from_raw(1), UPLOAD);
    let (_, cache) = collect_for(&pane, Rect::new(0, 0, 20, 5), &DeliveryCache::default());

    // Hidden: not composed, but still alive, so its image stays retained.
    let retained = [ScenePane {
        id: PaneId::from_raw(1),
        public_id: "t1".into(),
        inner_rect: Rect::default(),
        terminal: &pane,
    }];
    let (hidden, _, _) = collect_scene(&[], &retained, CELL, &cache);
    assert!(hidden.placements.is_empty());
    assert_eq!(hidden.retained_assets.len(), 1, "{hidden:?}");

    // A deleted image cannot be retained.
    pane.process_pty_bytes(PaneId::from_raw(1), b"\x1b_Ga=d,d=I,i=7,q=2\x1b\\");
    let (gone, _, _) = collect_scene(&[], &retained, CELL, &cache);
    assert!(gone.retained_assets.is_empty(), "{gone:?}");
}

#[test]
fn unknown_cell_size_collects_nothing() {
    crate::kitty_graphics::set_enabled(true);
    let pane = pane(20, 5);
    pane.process_pty_bytes(PaneId::from_raw(1), UPLOAD);
    let panes = [ScenePane {
        id: PaneId::from_raw(1),
        public_id: "t1".into(),
        inner_rect: Rect::new(0, 0, 20, 5),
        terminal: &pane,
    }];
    let (scene, cache, _) = collect_scene(
        &panes,
        &[],
        HostCellSize::default(),
        &DeliveryCache::default(),
    );
    assert_eq!(scene, SurfaceGraphicsScene::default());
    assert!(cache.assets.is_empty());
}
