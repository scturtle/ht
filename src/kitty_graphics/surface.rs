use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::output::GraphicsOutput;

use ratatui::layout::Rect;

use super::{
    clipped_placement, encode_graphics_output, HostCellSize, HostGraphicsCache, HostPlacement,
    HostSourceKey, ImageSignature,
};
use crate::ghostty::{KittyImageFormat, KittyImagePlacement, KittyPlacementRenderInfo};
use crate::layout::PaneId;
use crate::protocol::{
    SurfaceGraphicsAsset, SurfaceGraphicsAssetKey, SurfaceGraphicsFormat, SurfaceGraphicsPlacement,
    SurfaceGraphicsScene, SurfaceGraphicsSource, SurfaceGraphicsTarget,
};

pub(crate) const MAX_SURFACE_GRAPHICS_PLACEMENTS: usize = 4_096;
const MAX_OFFSCREEN_IMAGES: usize = 16;
const MAX_OFFSCREEN_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub(crate) struct DeliveryCache {
    assets: HashSet<SurfaceGraphicsAssetKey>,
    /// Delivered terminal images without a placement, most recently visible first.
    offscreen: Vec<SurfaceGraphicsAssetKey>,
    pending: bool,
}

pub(crate) type SourceFiles =
    HashMap<SurfaceGraphicsAssetKey, Arc<crate::pane_graphics_files::OwnedExport>>;

impl DeliveryCache {
    pub(crate) fn contains(&self, key: &SurfaceGraphicsAssetKey) -> bool {
        self.assets.contains(key)
    }

    /// A synchronized pane still displays these already-delivered assets even
    /// if its live parser has removed/replaced them inside an unfinished update.
    pub(crate) fn retain_committed(&mut self, key: &SurfaceGraphicsAssetKey) {
        self.assets.insert(key.clone());
    }

    /// Assets still waiting for a later scene: the byte budget bounded this one.
    pub(crate) fn has_pending(&self) -> bool {
        self.pending
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Visibility {
    Main,
    Hidden,
}

#[derive(Debug, Default)]
pub(crate) struct Occlusion {
    regions: Vec<Rect>,
}

impl Occlusion {
    pub(crate) fn cover(&mut self, rect: Rect) {
        if !rect.is_empty() {
            self.regions.push(rect);
        }
    }

    /// Visible pieces for each placement in a frame. When cropping would exceed
    /// the frame's piece budget, touched images are hidden whole instead.
    fn frame_pieces(
        &self,
        placements: &[(&SurfaceGraphicsPlacement, (u16, u16))],
    ) -> Vec<Vec<GridPiece>> {
        let mut extra_budget = MAX_EXTRA_CROP_PIECES;
        let mut pieces = Vec::with_capacity(placements.len());
        for &(placement, origin) in placements {
            let Some(found) = self.visible_pieces(placement, origin, extra_budget + 1) else {
                return placements
                    .iter()
                    .map(|&(placement, origin)| {
                        let whole = vec![GridPiece::whole(placement)];
                        if self.visible_pieces(placement, origin, 1).as_ref() == Some(&whole) {
                            whole
                        } else {
                            Vec::new()
                        }
                    })
                    .collect();
            };
            extra_budget -= found.len().saturating_sub(1);
            pieces.push(found);
        }
        pieces
    }

    /// Splits a placement's cell grid into the rectangles no overlay touches,
    /// or returns `None` when that would take more than `limit` pieces.
    fn visible_pieces(
        &self,
        placement: &SurfaceGraphicsPlacement,
        origin: (u16, u16),
        limit: usize,
    ) -> Option<Vec<GridPiece>> {
        let regions = &self.regions;
        let mut pieces = vec![GridPiece::whole(placement)];
        // Pixel offsets shrink an image inside its cells, so cell overlap is exact.
        let x = u32::from(origin.0.saturating_add(placement.x));
        let y = u32::from(origin.1.saturating_add(placement.y));
        let covered_span = |start: u16, end: u16, image_start: u32, len: u32| {
            let first = u32::from(start).saturating_sub(image_start);
            let last = u32::from(end).saturating_sub(image_start).min(len);
            (first < last).then_some((first, last))
        };
        for rect in regions {
            let Some((col_start, col_end)) = covered_span(rect.x, rect.right(), x, placement.cols)
            else {
                continue;
            };
            let Some((row_start, row_end)) = covered_span(rect.y, rect.bottom(), y, placement.rows)
            else {
                continue;
            };
            pieces = pieces
                .into_iter()
                .flat_map(|piece| piece.subtract(col_start, col_end, row_start, row_end))
                .collect();
            if pieces.is_empty() {
                return Some(pieces);
            }
            if pieces.len() > limit {
                return None;
            }
        }
        Some(pieces)
    }
}

/// Extra placements one frame may add by cropping around overlays. Beyond it,
/// the frame hides touched images whole so the quadratic host encoder stays cheap.
const MAX_EXTRA_CROP_PIECES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GridPiece {
    col: u32,
    row: u32,
    cols: u32,
    rows: u32,
}

impl GridPiece {
    fn whole(placement: &SurfaceGraphicsPlacement) -> Self {
        Self {
            col: 0,
            row: 0,
            cols: placement.cols,
            rows: placement.rows,
        }
    }

    fn subtract(self, col_start: u32, col_end: u32, row_start: u32, row_end: u32) -> Vec<Self> {
        let right = self.col + self.cols;
        let bottom = self.row + self.rows;
        if col_end <= self.col || col_start >= right || row_end <= self.row || row_start >= bottom {
            return vec![self];
        }
        let middle_top = row_start.max(self.row);
        let middle_bottom = row_end.min(bottom);
        [
            (self.col, self.row, right, middle_top),
            (self.col, middle_bottom, right, bottom),
            (self.col, middle_top, col_start.max(self.col), middle_bottom),
            (col_end.min(right), middle_top, right, middle_bottom),
        ]
        .into_iter()
        .filter(|(left, top, right, bottom)| left < right && top < bottom)
        .map(|(left, top, right, bottom)| Self {
            col: left,
            row: top,
            cols: right - left,
            rows: bottom - top,
        })
        .collect()
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ClientState {
    scope: String,
    scene: SurfaceGraphicsScene,
    display_scene: SurfaceGraphicsScene,
    native_image_ids: HashMap<SurfaceGraphicsAssetKey, u32>,
    assets: HashMap<SurfaceGraphicsAssetKey, Arc<[u8]>>,
    host: HostGraphicsCache,
    trusted_direct: HashMap<SurfaceGraphicsAssetKey, u32>,
    reset_pending: bool,
    stale_images: Vec<u32>,
    forced_delete_images: Vec<u32>,
}

/// One pane the surface draws, with everything the original collector read from
/// the server's `App`: pane identity, its public (Core) address and inner rect.
pub(crate) struct ScenePane<'a> {
    pub id: PaneId,
    pub public_id: String,
    pub inner_rect: Rect,
    pub terminal: &'a crate::pane::PaneTerminal,
}

/// Restored from the original server-side collector. The reader owns the
/// authoritative Ghostty images now, so panes and the retained (hidden) pane
/// set are supplied instead of `App`/`TerminalRuntimeRegistry`; geometry,
/// delivery, offscreen retention and byte budgeting are unchanged.
pub(crate) fn collect_scene(
    surface: &[ScenePane<'_>],
    retained: &[ScenePane<'_>],
    cell_size: HostCellSize,
    delivered: &DeliveryCache,
) -> (SurfaceGraphicsScene, DeliveryCache, SourceFiles) {
    collect_scene_with_limit(
        surface,
        retained,
        cell_size,
        delivered,
        MAX_SURFACE_GRAPHICS_PLACEMENTS,
    )
}

/// Reserve placement slots for already-committed synchronized panes before
/// budgeting new assets; do not transmit/cache assets for pruned placements.
pub(crate) fn collect_scene_with_limit(
    surface: &[ScenePane<'_>],
    retained: &[ScenePane<'_>],
    cell_size: HostCellSize,
    delivered: &DeliveryCache,
    placement_limit: usize,
) -> (SurfaceGraphicsScene, DeliveryCache, SourceFiles) {
    if !cell_size.is_known() {
        return (
            SurfaceGraphicsScene::default(),
            DeliveryCache::default(),
            SourceFiles::default(),
        );
    }
    let mut targets = HashMap::new();
    let mut public_panes = HashMap::new();
    // The original mapped every pane of the workspace, not only the ones the
    // composed surface draws: offscreen retention needs the hidden ones too.
    for pane in surface.iter().chain(retained) {
        public_panes.insert(pane.public_id.clone(), pane.id);
        targets
            .entry(pane.id)
            .or_insert_with(|| SurfaceGraphicsTarget::Pane {
                pane_id: pane.public_id.clone(),
            });
    }

    // Reconstruct the small source-signature index expected by the collector.
    // This avoids copying already-delivered payloads while Ghostty remains the
    // authoritative image store.
    let mut delivered_terminal_images = HashMap::new();
    for key in &delivered.assets {
        let signature = image_signature_from_asset(key);
        match &key.source {
            SurfaceGraphicsSource::Terminal {
                target: SurfaceGraphicsTarget::Pane { pane_id },
                image_id,
            } => {
                if let Some(pane_id) = public_panes.get(pane_id) {
                    delivered_terminal_images.insert(
                        HostSourceKey::Terminal {
                            pane_id: *pane_id,
                            image_id: *image_id,
                        },
                        signature,
                    );
                }
            }
            // Frozen for old-server compatibility; current servers no longer collect layers.
            SurfaceGraphicsSource::PaneLayer { .. } => {}
        }
    }
    let host_placements = super::collect_visible_placements(
        surface
            .iter()
            .map(|pane| (pane.id, pane.inner_rect, pane.terminal)),
        cell_size,
        &delivered_terminal_images,
    );

    let mut placements = Vec::new();
    let mut asset_data = HashMap::<SurfaceGraphicsAssetKey, Vec<u8>>::new();
    let mut sources = SourceFiles::default();
    for mut placement in host_placements {
        if placements.len() == placement_limit.min(MAX_SURFACE_GRAPHICS_PLACEMENTS) {
            break;
        }
        let Some(target) = targets.get(&placement.pane_id).cloned() else {
            continue;
        };
        let source = match &placement.source_key {
            HostSourceKey::Terminal { image_id, .. } => SurfaceGraphicsSource::Terminal {
                target,
                image_id: *image_id,
            },
            HostSourceKey::ClientSurface { .. } => continue,
        };
        let Some((clipped, _)) = clipped_placement(&placement) else {
            continue;
        };
        let asset = asset_key(source, &placement.placement);
        if !delivered.assets.contains(&asset) {
            if let Some(file) = placement.placement.source_file.take() {
                sources.entry(asset.clone()).or_insert(file);
                asset_data.entry(asset.clone()).or_default();
            }
        }
        if !placement.placement.data.is_empty() {
            asset_data
                .entry(asset.clone())
                .or_insert_with(|| std::mem::take(&mut placement.placement.data));
        }
        placements.push(SurfaceGraphicsPlacement {
            asset,
            logical_placement_id: placement.placement.placement_id,
            x: clipped.x,
            y: clipped.y,
            cols: clipped.cols,
            rows: clipped.rows,
            source_x: clipped.source_x,
            source_y: clipped.source_y,
            source_width: clipped.source_width,
            source_height: clipped.source_height,
            x_offset: clipped.x_offset,
            y_offset: clipped.y_offset,
            z: placement.placement.z,
            scrollback_offset: placement.scrollback_offset,
        });
    }

    let desired = placements
        .iter()
        .map(|placement| placement.asset.clone())
        .collect::<HashSet<_>>();
    let offscreen = offscreen_assets(retained, &public_panes, delivered, &desired);
    let mut next = DeliveryCache {
        assets: delivered
            .assets
            .intersection(&desired)
            .chain(&offscreen)
            .cloned()
            .collect(),
        offscreen: offscreen.clone(),
        pending: false,
    };
    let mut assets = Vec::new();
    let mut available = asset_data.into_iter().collect::<Vec<_>>();
    available.sort_by_key(|(key, _)| format!("{:?}", key.source));
    let mut payload_bytes = 0usize;
    for (key, data) in available {
        if next.assets.contains(&key) {
            continue;
        }
        let encoded_size = super::image_transfer_estimated_size(
            usize::try_from(key.data_len).unwrap_or(usize::MAX),
        );
        if encoded_size > super::HEADLESS_GRAPHICS_TRANSACTION_BUDGET {
            continue;
        }
        if payload_bytes.saturating_add(encoded_size) > super::HEADLESS_GRAPHICS_TRANSACTION_BUDGET
        {
            next.pending = true;
            continue;
        }
        payload_bytes = payload_bytes.saturating_add(encoded_size);
        assets.push(SurfaceGraphicsAsset {
            key: key.clone(),
            data,
        });
        next.assets.insert(key);
    }
    assets.sort_by_key(|asset| format!("{:?}", asset.key.source));
    placements.sort_by_key(|placement| {
        (
            format!("{:?}", placement.asset.source),
            placement.logical_placement_id,
            placement.y,
            placement.x,
        )
    });
    sources.retain(|key, _| next.assets.contains(key));
    (
        SurfaceGraphicsScene {
            assets,
            placements,
            retained_assets: offscreen,
        },
        next,
        sources,
    )
}

/// Delivered pane images that left the view but still exist in their terminal,
/// bounded and ordered from most to least recently visible.
fn offscreen_assets(
    retained: &[ScenePane<'_>],
    public_panes: &HashMap<String, PaneId>,
    delivered: &DeliveryCache,
    visible: &HashSet<SurfaceGraphicsAssetKey>,
) -> Vec<SurfaceGraphicsAssetKey> {
    fn pane_image(key: &SurfaceGraphicsAssetKey) -> Option<(&str, u32)> {
        match &key.source {
            SurfaceGraphicsSource::Terminal {
                target: SurfaceGraphicsTarget::Pane { pane_id },
                image_id,
            } => Some((pane_id.as_str(), *image_id)),
            _ => None,
        }
    }
    let mut newly_hidden = delivered
        .assets
        .iter()
        .filter(|key| !visible.contains(*key) && !delivered.offscreen.contains(key))
        .collect::<Vec<_>>();
    if newly_hidden.is_empty() && delivered.offscreen.is_empty() {
        return Vec::new();
    }
    newly_hidden.sort_by_key(|key| pane_image(key));
    let visible_sources = visible
        .iter()
        .map(|key| &key.source)
        .collect::<HashSet<_>>();
    let mut candidates_by_pane = HashMap::<PaneId, Vec<&SurfaceGraphicsAssetKey>>::new();
    let candidates = newly_hidden
        .into_iter()
        .chain(&delivered.offscreen)
        .filter(|key| {
            !visible_sources.contains(&key.source) && key.data_len <= MAX_OFFSCREEN_IMAGE_BYTES
        })
        .filter_map(|key| {
            let (public_id, _) = pane_image(key)?;
            let pane_id = *public_panes.get(public_id)?;
            candidates_by_pane.entry(pane_id).or_default().push(key);
            Some(key)
        })
        .collect::<Vec<_>>();

    let mut live = HashSet::new();
    for (pane_id, keys) in candidates_by_pane {
        let Some(pane) = retained.iter().find(|pane| pane.id == pane_id) else {
            continue;
        };
        let image_ids = keys
            .iter()
            .filter_map(|key| pane_image(key).map(|(_, image_id)| image_id))
            .collect::<Vec<_>>();
        for (key, fingerprint) in keys
            .into_iter()
            .zip(pane.terminal.kitty_image_fingerprints(&image_ids))
        {
            if fingerprint == Some(key.data_fingerprint) {
                live.insert(key);
            }
        }
    }

    let mut bytes = 0u64;
    let mut offscreen = Vec::new();
    for key in candidates {
        if offscreen.len() == MAX_OFFSCREEN_IMAGES {
            break;
        }
        if !live.contains(key) || bytes.saturating_add(key.data_len) > MAX_OFFSCREEN_IMAGE_BYTES {
            continue;
        }
        bytes += key.data_len;
        offscreen.push(key.clone());
    }
    offscreen
}

fn asset_key(
    source: SurfaceGraphicsSource,
    placement: &KittyImagePlacement,
) -> SurfaceGraphicsAssetKey {
    SurfaceGraphicsAssetKey {
        source,
        image_width: placement.image_width,
        image_height: placement.image_height,
        format: match placement.format {
            KittyImageFormat::Rgb => SurfaceGraphicsFormat::Rgb,
            KittyImageFormat::Rgba => SurfaceGraphicsFormat::Rgba,
            KittyImageFormat::Png => SurfaceGraphicsFormat::Png,
        },
        data_len: placement.data_len as u64,
        data_fingerprint: placement.data_fingerprint,
    }
}

impl ClientState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn image_id(&self, key: &SurfaceGraphicsAssetKey) -> u32 {
        if matches!(key.source, SurfaceGraphicsSource::Terminal { .. }) {
            return self
                .native_image_ids
                .get(key)
                .copied()
                .unwrap_or_else(|| native_host_image_id(&self.scope, key));
        }
        host_image_id(&self.scope, key)
    }

    pub(crate) fn scope(&self) -> &str {
        &self.scope
    }

    pub(crate) fn set_scope(&mut self, scope: &str) {
        if self.scope == scope {
            return;
        }
        self.scope = scope.to_owned();
        self.scene = SurfaceGraphicsScene::default();
        self.display_scene = SurfaceGraphicsScene::default();
        self.native_image_ids.clear();
        self.assets.clear();
        self.trusted_direct.clear();
        self.stale_images.clear();
        self.forced_delete_images.clear();
        self.reset_pending = true;
    }

    pub(crate) fn accepts_direct_asset(
        &self,
        key: &SurfaceGraphicsAssetKey,
        image_id: u32,
    ) -> bool {
        if matches!(key.source, SurfaceGraphicsSource::Terminal { .. }) {
            return !self.stale_images.contains(&image_id)
                && !self.forced_delete_images.contains(&image_id)
                && (image_id == native_host_image_id(&self.scope, key)
                    || image_id == (native_host_image_id(&self.scope, key) ^ NATIVE_SLOT_BIT))
                && !self
                    .display_scene
                    .placements
                    .iter()
                    .map(|p| &p.asset)
                    .chain(self.display_scene.retained_assets.iter())
                    .any(|visible| {
                        self.image_id(visible) == image_id
                            && self.host.images.get(&image_id)
                                == Some(&image_signature_from_asset(visible))
                    })
                && (self
                    .scene
                    .placements
                    .iter()
                    .any(|placement| &placement.asset == key)
                    || self.scene.retained_assets.contains(key));
        }
        image_id == host_image_id(&self.scope, key)
    }

    pub(crate) fn trust_direct_asset(
        &mut self,
        key: &SurfaceGraphicsAssetKey,
        image_id: u32,
    ) -> bool {
        if !self.accepts_direct_asset(key, image_id) {
            return false;
        }
        if matches!(key.source, SurfaceGraphicsSource::Terminal { .. }) {
            self.native_image_ids.insert(key.clone(), image_id);
        }
        self.host
            .images
            .insert(image_id, image_signature_from_asset(key));
        self.refresh_display_scene();
        if self
            .scene
            .placements
            .iter()
            .all(|placement| &placement.asset != key)
            && !self.scene.retained_assets.contains(key)
        {
            self.trusted_direct.insert(key.clone(), image_id);
        }
        true
    }

    pub(crate) fn retire_direct_image(&mut self, image_id: u32) {
        self.trusted_direct
            .retain(|_, trusted| *trusted != image_id);
        self.forced_delete_images.push(image_id);
    }

    pub(crate) fn take_pending_cleanup(&mut self) -> Vec<u8> {
        let mut bytes = if self.reset_pending {
            self.reset_pending = false;
            self.stale_images.clear();
            self.host.clear_bytes()
        } else {
            Vec::new()
        };
        self.forced_delete_images.sort_unstable();
        self.forced_delete_images.dedup();
        for image_id in self.forced_delete_images.drain(..) {
            self.host.images.remove(&image_id);
            self.host.placements.retain(|(id, _), _| *id != image_id);
            self.host.sources.retain(|_, id| *id != image_id);
            self.host
                .replayed_placements
                .retain(|(id, _)| *id != image_id);
            super::encode_delete_image(&mut bytes, image_id);
        }
        bytes
    }

    pub(crate) fn set_scene(&mut self, mut scene: SurfaceGraphicsScene) {
        let desired = scene
            .placements
            .iter()
            .map(|placement| placement.asset.clone())
            .chain(scene.retained_assets.iter().cloned())
            .collect::<HashSet<_>>();
        let unclaimed = self
            .trusted_direct
            .iter()
            .filter_map(|(key, image_id)| (!desired.contains(key)).then_some(*image_id))
            .collect::<Vec<_>>();
        self.stale_images.extend(unclaimed);
        self.trusted_direct.clear();
        let placed = scene
            .placements
            .iter()
            .map(|placement| placement.asset.clone())
            .collect::<HashSet<_>>();
        for asset in std::mem::take(&mut scene.assets) {
            if asset.data.len() as u64 == asset.key.data_len && placed.contains(&asset.key) {
                self.assets.insert(asset.key, Arc::from(asset.data));
            }
        }
        self.scene = scene;
        self.refresh_display_scene();
    }

    fn refresh_display_scene(&mut self) {
        let mut next = self.scene.clone();
        let previous_placements = self
            .display_scene
            .placements
            .iter()
            .map(|p| ((&p.asset.source, p.logical_placement_id, p.x, p.y), p))
            .collect::<HashMap<_, _>>();
        let previous_assets = self
            .display_scene
            .retained_assets
            .iter()
            .chain(self.display_scene.placements.iter().map(|p| &p.asset))
            .filter(|key| {
                self.host.images.get(&self.image_id(key)) == Some(&image_signature_from_asset(key))
            })
            .map(|key| (&key.source, key))
            .collect::<HashMap<_, _>>();
        for placement in &mut next.placements {
            let key = &placement.asset;
            if !matches!(key.source, SurfaceGraphicsSource::Terminal { .. })
                || self.assets.contains_key(key)
                || self.host.images.get(&self.image_id(key))
                    == Some(&image_signature_from_asset(key))
            {
                continue;
            }
            // A metadata-only replacement is not visible until its upload is ACKed.
            // Reuse only an identical placement, never stale resize/scroll geometry.
            if let Some(previous) = previous_placements
                .get(&(
                    &key.source,
                    placement.logical_placement_id,
                    placement.x,
                    placement.y,
                ))
                .copied()
                .filter(|previous| {
                    if self.host.images.get(&self.image_id(&previous.asset))
                        != Some(&image_signature_from_asset(&previous.asset))
                    {
                        return false;
                    }
                    let mut candidate = (*previous).clone();
                    candidate.asset = key.clone();
                    candidate == *placement
                })
            {
                placement.asset = previous.asset.clone();
            }
        }
        for key in &mut next.retained_assets {
            if matches!(key.source, SurfaceGraphicsSource::Terminal { .. })
                && !self.assets.contains_key(key)
                && self.host.images.get(&self.image_id(key))
                    != Some(&image_signature_from_asset(key))
            {
                if let Some(previous) = previous_assets.get(&key.source) {
                    *key = (*previous).clone();
                }
            }
        }
        let desired = next
            .placements
            .iter()
            .map(|p| &p.asset)
            .chain(next.retained_assets.iter())
            .collect::<HashSet<_>>();
        let stale = self
            .display_scene
            .placements
            .iter()
            .map(|p| &p.asset)
            .chain(self.display_scene.retained_assets.iter())
            .filter(|key| !desired.contains(key))
            .map(|key| self.image_id(key))
            .collect::<Vec<_>>();
        self.stale_images.extend(stale);
        let owned = self
            .scene
            .placements
            .iter()
            .map(|p| &p.asset)
            .chain(self.scene.retained_assets.iter())
            .chain(desired.iter().copied())
            .collect::<HashSet<_>>();
        self.assets.retain(|key, _| owned.contains(key));
        self.native_image_ids.retain(|key, _| owned.contains(key));
        self.display_scene = next;
    }

    pub(crate) fn encode_output(
        &mut self,
        visibility: Visibility,
        main_origin: (u16, u16),
        cell_size: HostCellSize,
        occlusion: &Occlusion,
    ) -> GraphicsOutput {
        let mut bytes = self.take_pending_cleanup();
        self.stale_images.sort_unstable();
        self.stale_images.dedup();
        for image_id in self.stale_images.drain(..) {
            if self.host.images.remove(&image_id).is_some() {
                super::encode_delete_image(&mut bytes, image_id);
            }
            self.host.placements.retain(|(id, _), _| *id != image_id);
            self.host.sources.retain(|_, id| *id != image_id);
            self.host
                .replayed_placements
                .retain(|(id, _)| *id != image_id);
        }
        if !cell_size.is_known() || self.scope.is_empty() {
            bytes.extend(self.host.clear_bytes());
            return GraphicsOutput::from_bytes(bytes);
        }

        let visible = self
            .display_scene
            .placements
            .iter()
            .filter_map(|placement| {
                client_placement_origin(placement, visibility, main_origin)
                    .map(|origin| (placement, origin))
            })
            .collect::<Vec<_>>();
        let pieces = occlusion.frame_pieces(&visible);
        let this = &*self;
        let placements = visible
            .iter()
            .zip(pieces)
            .flat_map(|(&(placement, origin), pieces)| {
                pieces.into_iter().enumerate().map(move |(index, piece)| {
                    let mut host = client_host_piece(
                        &this.scope,
                        placement,
                        origin,
                        cell_size,
                        piece,
                        index as u32,
                    );
                    host.host_image_id = Some(this.image_id(&placement.asset));
                    host.raw_data = this.assets.get(&placement.asset).map(Arc::clone);
                    host
                })
            })
            .collect::<Vec<_>>();
        let mut output = GraphicsOutput::from_bytes(bytes);
        self.host.request_placement_replay();
        output.extend(encode_graphics_output(&mut self.host, &placements));
        output
    }
}

pub(crate) const NATIVE_TRANSFER_BIT: u64 = 1 << 63;
pub(crate) const NATIVE_SLOT_BIT: u32 = 1 << 29;

pub(crate) fn native_host_image_id(scope: &str, key: &SurfaceGraphicsAssetKey) -> u32 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    scope.hash(&mut hasher);
    key.source.hash(&mut hasher);
    // Keep native IDs separate from the direct API namespace.
    0x4000_0000 | (hasher.finish() as u32 & 0x1fff_ffff)
}

pub(crate) fn host_image_id(scope: &str, key: &SurfaceGraphicsAssetKey) -> u32 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    scope.hash(&mut hasher);
    key.hash(&mut hasher);
    10_000 + ((hasher.finish() as u32) % 900_000)
}

fn client_placement_origin(
    placement: &SurfaceGraphicsPlacement,
    visibility: Visibility,
    main_origin: (u16, u16),
) -> Option<(u16, u16)> {
    match (&placement.asset.source, visibility) {
        (
            SurfaceGraphicsSource::Terminal {
                target: SurfaceGraphicsTarget::Pane { .. },
                ..
            },
            Visibility::Main,
        )
        | (SurfaceGraphicsSource::PaneLayer { .. }, Visibility::Main) => Some(main_origin),
        _ => None,
    }
}

fn client_host_piece(
    scope: &str,
    placement: &SurfaceGraphicsPlacement,
    origin: (u16, u16),
    cell_size: HostCellSize,
    piece: GridPiece,
    piece_index: u32,
) -> HostPlacement {
    let source_width = if placement.source_width == 0 {
        placement.asset.image_width
    } else {
        placement.source_width
    };
    let source_height = if placement.source_height == 0 {
        placement.asset.image_height
    } else {
        placement.source_height
    };
    let (source_x, piece_source_width) = piece_source_span(
        piece.col,
        piece.cols,
        placement.cols,
        cell_size.width_px,
        placement.x_offset,
        source_width,
    );
    let (source_y, piece_source_height) = piece_source_span(
        piece.row,
        piece.rows,
        placement.rows,
        cell_size.height_px,
        placement.y_offset,
        source_height,
    );
    let source_key = HostSourceKey::ClientSurface {
        scope: scope.to_owned(),
        source: placement.asset.source.clone(),
    };
    let signature = ImageSignature {
        image_width: placement.asset.image_width,
        image_height: placement.asset.image_height,
        format_code: format_code(placement.asset.format),
        data_len: usize::try_from(placement.asset.data_len).unwrap_or(usize::MAX),
        data_fingerprint: placement.asset.data_fingerprint,
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    scope.hash(&mut hasher);
    placement.asset.source.hash(&mut hasher);
    signature.hash(&mut hasher);
    let raw = hasher.finish();
    let pane_id = PaneId::from_raw((raw as u32).max(1));
    let host_image_id = host_image_id(scope, &placement.asset);
    let cols = piece.cols.min(u32::from(u16::MAX)) as u16;
    let rows = piece.rows.min(u32::from(u16::MAX)) as u16;
    HostPlacement {
        raw_data: None,
        pane_id,
        host_image_id: Some(host_image_id),
        area: Rect::new(
            origin
                .0
                .saturating_add(placement.x)
                .saturating_add(piece.col.min(u32::from(u16::MAX)) as u16),
            origin
                .1
                .saturating_add(placement.y)
                .saturating_add(piece.row.min(u32::from(u16::MAX)) as u16),
            cols,
            rows,
        ),
        cell_size,
        source_key,
        placement: KittyImagePlacement {
            // Only feeds the host placement id, so each piece gets its own.
            image_id: 1 + piece_index,
            placement_id: placement.logical_placement_id,
            z: placement.z,
            x_offset: if piece.col == 0 {
                placement.x_offset
            } else {
                0
            },
            y_offset: if piece.row == 0 {
                placement.y_offset
            } else {
                0
            },
            image_width: placement.asset.image_width,
            image_height: placement.asset.image_height,
            format: match placement.asset.format {
                SurfaceGraphicsFormat::Rgb => KittyImageFormat::Rgb,
                SurfaceGraphicsFormat::Rgba => KittyImageFormat::Rgba,
                SurfaceGraphicsFormat::Png => KittyImageFormat::Png,
            },
            data_len: usize::try_from(placement.asset.data_len).unwrap_or(usize::MAX),
            data_fingerprint: placement.asset.data_fingerprint,
            source_file: None,
            data: Vec::new(),
            render: KittyPlacementRenderInfo {
                pixel_width: piece.cols.saturating_mul(cell_size.width_px),
                pixel_height: piece.rows.saturating_mul(cell_size.height_px),
                grid_cols: piece.cols,
                grid_rows: piece.rows,
                viewport_col: 0,
                viewport_row: 0,
                source_x: placement.source_x.saturating_add(source_x),
                source_y: placement.source_y.saturating_add(source_y),
                source_width: piece_source_width,
                source_height: piece_source_height,
            },
        },
        scrollback_offset: placement.scrollback_offset,
    }
}

/// Maps a piece's cells onto the source pixels shown there. The leading pixel
/// offset shrinks the image inside its cells, and cuts use absolute boundaries so
/// adjacent pieces share exact source edges.
fn piece_source_span(
    start_cell: u32,
    cells: u32,
    total_cells: u32,
    cell_px: u32,
    offset: u32,
    source_len: u32,
) -> (u32, u32) {
    let display = u64::from(total_cells) * u64::from(cell_px);
    let offset = u64::from(offset).min(display.saturating_sub(1));
    let edge = |cell: u32| {
        let pixel = (u64::from(cell) * u64::from(cell_px)).max(offset);
        ((pixel - offset) * u64::from(source_len) / (display - offset).max(1)) as u32
    };
    let start = edge(start_cell);
    let end = edge(start_cell + cells);
    if end > start {
        (start, end - start)
    } else {
        // An upscaled sliver still needs one source pixel; zero means "whole image".
        (start.min(source_len.saturating_sub(1)), 1)
    }
}

fn format_code(format: SurfaceGraphicsFormat) -> u32 {
    match format {
        SurfaceGraphicsFormat::Rgb => 24,
        SurfaceGraphicsFormat::Rgba => 32,
        SurfaceGraphicsFormat::Png => 100,
    }
}

fn image_signature_from_asset(key: &SurfaceGraphicsAssetKey) -> ImageSignature {
    ImageSignature {
        image_width: key.image_width,
        image_height: key.image_height,
        format_code: format_code(key.format),
        data_len: usize::try_from(key.data_len).unwrap_or(usize::MAX),
        data_fingerprint: key.data_fingerprint,
    }
}

#[cfg(test)]
#[path = "surface_collector_tests.rs"]
mod collector_tests;

#[cfg(test)]
#[path = "surface_tests.rs"]
mod tests;
