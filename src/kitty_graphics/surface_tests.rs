//! Surface graphics client, occlusion, and scaling tests.

use super::*;

impl Occlusion {
    fn covers(&self, placement: &SurfaceGraphicsPlacement, origin: (u16, u16)) -> bool {
        self.visible_pieces(placement, origin, 1) != Some(vec![GridPiece::whole(placement)])
    }
}

impl ClientState {
    pub(crate) fn encode(
        &mut self,
        visibility: Visibility,
        main_origin: (u16, u16),
        cell_size: HostCellSize,
        occlusion: &Occlusion,
    ) -> Vec<u8> {
        self.encode_output(visibility, main_origin, cell_size, occlusion)
            .into_inline_bytes()
    }
}

fn asset(target: SurfaceGraphicsTarget, fingerprint: u64, data: Vec<u8>) -> SurfaceGraphicsAsset {
    SurfaceGraphicsAsset {
        key: SurfaceGraphicsAssetKey {
            source: SurfaceGraphicsSource::Terminal {
                target,
                image_id: 7,
            },
            image_width: 1,
            image_height: 1,
            format: SurfaceGraphicsFormat::Rgba,
            data_len: data.len() as u64,
            data_fingerprint: fingerprint,
        },
        data,
    }
}

fn scene(asset: SurfaceGraphicsAsset, x: u16, y: u16) -> SurfaceGraphicsScene {
    SurfaceGraphicsScene {
        placements: vec![SurfaceGraphicsPlacement {
            asset: asset.key.clone(),
            logical_placement_id: 3,
            x,
            y,
            cols: 1,
            rows: 1,
            source_x: 0,
            source_y: 0,
            source_width: 1,
            source_height: 1,
            x_offset: 0,
            y_offset: 0,
            z: 0,
            scrollback_offset: 0,
        }],
        assets: vec![asset],
        retained_assets: Vec::new(),
    }
}

#[test]
fn legacy_pane_layer_scene_still_uploads_and_places() {
    let mut state = ClientState::default();
    state.set_scope("old-server-pane-layer");
    let mut image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "pane".into(),
        },
        1,
        vec![1, 2, 3, 255],
    );
    image.key.source = SurfaceGraphicsSource::PaneLayer {
        pane_id: "pane".into(),
        layer_id: "primary".into(),
    };
    state.set_scene(scene(image, 2, 1));

    let bytes = String::from_utf8(state.encode(
        Visibility::Main,
        (10, 5),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    ))
    .unwrap();

    assert!(bytes.contains("a=t,t=d"), "{bytes:?}");
    assert!(bytes.contains("a=p,"), "{bytes:?}");
    assert!(bytes.contains("\u{1b}[7;13H"), "{bytes:?}");
}

#[test]
fn stable_ids_replace_native_pixels_and_delete_on_removal() {
    use crate::kitty_graphics::GraphicsOperation;
    let mut state = ClientState::default();
    state.set_scope("stable-native");
    let target = SurfaceGraphicsTarget::Pane {
        pane_id: "pane".into(),
    };
    let first = asset(target.clone(), 1, vec![1, 2, 3, 255]);
    let id = state.image_id(&first.key);
    let cell = HostCellSize {
        width_px: 8,
        height_px: 16,
    };
    for revision in 1..=8 {
        let next = asset(target.clone(), revision, vec![revision as u8, 2, 3, 255]);
        assert_eq!(state.image_id(&next.key), id);
        state.set_scene(scene(next, 0, 0));
        let output = state.encode_output(Visibility::Main, (0, 0), cell, &Occlusion::default());
        let uploads = output
            .operations
            .iter()
            .filter_map(|op| match op {
                GraphicsOperation::Upload { control, data } => Some((control, data)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(uploads.len(), 1);
        assert!(uploads[0].0.contains(&format!("i={id},")));
        assert_eq!(uploads[0].1.as_ref(), &[revision as u8, 2, 3, 255]);
        let bytes = String::from_utf8(output.into_inline_bytes()).unwrap();
        let upload = bytes.find("a=t,").unwrap();
        let display = bytes.find("a=p,").unwrap();
        assert!(upload < display);
        if revision > 1 {
            assert!(bytes.find("a=d,d=I,").unwrap() < upload);
        }
        assert_eq!(state.host.images.len(), 1);
    }
    let mut other = first.key.clone();
    other.source = SurfaceGraphicsSource::Terminal {
        target: SurfaceGraphicsTarget::Pane {
            pane_id: "other-pane".into(),
        },
        image_id: 7,
    };
    assert_ne!(state.image_id(&other), id);
    other.source = SurfaceGraphicsSource::PaneLayer {
        pane_id: "pane".into(),
        layer_id: "layer".into(),
    };
    assert_eq!(state.image_id(&other), host_image_id(state.scope(), &other));
    let legacy_id = state.image_id(&other);
    other.data_fingerprint += 1;
    assert_ne!(state.image_id(&other), legacy_id);
    state.set_scene(SurfaceGraphicsScene::default());
    let bytes = state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default());
    assert!(String::from_utf8(bytes)
        .unwrap()
        .contains(&format!("a=d,d=I,i={id}")));
    assert!(state.host.images.is_empty());
}

#[test]
fn native_file_adoption_keeps_previous_frame_until_replacement_ack() {
    let mut state = ClientState::default();
    state.set_scope("native-files");
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "p1".into(),
        },
        1,
        vec![1, 2, 3, 255],
    );
    let id = native_host_image_id(state.scope(), &image.key);
    assert!(!state.accepts_direct_asset(&image.key, id));
    let mut metadata = scene(image.clone(), 0, 0);
    metadata.assets.clear();
    state.set_scene(metadata);
    let cell = HostCellSize {
        width_px: 8,
        height_px: 16,
    };
    state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default());
    assert!(state.trust_direct_asset(&image.key, id));
    let first =
        String::from_utf8(state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default()))
            .unwrap();
    assert!(first.contains("a=p,"));
    assert!(!first.contains("a=t,"));
    let mut next = image.clone();
    next.key.data_fingerprint += 1;
    let mut metadata = scene(next.clone(), 0, 0);
    metadata.assets.clear();
    state.set_scene(metadata);
    assert!(!state.trust_direct_asset(&image.key, id));
    let cleanup =
        String::from_utf8(state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default()))
            .unwrap();
    assert!(!cleanup.contains("a=d,"));
    assert!(cleanup.contains(&format!("a=p,i={id},")));
    assert!(!state.accepts_direct_asset(&next.key, id));
    let replacement_id = id ^ NATIVE_SLOT_BIT;
    assert!(state.trust_direct_asset(&next.key, replacement_id));
    let placed =
        String::from_utf8(state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default()))
            .unwrap();
    assert!(placed.contains(&format!("a=p,i={replacement_id},")));
    assert!(placed.contains(&format!("a=d,d=I,i={id},")));
    assert!(!placed.contains("a=t,"));
    assert_eq!(state.host.images.len(), 1);
    assert_eq!(state.native_image_ids.len(), 1);
    // The vacated bank is reusable, but the visible bank cannot be overwritten.
    assert!(state.accepts_direct_asset(&next.key, id));
    assert!(!state.accepts_direct_asset(&next.key, replacement_id));
    assert!(!state.accepts_direct_asset(&next.key, host_image_id(state.scope(), &next.key)));
}

#[test]
fn native_replacements_use_two_ids_without_blank_pending_frames() {
    let mut state = ClientState::default();
    state.set_scope("double-buffer");
    let cell = HostCellSize {
        width_px: 8,
        height_px: 16,
    };
    let mut previous = None;
    let mut ids = HashSet::new();
    for revision in 0..32 {
        let image = asset(
            SurfaceGraphicsTarget::Pane {
                pane_id: "pane".into(),
            },
            revision,
            vec![revision as u8, 2, 3, 255],
        );
        let base = native_host_image_id(state.scope(), &image.key);
        let id = if revision % 2 == 0 {
            base ^ NATIVE_SLOT_BIT
        } else {
            base
        };
        ids.insert(id);
        let mut metadata = scene(image.clone(), 0, 0);
        metadata.assets.clear();
        state.set_scene(metadata.clone());
        for _ in 0..3 {
            state.set_scene(metadata.clone());
            let pending = String::from_utf8(state.encode(
                Visibility::Main,
                (0, 0),
                cell,
                &Occlusion::default(),
            ))
            .unwrap();
            assert!(!pending.contains("a=d,"), "{pending:?}");
            if let Some(previous) = previous {
                assert!(pending.contains(&format!("a=p,i={previous},")));
                assert!(!state.accepts_direct_asset(&image.key, previous));
            }
        }
        assert!(state.trust_direct_asset(&image.key, id));
        let swapped =
            String::from_utf8(state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default()))
                .unwrap();
        assert!(swapped.contains(&format!("a=p,i={id},")));
        assert!(!swapped.contains(&format!("a=d,d=I,i={id},")));
        if let Some(previous) = previous {
            assert!(swapped.contains(&format!("a=d,d=I,i={previous},")));
        }
        assert_eq!(state.host.images.len(), 1);
        assert_eq!(state.native_image_ids.len(), 1);
        assert!(!state.accepts_direct_asset(&image.key, id));
        assert_eq!(id & 0x8000_0000, 0, "native IDs must not alias API IDs");
        previous = Some(id);
    }
    assert_eq!(ids.len(), 2);
    state.set_scene(SurfaceGraphicsScene::default());
    let cleanup =
        String::from_utf8(state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default()))
            .unwrap();
    assert!(cleanup.contains(&format!("a=d,d=I,i={},", previous.unwrap())));
    assert!(state.host.images.is_empty());
    assert!(state.native_image_ids.is_empty());
}

#[test]
fn native_pending_replacement_failure_and_geometry_do_not_preserve_stale_frames() {
    let mut state = ClientState::default();
    state.set_scope("replacement-fallback");
    let cell = HostCellSize {
        width_px: 8,
        height_px: 16,
    };
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "pane".into(),
        },
        1,
        vec![1, 2, 3, 255],
    );
    let base = native_host_image_id(state.scope(), &image.key);
    let active = base ^ NATIVE_SLOT_BIT;
    let mut metadata = scene(image.clone(), 0, 0);
    metadata.assets.clear();
    state.set_scene(metadata);
    state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default());
    assert!(state.trust_direct_asset(&image.key, active));
    state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default());
    let mut next = image.clone();
    next.key.data_fingerprint += 1;
    let mut metadata = scene(next.clone(), 0, 0);
    metadata.assets.clear();
    state.set_scene(metadata);
    state.retire_direct_image(base);
    let rejected =
        String::from_utf8(state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default()))
            .unwrap();
    assert!(!rejected.contains(&format!("a=d,d=I,i={active},")));
    assert!(rejected.contains(&format!("a=p,i={active},")));
    state.set_scene(scene(next.clone(), 0, 0));
    let fallback =
        String::from_utf8(state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default()))
            .unwrap();
    assert!(fallback.contains("a=t,"));
    assert!(fallback.contains(&format!("a=p,i={base},")));
    assert!(fallback.contains(&format!("a=d,d=I,i={active},")));
    assert!(!state.accepts_direct_asset(&next.key, base));
    next.key.data_fingerprint += 1;
    assert!(!state.accepts_direct_asset(&next.key, base));
    let mut moved = scene(next.clone(), 1, 0);
    moved.assets.clear();
    state.set_scene(moved);
    let geometry =
        String::from_utf8(state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default()))
            .unwrap();
    assert!(geometry.contains(&format!("a=d,d=I,i={base},")));
    assert!(!geometry.contains("a=p,"));
    state.set_scene(SurfaceGraphicsScene::default());
    assert!(!state.trust_direct_asset(&next.key, active));
}

#[test]
fn deferred_client_reuses_resident_arc_and_orders_cleanup_before_upload() {
    use super::super::output::GraphicsOperation;
    let mut state = ClientState::default();
    state.set_scope("raw");
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "pane".into(),
        },
        1,
        vec![1, 2, 3, 4],
    );
    let key = image.key.clone();
    let mut graphics = scene(image, 0, 0);
    let mut second = graphics.placements[0].clone();
    second.logical_placement_id += 1;
    second.x = 2;
    graphics.placements.push(second);
    let mut replacement = graphics.clone();
    replacement.assets.clear();
    state.set_scene(graphics);
    let resident = Arc::clone(&state.assets[&key]);
    state.set_scene(replacement);
    assert!(Arc::ptr_eq(&resident, &state.assets[&key]));
    state.forced_delete_images.push(4242);
    let cell = HostCellSize {
        width_px: 8,
        height_px: 16,
    };
    let output = state.encode_output(Visibility::Main, (0, 0), cell, &Occlusion::default());
    assert!(
        matches!(&output.operations[0], GraphicsOperation::Bytes(bytes)
        if String::from_utf8_lossy(bytes).contains("a=d,d=I,i=4242"))
    );
    let uploads = output
        .operations
        .iter()
        .filter_map(|op| match op {
            GraphicsOperation::Upload { data, .. } => Some(data),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(uploads.len(), 1);
    assert!(Arc::ptr_eq(uploads[0], &resident));
    assert_eq!(
        String::from_utf8(output.clone().into_inline_bytes())
            .unwrap()
            .matches("a=p")
            .count(),
        2
    );
    let replay = state.encode_output(Visibility::Main, (0, 0), cell, &Occlusion::default());
    assert!(replay
        .operations
        .iter()
        .all(|op| matches!(op, GraphicsOperation::Bytes(_))));
}

#[test]
fn occlusion_uses_placement_cells_and_strict_overlap() {
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "pane".into(),
        },
        1,
        vec![1, 2, 3, 4],
    );
    let mut placement = scene(image, 1, 2).placements.remove(0);
    let mut cover = Occlusion::default();
    cover.cover(Rect::new(12, 8, 1, 1));
    // The image at (11, 7) just touches the cover's top-left corner.
    assert!(!cover.covers(&placement, (10, 5)));
    // Pixel offsets shrink the image inside its cells instead of bleeding.
    placement.x_offset = 1;
    placement.y_offset = 1;
    assert!(!cover.covers(&placement, (10, 5)));
    placement.cols = 2;
    placement.rows = 2;
    assert!(cover.covers(&placement, (10, 5)));
    assert!(!cover.covers(&placement, (0, 0)));
    cover = Occlusion::default();
    cover.cover(Rect::new(11, 7, 0, 1));
    assert!(!cover.covers(&placement, (10, 5)));
}

#[test]
fn occlusion_keeps_shared_asset_placements_and_restores_without_upload() {
    for direct in [false, true] {
        let mut state = ClientState::default();
        state.set_scope("occlusion");
        let _ = state.take_pending_cleanup();
        let image = asset(
            SurfaceGraphicsTarget::Pane {
                pane_id: "pane".into(),
            },
            1,
            vec![1, 2, 3, 4],
        );
        let id = state.image_id(&image.key);
        let mut graphics = scene(image.clone(), 0, 0);
        let mut second = graphics.placements[0].clone();
        second.logical_placement_id = 4;
        second.x = 5;
        graphics.placements.push(second);
        if direct {
            // Model an already uploaded asset, including on hosts without direct transport.
            graphics.assets.clear();
            state.host.images.insert(
                id,
                ImageSignature {
                    image_width: 1,
                    image_height: 1,
                    format_code: 32,
                    data_len: 4,
                    data_fingerprint: 1,
                },
            );
        }
        state.set_scene(graphics);
        let cell = HostCellSize {
            width_px: 8,
            height_px: 16,
        };
        let _ = state.encode(Visibility::Main, (10, 5), cell, &Occlusion::default());
        assert_eq!(state.host.placements.len(), 2);
        let mut cover = Occlusion::default();
        cover.cover(Rect::new(10, 5, 1, 1));
        let hidden = state.encode(Visibility::Main, (10, 5), cell, &cover);
        let hidden = String::from_utf8_lossy(&hidden);
        assert!(hidden.contains("a=d,d=i"), "{hidden}");
        assert!(!hidden.contains("a=d,d=I"), "{hidden}");
        assert!(hidden.contains("\u{1b}[6;16H"), "{hidden}");
        assert_eq!(state.host.placements.len(), 1);
        cover.cover(Rect::new(15, 5, 1, 1));
        let _ = state.encode(Visibility::Main, (10, 5), cell, &cover);
        assert!(state.host.placements.is_empty());
        assert!(state.host.images.contains_key(&id));
        let restored = state.encode(Visibility::Main, (10, 5), cell, &Occlusion::default());
        let restored = String::from_utf8_lossy(&restored);
        assert!(restored.contains("a=p"), "{restored}");
        assert!(!restored.contains("a=t"), "{restored}");
        assert_eq!(state.host.placements.len(), 2);
    }
}

fn grid_image(cols: u32, rows: u32) -> SurfaceGraphicsScene {
    let mut image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "pane".into(),
        },
        1,
        vec![0; (cols * 10 * rows * 10 * 4) as usize],
    );
    image.key.image_width = cols * 10;
    image.key.image_height = rows * 10;
    let mut graphics = scene(image, 0, 0);
    let placement = &mut graphics.placements[0];
    placement.cols = cols;
    placement.rows = rows;
    placement.source_width = cols * 10;
    placement.source_height = rows * 10;
    graphics
}

fn placed_pieces(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut pieces = text
        .split("\u{1b}[")
        .filter(|chunk| chunk.contains("a=p,"))
        .map(|chunk| {
            let (cursor, rest) = chunk.split_once('H').unwrap();
            let control = rest
                .trim_start_matches("\u{1b}_G")
                .split(';')
                .next()
                .unwrap()
                .split(',')
                .filter(|field| {
                    ["c=", "r=", "x=", "y=", "w=", "h=", "X=", "Y="]
                        .iter()
                        .any(|key| field.starts_with(key))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{cursor} {control}")
        })
        .collect::<Vec<_>>();
    pieces.sort();
    pieces
}

fn encode_with_cover(
    graphics: SurfaceGraphicsScene,
    covers: &[Rect],
) -> (ClientState, Vec<String>) {
    let mut state = ClientState::default();
    state.set_scope("crop");
    let _ = state.take_pending_cleanup();
    state.set_scene(graphics);
    let mut occlusion = Occlusion::default();
    for rect in covers {
        occlusion.cover(*rect);
    }
    let bytes = state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &occlusion,
    );
    (state, placed_pieces(&bytes))
}

#[test]
fn overlay_on_image_corner_keeps_the_rest_visible() {
    // A 10x4-cell image with 10px source pixels per cell; a toast covers
    // the top-right 4x2 cells.
    let (_, pieces) = encode_with_cover(grid_image(10, 4), &[Rect::new(6, 0, 4, 2)]);
    assert_eq!(
        pieces,
        vec![
            "1;1 c=6,r=2,w=60,h=20".to_owned(),
            "3;1 c=10,r=2,y=20,w=100,h=20".to_owned(),
        ]
    );
}

#[test]
fn overlay_inside_image_keeps_four_surrounding_pieces() {
    let (_, pieces) = encode_with_cover(grid_image(10, 6), &[Rect::new(3, 2, 4, 2)]);
    assert_eq!(
        pieces,
        vec![
            "1;1 c=10,r=2,w=100,h=20".to_owned(),
            "3;1 c=3,r=2,y=20,w=30,h=20".to_owned(),
            "3;8 c=3,r=2,x=70,y=20,w=30,h=20".to_owned(),
            "5;1 c=10,r=2,y=40,w=100,h=20".to_owned(),
        ]
    );
}

#[test]
fn overlay_across_image_edge_crops_only_the_covered_side() {
    let (_, pieces) = encode_with_cover(grid_image(10, 4), &[Rect::new(7, 0, 20, 20)]);
    assert_eq!(pieces, vec!["1;1 c=7,r=4,w=70,h=40".to_owned()]);
}

#[test]
fn multiple_overlays_crop_around_each_other() {
    let (_, pieces) = encode_with_cover(
        grid_image(10, 4),
        &[Rect::new(0, 0, 2, 1), Rect::new(8, 3, 2, 1)],
    );
    assert_eq!(
        pieces,
        vec![
            "1;3 c=8,r=1,x=20,w=80,h=10".to_owned(),
            "2;1 c=10,r=2,y=10,w=100,h=20".to_owned(),
            "4;1 c=8,r=1,y=30,w=80,h=10".to_owned(),
        ]
    );
}

#[test]
fn pixel_offsets_stay_on_the_leading_pieces_without_seams() {
    let mut graphics = grid_image(10, 4);
    graphics.placements[0].x_offset = 4;
    graphics.placements[0].y_offset = 8;
    // Offsets shrink the image inside its cells: 100x40 source pixels span
    // 76x56 display pixels starting at (4, 8).
    let (_, pieces) = encode_with_cover(graphics.clone(), &[Rect::new(1, 0, 1, 4)]);
    assert_eq!(
        pieces,
        vec![
            "1;1 c=1,r=4,w=5,h=40,X=4,Y=8".to_owned(),
            "1;3 c=8,r=4,x=15,w=85,h=40,Y=8".to_owned(),
        ]
    );
    let (_, pieces) = encode_with_cover(graphics, &[Rect::new(0, 1, 10, 1)]);
    assert_eq!(
        pieces,
        vec![
            "1;1 c=10,r=1,w=100,h=5,X=4,Y=8".to_owned(),
            "3;1 c=10,r=2,y=17,w=100,h=23,X=4".to_owned(),
        ]
    );
}

#[test]
fn upscaled_slivers_never_fall_back_to_the_whole_source() {
    let mut graphics = grid_image(10, 1);
    let placement = &mut graphics.placements[0];
    placement.source_x = 30;
    placement.source_width = 2;
    let (_, pieces) = encode_with_cover(graphics, &[Rect::new(1, 0, 9, 1)]);
    assert_eq!(pieces, vec!["1;1 c=1,r=1,x=30,w=1,h=10".to_owned()]);
}

#[test]
fn uneven_source_scaling_leaves_no_seam_between_pieces() {
    let mut graphics = grid_image(3, 1);
    graphics.placements[0].source_width = 10;
    let (_, pieces) = encode_with_cover(graphics, &[Rect::new(1, 0, 1, 1)]);
    assert_eq!(
        pieces,
        vec![
            "1;1 c=1,r=1,w=3,h=10".to_owned(),
            "1;3 c=1,r=1,x=6,w=4,h=10".to_owned(),
        ]
    );
}

#[test]
fn over_budget_frames_hide_touched_images_whole() {
    // A 40x4 image with holes in every other cell of two rows needs 42
    // pieces, so the frame falls back to hiding every touched image.
    let mut graphics = grid_image(40, 4);
    let fragmented = graphics.placements[0].clone();
    let mut cropped = fragmented.clone();
    cropped.logical_placement_id = 4;
    cropped.y = 10;
    cropped.cols = 10;
    let mut untouched = cropped.clone();
    untouched.logical_placement_id = 5;
    untouched.x = 20;
    graphics.placements.extend([cropped, untouched]);
    let mut covers = (0..40)
        .map(|index| Rect::new(index % 20 * 2, index / 20 * 2 + 1, 1, 1))
        .collect::<Vec<_>>();
    covers.push(Rect::new(6, 10, 4, 2));
    let (state, pieces) = encode_with_cover(graphics.clone(), &covers);
    assert_eq!(pieces, vec!["11;21 c=10,r=4,w=400,h=40".to_owned()]);
    assert_eq!(state.host.placements.len(), 1);

    // Without the fragmenting holes the same frame crops normally.
    let (_, pieces) = encode_with_cover(graphics, &covers[40..]);
    assert_eq!(pieces.len(), 4, "{pieces:?}");
}

#[test]
fn moving_overlay_replaces_and_retires_pieces() {
    let mut state = ClientState::default();
    state.set_scope("crop");
    let _ = state.take_pending_cleanup();
    state.set_scene(grid_image(10, 6));
    let cell = HostCellSize {
        width_px: 8,
        height_px: 16,
    };
    let mut cover = Occlusion::default();
    cover.cover(Rect::new(3, 2, 4, 2));
    let _ = state.encode(Visibility::Main, (0, 0), cell, &cover);
    assert_eq!(state.host.placements.len(), 4);
    let mut cover = Occlusion::default();
    cover.cover(Rect::new(6, 0, 4, 2));
    let moved = state.encode(Visibility::Main, (0, 0), cell, &cover);
    assert!(!String::from_utf8_lossy(&moved).contains("a=t"));
    assert_eq!(state.host.placements.len(), 2);
    let restored = state.encode(Visibility::Main, (0, 0), cell, &Occlusion::default());
    assert_eq!(
        placed_pieces(&restored),
        vec!["1;1 c=10,r=6,w=100,h=60".to_owned()]
    );
    assert_eq!(state.host.placements.len(), 1);
}

#[test]
#[ignore = "manual image placement overlap scaling profile"]
fn graphics_occlusion_render_scale_profile() {
    let cell = HostCellSize {
        width_px: 8,
        height_px: 16,
    };
    for count in [1, 15] {
        let mut graphics = SurfaceGraphicsScene::default();
        for index in 0..count {
            let image = asset(
                SurfaceGraphicsTarget::Pane {
                    pane_id: format!("pane-{index}"),
                },
                index as u64,
                vec![1, 2, 3, 4],
            );
            let pane = scene(image, index * 4, 0);
            graphics.assets.extend(pane.assets);
            graphics.placements.extend(pane.placements);
        }
        for overlays in ["none", "disjoint", "toast_and_menu", "fragmenting"] {
            let mut state = ClientState::default();
            state.set_scope("profile");
            let mut cover = Occlusion::default();
            let mut graphics = graphics.clone();
            match overlays {
                "disjoint" => {
                    cover.cover(Rect::new(0, 10, 80, 3));
                    cover.cover(Rect::new(60, 0, 20, 5));
                }
                "toast_and_menu" | "fragmenting" => {
                    for (index, placement) in graphics.placements.iter_mut().enumerate() {
                        let x = index as u16 * 16;
                        placement.x = x;
                        placement.cols = 16;
                        placement.rows = 8;
                        if overlays == "fragmenting" {
                            // Six holes per 16x8 image leave 11 visible pieces each.
                            for (col, row) in [(3, 2), (8, 2), (13, 2), (3, 5), (8, 5), (13, 5)] {
                                cover.cover(Rect::new(x + col, row, 2, 1));
                            }
                        }
                    }
                    if overlays == "toast_and_menu" {
                        let width = count * 16;
                        cover.cover(Rect::new(width.saturating_sub(12), 0, 12, 3));
                        cover.cover(Rect::new(width / 2, 2, 10, 4));
                    }
                }
                _ => {}
            }
            state.set_scene(graphics);
            let _ = state.encode_output(Visibility::Main, (0, 0), cell, &cover);
            let mut samples = Vec::new();
            for _ in 0..101 {
                let start = std::time::Instant::now();
                for _ in 0..20 {
                    std::hint::black_box(state.encode_output(
                        Visibility::Main,
                        (0, 0),
                        cell,
                        &cover,
                    ));
                }
                samples.push(start.elapsed().as_nanos() / 20);
            }
            samples.sort_unstable();
            eprintln!(
                "graphics occlusion panes={count} overlays={overlays} median_ns={} p95_ns={}",
                samples[50], samples[95]
            );
        }
        {
            let mut state = ClientState::default();
            state.set_scope("swap-profile");
            state.set_scene(graphics.clone());
            state.encode_output(Visibility::Main, (0, 0), cell, &Occlusion::default());
            graphics.assets.clear();
            let mut samples = Vec::new();
            for revision in 1..=105 {
                graphics.placements[0].asset.data_fingerprint = revision;
                let key = graphics.placements[0].asset.clone();
                let base = native_host_image_id(state.scope(), &key);
                let id = if revision % 2 == 1 {
                    base ^ NATIVE_SLOT_BIT
                } else {
                    base
                };
                let scene = graphics.clone();
                let start = std::time::Instant::now();
                state.set_scene(scene);
                std::hint::black_box(state.encode_output(
                    Visibility::Main,
                    (0, 0),
                    cell,
                    &Occlusion::default(),
                ));
                let checkpoint = state.clone();
                assert!(state.trust_direct_asset(&key, id));
                std::hint::black_box(state.encode_output(
                    Visibility::Main,
                    (0, 0),
                    cell,
                    &Occlusion::default(),
                ));
                drop(checkpoint);
                if revision > 5 {
                    samples.push(start.elapsed().as_nanos());
                }
            }
            samples.sort_unstable();
            eprintln!(
                "graphics replacement panes={count} median_ns={} p95_ns={}",
                samples[50], samples[95]
            );
        }
    }
    // Unicode placeholder images (e.g. a terminal browser) arrive as one
    // placement per row, so overlays split only the rows they cross.
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "browser".into(),
        },
        1,
        vec![1, 2, 3, 4],
    );
    let mut browser = scene(image, 0, 0);
    let row = browser.placements.remove(0);
    browser.placements = (0..45)
        .map(|index| {
            let mut row = row.clone();
            row.logical_placement_id = index + 1;
            row.y = index as u16;
            row.cols = 150;
            row
        })
        .collect();
    for overlays in [false, true] {
        let mut state = ClientState::default();
        state.set_scope("browser-profile");
        state.set_scene(browser.clone());
        let mut cover = Occlusion::default();
        if overlays {
            cover.cover(Rect::new(110, 0, 40, 4));
            cover.cover(Rect::new(60, 20, 30, 8));
        }
        let _ = state.encode_output(Visibility::Main, (0, 0), cell, &cover);
        let mut samples = Vec::new();
        for _ in 0..101 {
            let start = std::time::Instant::now();
            std::hint::black_box(state.encode_output(Visibility::Main, (0, 0), cell, &cover));
            samples.push(start.elapsed().as_nanos());
        }
        samples.sort_unstable();
        eprintln!(
            "graphics placeholder rows=45 toast_and_menu={overlays} median_ns={} p95_ns={}",
            samples[50], samples[95]
        );
    }
}

#[test]
fn client_encodes_final_main_origin_upload_once_and_replays_placement() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        11,
        vec![1, 2, 3, 4],
    );
    state.set_scene(scene(image, 1, 2));

    let first = state.encode(
        Visibility::Main,
        (10, 5),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    );
    assert!(String::from_utf8_lossy(&first).contains("a=t,t=d"));
    assert!(String::from_utf8_lossy(&first).contains("\u{1b}[8;12H"));

    let second = state.encode(
        Visibility::Main,
        (10, 5),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    );
    let second = String::from_utf8_lossy(&second);
    assert!(!second.contains("a=t,t=d"));
    assert!(second.contains("a=p"));
    assert!(second.contains("\u{1b}[8;12H"));
}

#[test]
fn client_hides_and_restores_without_reuploading_pixels() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        12,
        vec![4, 3, 2, 1],
    );
    state.set_scene(scene(image, 0, 0));
    let cell = HostCellSize {
        width_px: 8,
        height_px: 16,
    };
    let _ = state.encode(Visibility::Main, (4, 2), cell, &Occlusion::default());

    let hidden = state.encode(Visibility::Hidden, (4, 2), cell, &Occlusion::default());
    assert!(String::from_utf8_lossy(&hidden).contains("a=d,d=i"));

    let restored = state.encode(Visibility::Main, (4, 2), cell, &Occlusion::default());
    let restored = String::from_utf8_lossy(&restored);
    assert!(restored.contains("a=p"));
    assert!(!restored.contains("a=t,t=d"));
}

#[test]
fn legacy_direct_asset_is_placed_without_inline_reupload() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let _ = state.encode(
        Visibility::Hidden,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    );
    let mut image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        15,
        vec![1, 2, 3, 4],
    );
    image.key.source = SurfaceGraphicsSource::PaneLayer {
        pane_id: "w1:p1".into(),
        layer_id: "primary".into(),
    };
    let image_id = state.image_id(&image.key);
    assert!(state.trust_direct_asset(&image.key, image_id));
    let mut direct_scene = scene(image, 0, 0);
    direct_scene.assets.clear();
    state.set_scene(direct_scene);

    let bytes = state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    );
    let bytes = String::from_utf8_lossy(&bytes);
    assert!(bytes.contains("a=p"));
    assert!(!bytes.contains("a=t,t=d"));
}

#[test]
fn direct_asset_trusted_after_scene_arrival_is_immediately_placeable() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let _ = state.take_pending_cleanup();
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        18,
        vec![1, 2, 3, 4],
    );
    let image_id = state.image_id(&image.key);
    let mut direct_scene = scene(image.clone(), 0, 0);
    direct_scene.assets.clear();
    state.set_scene(direct_scene);
    assert!(state.trust_direct_asset(&image.key, image_id));

    let bytes = state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    );
    let bytes = String::from_utf8_lossy(&bytes);
    assert!(bytes.contains("a=p"), "{bytes}");
    assert!(!bytes.contains("a=t,t=d"), "{bytes}");
}

#[test]
fn retained_direct_asset_survives_hidden_scene_and_replays_without_upload() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let _ = state.take_pending_cleanup();
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        19,
        vec![1, 2, 3, 4],
    );
    let image_id = state.image_id(&image.key);
    let mut active = scene(image.clone(), 0, 0);
    active.assets.clear();
    state.set_scene(active.clone());
    assert!(state.trust_direct_asset(&image.key, image_id));
    let _ = state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    );

    state.set_scene(SurfaceGraphicsScene {
        retained_assets: vec![image.key.clone()],
        ..SurfaceGraphicsScene::default()
    });
    let hidden = String::from_utf8(state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    ))
    .unwrap();
    assert!(!hidden.contains(&format!("a=d,d=I,i={image_id}")));

    active.retained_assets.push(image.key.clone());
    state.set_scene(active);
    let restored = String::from_utf8(state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    ))
    .unwrap();
    assert!(restored.contains("a=p"), "{restored}");
    assert!(!restored.contains("a=t,t=d"), "{restored}");

    state.set_scene(SurfaceGraphicsScene::default());
    let removed = String::from_utf8(state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    ))
    .unwrap();
    assert!(removed.contains(&format!("a=d,d=I,i={image_id}")));
}

#[test]
fn retired_pending_direct_asset_is_deleted_even_before_cache_adoption() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let _ = state.take_pending_cleanup();
    state.retire_direct_image(4242);
    let cleanup = String::from_utf8(state.take_pending_cleanup()).unwrap();
    assert!(cleanup.contains("a=d,d=I,i=4242"), "{cleanup}");
}

#[test]
fn unclaimed_legacy_direct_asset_is_deleted_by_the_next_authoritative_scene() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let _ = state.take_pending_cleanup();
    let mut image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        16,
        vec![1, 2, 3, 4],
    );
    image.key.source = SurfaceGraphicsSource::PaneLayer {
        pane_id: "w1:p1".into(),
        layer_id: "primary".into(),
    };
    let image_id = state.image_id(&image.key);
    assert!(state.trust_direct_asset(&image.key, image_id));
    state.set_scene(SurfaceGraphicsScene::default());

    let bytes = state.encode(
        Visibility::Hidden,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    );
    let bytes = String::from_utf8_lossy(&bytes);
    assert!(bytes.contains(&format!("a=d,d=I,i={image_id}")), "{bytes}");
}

#[test]
fn boot_scope_cleanup_does_not_wait_for_a_coherent_surface() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        17,
        vec![1, 2, 3, 4],
    );
    state.set_scene(scene(image, 0, 0));
    let _ = state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    );

    state.set_scope("endpoint-a:boot-2");
    let cleanup = String::from_utf8(state.take_pending_cleanup()).unwrap();
    assert!(cleanup.contains("a=d,d=I"), "{cleanup}");
    assert!(state.take_pending_cleanup().is_empty());
}

#[test]
fn replacement_scene_without_repeated_asset_bytes_keeps_resident_data() {
    let mut state = ClientState::default();
    state.set_scope("endpoint-a:boot-1");
    let image = asset(
        SurfaceGraphicsTarget::Pane {
            pane_id: "w1:p1".into(),
        },
        14,
        vec![1, 1, 1, 1],
    );
    let first_scene = scene(image, 0, 0);
    let mut replacement = first_scene.clone();
    replacement.assets.clear();
    state.set_scene(first_scene);
    state.set_scene(replacement);

    let bytes = state.encode(
        Visibility::Main,
        (0, 0),
        HostCellSize {
            width_px: 8,
            height_px: 16,
        },
        &Occlusion::default(),
    );
    assert!(String::from_utf8_lossy(&bytes).contains("a=t,t=d"));
}
