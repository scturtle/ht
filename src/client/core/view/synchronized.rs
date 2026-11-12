//! A bounded synchronized update holds only this pane's last committed visuals.
//! The parser, replies, input protocols and unrelated panes keep progressing.
use std::time::{Duration, Instant};

use ratatui::{buffer::Buffer, layout::Rect};
use unicode_width::UnicodeWidthStr;

use super::*;

const TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct Update {
    active: bool,
    until: Option<Instant>,
}
impl Update {
    pub fn observe(&mut self, active: bool) {
        if !active {
            self.until = None;
        } else if !self.active {
            self.until = Some(Instant::now() + TIMEOUT);
        }
        self.active = active;
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.until
    }
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.until.is_some_and(|until| now >= until) {
            // The caller resets only the local parser's 2026 mode. Subsequent
            // DATA without another BEGIN cannot rearm this bounded wait.
            self.active = false;
            self.until = None;
            true
        } else {
            false
        }
    }
}

pub(super) struct RetainedPane {
    pub id: PaneId,
    pub channel: u32,
    pub pane: PaneSurfacePane,
}

/// Captured lazily, only if composition actually overlaps an open update. Never
/// copy an extra screen on the ordinary render or per-DATA path. Coordinates in
/// this snapshot are pane-local, so latest geometry can crop/move it safely.
pub(super) struct FrozenPane {
    pub id: PaneId,
    pub channel: u32,
    pub revision: u64,
    pub scroll: Option<PaneSurfaceScrollMetrics>,
    cells: Buffer,
    cursor: Option<CursorState>,
    links: Vec<((u16, u16), String, String)>,
    placements: Vec<SurfaceGraphicsPlacement>,
    assets: Vec<SurfaceGraphicsAssetKey>,
}
impl FrozenPane {
    pub fn capture(
        id: PaneId,
        channel: u32,
        previous: Option<(&RetainedPane, &RetainedSurface)>,
        buffer: &Buffer,
        delivery: &crate::kitty_graphics::surface::DeliveryCache,
    ) -> Self {
        let mut frozen = Self {
            id,
            channel,
            revision: 0,
            scroll: None,
            cells: Buffer::empty(Rect::default()),
            cursor: None,
            links: Vec::new(),
            placements: Vec::new(),
            assets: Vec::new(),
        };
        let Some((retained, surface)) = previous else {
            return frozen;
        };
        let pane = &retained.pane;
        let r = pane.inner_rect;
        let area = Rect::new(r.x, r.y, r.width, r.height);
        frozen.revision = pane.content_revision;
        frozen.scroll = pane.scroll;
        frozen.cells = Buffer::empty(Rect::new(0, 0, area.width, area.height));
        for y in 0..area.height {
            for x in 0..area.width {
                frozen.cells[(x, y)] = buffer[(area.x + x, area.y + y)].clone();
            }
        }
        frozen.cursor = surface
            .cursor
            .clone()
            .filter(|_| pane.focused)
            .map(|mut c| {
                c.x = c.x.saturating_sub(area.x);
                c.y = c.y.saturating_sub(area.y);
                c
            });
        frozen.links = surface
            .hyperlinks
            .iter()
            .filter(|((x, y), _, _)| area.contains((*x, *y).into()))
            .map(|((x, y), symbol, uri)| ((*x - area.x, *y - area.y), symbol.clone(), uri.clone()))
            .collect();
        let belongs = |key: &SurfaceGraphicsAssetKey| {
            matches!(&key.source,
            SurfaceGraphicsSource::Terminal { target: SurfaceGraphicsTarget::Pane { pane_id }, .. }
            if pane_id == &pane.pane_id)
        };
        frozen.placements = surface
            .graphics
            .placements
            .iter()
            .filter(|p| belongs(&p.asset) && delivery.contains(&p.asset))
            .map(|p| {
                let mut p = p.clone();
                p.x = p.x.saturating_sub(area.x);
                p.y = p.y.saturating_sub(area.y);
                p
            })
            .collect();
        frozen.assets = surface
            .graphics
            .retained_assets
            .iter()
            .filter(|k| belongs(k))
            .cloned()
            .collect();
        frozen
    }

    pub fn paint(&self, buffer: &mut Buffer, area: Rect) {
        let width = area.width.min(self.cells.area.width);
        let height = area.height.min(self.cells.area.height);
        for y in 0..height {
            for x in 0..width {
                buffer[(area.x + x, area.y + y)] = self.cells[(x, y)].clone();
            }
            // Do not strand half a wide glyph against a newly narrowed border.
            if width > 0 && buffer[(area.x + width - 1, area.y + y)].symbol().width() > 1 {
                buffer[(area.x + width - 1, area.y + y)].reset();
            }
        }
    }
    pub fn cursor(&self, area: Rect) -> Option<CursorState> {
        self.cursor.clone().map(|mut c| {
            c.visible &= c.x < area.width && c.y < area.height;
            c.x = c.x.saturating_add(area.x);
            c.y = c.y.saturating_add(area.y);
            c
        })
    }
    pub fn links(&self, area: Rect) -> impl Iterator<Item = ((u16, u16), String, String)> + '_ {
        self.links
            .iter()
            .filter(move |((x, y), _, _)| *x < area.width && *y < area.height)
            .map(move |((x, y), symbol, uri)| {
                ((area.x + x, area.y + y), symbol.clone(), uri.clone())
            })
    }
    pub fn placement_count(&self) -> usize {
        // Reserve the whole committed scene, including temporarily cropped
        // images. They must stay cached if the pane grows again before END.
        self.placements.len() + self.assets.len()
    }

    pub fn graphics(
        &self,
        area: Rect,
        scene: &mut SurfaceGraphicsScene,
        delivery: &mut crate::kitty_graphics::surface::DeliveryCache,
    ) {
        for key in &self.assets {
            scene.retained_assets.push(key.clone());
            delivery.retain_committed(key);
        }
        for p in &self.placements {
            delivery.retain_committed(&p.asset);
            if p.x >= area.width || p.y >= area.height {
                // These keys belong to the bounded, already-visible committed
                // scene, not new offscreen uploads. Keep bytes on the client so
                // a resize back cannot require a now-deleted native image.
                scene.retained_assets.push(p.asset.clone());
                continue;
            }
            let mut clipped = p.clone();
            clipped.cols = p.cols.min(u32::from(area.width - p.x));
            clipped.rows = p.rows.min(u32::from(area.height - p.y));
            clipped.source_width = ((u64::from(p.source_width) * u64::from(clipped.cols))
                / u64::from(p.cols.max(1)))
            .max(1) as u32;
            clipped.source_height = ((u64::from(p.source_height) * u64::from(clipped.rows))
                / u64::from(p.rows.max(1)))
            .max(1) as u32;
            clipped.x += area.x;
            clipped.y += area.y;
            delivery.retain_committed(&clipped.asset);
            scene.placements.push(clipped);
        }
    }
}
