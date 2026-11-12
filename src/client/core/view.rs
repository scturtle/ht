//! Presentation-only terminals. No PTYs, process inspection or agent detection.
use std::collections::HashMap;
use std::io;
use std::time::Instant;

mod synchronized;
use synchronized::{FrozenPane, RetainedPane};

use ratatui::{
    backend::TestBackend,
    layout::{Direction, Rect},
    style::Style,
    widgets::Block,
    Terminal,
};

use super::model::Model;
use crate::layout::{PaneId, PaneInfo};
use crate::pane::{GhosttyPaneTerminal, PaneTerminal};
use crate::protocol::*;

#[derive(Clone)]
pub(super) struct ChromeConfig {
    pub pane_borders: crate::config::PaneBordersConfig,
    pub pane_gaps: bool,
    pub pane_outer_borders: bool,
    pub palette: crate::ui::Palette,
}
impl ChromeConfig {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            pane_borders: config.ui.pane_borders,
            pane_gaps: config.ui.pane_gaps,
            pane_outer_borders: config.ui.pane_outer_borders,
            palette: crate::ui::ThemeRuntimeConfig::from_config(config).palette(),
        }
    }
}

pub(in crate::client) struct View {
    pub terminal: PaneTerminal,
    pub channel: u32,
    pub revision: u64,
    /// Authoritative parser grid; changed only by Core RESIZE frames.
    pub size: (u16, u16, u32, u32),
    /// This client's desired grid; never infer it from another client's winner.
    pub requested: (u16, u16, u32, u32),
    pub closed: bool,
    pub visible: bool,
    synchronized: synchronized::Update,
}

impl View {
    /// Ordered attach keeps one parser fed by live bytes: the first paint is
    /// ordinary DATA preceded by an authoritative RESIZE. Only CLOSE ends it.
    /// The RESIZE payload that claims this pane's geometry (`latest` policy).
    pub fn resize_request(&self) -> [u8; 4] {
        let [cols_lo, cols_hi] = self.requested.0.to_le_bytes();
        let [rows_lo, rows_hi] = self.requested.1.to_le_bytes();
        [cols_lo, cols_hi, rows_lo, rows_hi]
    }

    /// Revealing a formerly passive pane must announce its desired grid even
    /// when that request is unchanged: another viewer may currently own Core's
    /// authoritative geometry. Hidden panes do not update or send requests.
    pub fn request_visible_size(
        &mut self,
        visible: bool,
        size: (u16, u16, u32, u32),
    ) -> Option<(bool, bool)> {
        let revealed = visible && !self.visible;
        self.visible = visible;
        if !visible {
            return None;
        }
        let (grid, display) = self.request_size(size);
        Some((grid || revealed, display))
    }

    pub fn request_size(&mut self, size: (u16, u16, u32, u32)) -> (bool, bool) {
        let grid = (self.requested.0, self.requested.1) != (size.0, size.1);
        let display = (self.requested.2, self.requested.3) != (size.2, size.3);
        self.requested = size;
        (grid, display)
    }
    pub fn authoritative_size(&mut self, size: (u16, u16, u32, u32)) -> Vec<bytes::Bytes> {
        self.apply_size(size)
    }
    pub fn display_cells(&mut self, width: u32, height: u32) -> Vec<bytes::Bytes> {
        self.apply_size((self.size.0, self.size.1, width, height))
    }
    fn apply_size(&mut self, size: (u16, u16, u32, u32)) -> Vec<bytes::Bytes> {
        if self.size == size {
            return Vec::new();
        }
        let replies = self.terminal.resize(size.1, size.0, size.2, size.3);
        self.size = size;
        self.revision = self.revision.saturating_add(2);
        replies
    }
    pub fn process_output(&mut self, id: PaneId, bytes: &[u8]) -> crate::pane::ProcessBytesResult {
        let mut result = self.terminal.process_pty_bytes(id, bytes);
        self.synchronized.observe(result.synchronized_output);
        self.revision = self.revision.saturating_add(2);
        result.request_render &= self.visible;
        result
    }

    pub fn synchronized_deadline(&self) -> Option<Instant> {
        (!self.closed)
            .then(|| self.synchronized.deadline())
            .flatten()
    }

    pub fn expire_synchronized(&mut self, now: Instant) -> bool {
        let expired = self.synchronized.expire(now);
        if expired {
            self.terminal.end_synchronized_output();
        }
        expired
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.synchronized.observe(false);
        self.terminal.end_synchronized_output();
    }

    fn synchronized_held(&self) -> bool {
        self.synchronized_deadline().is_some()
    }

    #[cfg(test)]
    pub fn new(cols: u16, rows: u16, channel: u32) -> io::Result<Self> {
        Self::with_scrollback(cols, rows, channel, &Default::default())
    }

    pub fn with_scrollback(
        cols: u16,
        rows: u16,
        channel: u32,
        scrollback: &crate::config::AdvancedConfig,
    ) -> io::Result<Self> {
        // A byte-only default can discard the requested history before the row
        // target is reached. Rows bound retention; bytes are an optional extra cap.
        let bytes = if scrollback.scrollback_limit_lines == 0 {
            0
        } else {
            scrollback.scrollback_limit_bytes.unwrap_or(usize::MAX)
        };
        let mut terminal =
            crate::ghostty::Terminal::new(cols, rows, bytes).map_err(io::Error::other)?;
        terminal
            .set_scrollback_limit_lines(scrollback.scrollback_limit_lines as usize)
            .map_err(io::Error::other)?;
        Ok(Self {
            terminal: PaneTerminal::new(GhosttyPaneTerminal::new(terminal)?),
            channel,
            revision: 2,
            size: (cols, rows, 0, 0),
            requested: (cols, rows, 0, 0),
            closed: false,
            visible: true,
            synchronized: Default::default(),
        })
    }
}

/// What one ordered presentation frame changed, if anything.
pub(super) enum FrameOutcome {
    Output(crate::pane::ProcessBytesResult),
    Changed,
}

/// Apply one frame from an ordered attach channel. The canonical first paint is
/// ordinary DATA preceded by an authoritative RESIZE: there is no begin/end
/// boundary, so nothing here withholds presentation or admits input.
pub(super) fn apply_frame(
    view: &mut View,
    id: PaneId,
    kind: u8,
    payload: &[u8],
    cells: (u32, u32),
) -> io::Result<Option<FrameOutcome>> {
    use super::transport::{CLOSE, DATA, RESIZE};
    if view.closed && kind != CLOSE {
        return Err(super::model::invalid("Core presentation frame after CLOSE"));
    }
    match kind {
        DATA => Ok(Some(FrameOutcome::Output(view.process_output(id, payload)))),
        RESIZE => {
            let bytes: [u8; 4] = payload
                .try_into()
                .map_err(|_| super::model::invalid("Core RESIZE must contain 4 bytes"))?;
            let cols = u16::from_le_bytes([bytes[0], bytes[1]]);
            let rows = u16::from_le_bytes([bytes[2], bytes[3]]);
            if cols == 0 || rows == 0 {
                return Err(super::model::invalid("Core RESIZE is zero"));
            }
            // Authoritative even when another client currently owns the grid.
            view.authoritative_size((cols, rows, cells.0, cells.1));
            Ok(Some(FrameOutcome::Changed))
        }
        CLOSE => {
            view.close();
            Ok(Some(FrameOutcome::Changed))
        }
        _ => Err(super::model::invalid("unexpected Core presentation frame")),
    }
}

/// The leftmost leaf of a layout: the pane that occupied the root position.
pub(super) fn first_leaf(node: &crate::layout::Node) -> Option<PaneId> {
    match node {
        crate::layout::Node::Pane(id) => Some(*id),
        crate::layout::Node::Split { first, .. } => first_leaf(first),
    }
}

pub(super) fn pane_infos(model: &Model, area: Rect, config: &ChromeConfig) -> Vec<PaneInfo> {
    let Some(layout) = model.layout() else {
        return Vec::new();
    };
    if model.active_zoomed() {
        vec![zoomed_info(layout, area, config)]
    } else {
        layout_infos(layout, area, config)
    }
}

/// Same full-area geometry as the original zoom path; hidden BSP leaves remain
/// available for navigation but must not receive a competing resize.
pub(super) fn zoomed_info(
    layout: &crate::layout::TileLayout,
    area: Rect,
    config: &ChromeConfig,
) -> PaneInfo {
    crate::ui::zoomed_pane_info(
        layout.focused(),
        area,
        config.pane_borders,
        config.pane_outer_borders,
        layout.pane_count() > 1,
    )
}

pub(super) fn layout_infos(
    layout: &crate::layout::TileLayout,
    area: Rect,
    config: &ChromeConfig,
) -> Vec<PaneInfo> {
    crate::ui::apply_pane_chrome(
        layout.panes(area),
        config.pane_borders,
        config.pane_gaps,
        config.pane_outer_borders,
    )
    .into_iter()
    .map(|mut p| {
        p.inner_rect = crate::ui::pane_inner_rect(p.rect, p.borders);
        p
    })
    .collect()
}

/// Original public layout replies retain the hidden BSP while zoomed and apply
/// the same pane chrome as the native API (not the full-screen surface geometry).
pub(super) fn api_layout_snapshot(
    model: &Model,
    tab: &str,
    area: Rect,
    config: &ChromeConfig,
) -> io::Result<crate::actions::PaneLayoutSnapshot> {
    let mut snapshot = if model.active_tab() == Some(tab) {
        model.layout_snapshot(area)?
    } else {
        model.layout_snapshot_for(tab, area)?
    };
    if let Some(layout) = model.layouts.get(tab) {
        snapshot.panes = layout_infos(layout, area, config)
            .into_iter()
            .map(|pane| crate::actions::PaneLayoutPane {
                pane_id: model.records[&pane.id].address.clone(),
                focused: pane.is_focused,
                rect: pane.rect.into(),
            })
            .collect();
    }
    Ok(snapshot)
}

struct RetainedSurface {
    panes: Vec<RetainedPane>,
    splits: Vec<PaneSurfaceSplit>,
    cursor: Option<CursorState>,
    hyperlinks: Vec<((u16, u16), String, String)>,
    /// Committed graphics scene. A metadata-only projection must keep the images
    /// already drawn instead of republishing an empty scene (which would delete
    /// them from the host terminal).
    graphics: SurfaceGraphicsScene,
}
pub(super) struct Renderer {
    // Use ratatui only to supply a Frame over its semantic buffer. Never flush
    // diffs into TestBackend: it retains stale symbols under wide glyphs.
    terminal: Terminal<TestBackend>,
    #[cfg(test)]
    pub profile_ns: [u128; 5],
    revision: u64,
    retained: Option<RetainedSurface>,
    frozen: Vec<FrozenPane>,
    /// Host cell pixels; the graphics collector scales placements with it.
    cell_size: crate::kitty_graphics::HostCellSize,
    /// One delivery record per client connection, exactly like the server's.
    delivery: crate::kitty_graphics::surface::DeliveryCache,
}

impl Renderer {
    /// A scene whose byte budget bounded its assets needs another projection.
    pub fn graphics_pending(&self) -> bool {
        self.delivery.has_pending()
    }

    /// A new Core connection invalidates the committed surface, but not what the
    /// host terminal already displays: keep delivered images so a reconnect does
    /// not re-transmit them, and drop the stale frame/pane geometry.
    pub fn resume_connection(&mut self) {
        self.retained = None;
        self.frozen.clear();
    }

    /// Host cell pixels come from the client surface, like the server's grid.
    pub fn set_cell_size(&mut self, cells: (u32, u32)) {
        self.cell_size = crate::kitty_graphics::HostCellSize {
            width_px: cells.0,
            height_px: cells.1,
        };
    }

    pub fn new() -> io::Result<Self> {
        Ok(Self {
            terminal: Terminal::new(TestBackend::new(1, 1)).map_err(io::Error::other)?,
            #[cfg(test)]
            profile_ns: [0; 5],
            revision: 0,
            retained: None,
            frozen: Vec::new(),
            cell_size: crate::kitty_graphics::HostCellSize::default(),
            delivery: crate::kitty_graphics::surface::DeliveryCache::default(),
        })
    }

    /// A metadata-only snapshot still needs a matching projection revision. Reuse
    /// the committed terminal grid: no pane traversal, Ghostty locks or painting.
    pub fn retain_projection(&mut self, model: &Model) -> Option<PaneSurfaceFrame> {
        let retained = self.retained.as_ref()?;
        self.revision += 1;
        Some(PaneSurfaceFrame {
            boot_id: model.boot.clone(),
            projection_revision: model.revision,
            surface_revision: self.revision,
            frame: FrameData::from_ratatui_buffer_with_hyperlinks(
                self.terminal.current_buffer_mut(),
                retained.cursor.clone(),
                &retained.hyperlinks,
            ),
            panes: retained.panes.iter().map(|p| p.pane.clone()).collect(),
            splits: retained.splits.clone(),
            graphics: retained.graphics.clone(),
        })
    }

    pub fn compose(
        &mut self,
        model: &Model,
        views: &HashMap<PaneId, View>,
        size: ClientSurfaceSize,
        config: &ChromeConfig,
    ) -> io::Result<PaneSurfaceFrame> {
        #[cfg(test)]
        let mut stage = std::time::Instant::now();
        let area = Rect::new(0, 0, size.cols.max(1), size.rows.max(1));
        let infos = pane_infos(model, area, config);
        self.frozen.retain(|pane| {
            views
                .get(&pane.id)
                .is_some_and(|v| v.visible && v.channel == pane.channel && v.synchronized_held())
        });
        for p in &infos {
            let Some(view) = views.get(&p.id).filter(|v| v.synchronized_held()) else {
                continue;
            };
            if self.frozen.iter().any(|f| f.id == p.id) {
                continue;
            }
            let previous = self.retained.as_ref().and_then(|surface| {
                surface
                    .panes
                    .iter()
                    .find(|old| old.id == p.id && old.channel == view.channel)
                    .map(|old| (old, surface))
            });
            self.frozen.push(FrozenPane::capture(
                p.id,
                view.channel,
                previous,
                self.terminal.current_buffer_mut(),
                &self.delivery,
            ));
        }
        // Capture held regions before replacing the canvas on a geometry change.
        if self.terminal.size().map_err(io::Error::other)? != area.as_size() {
            self.terminal = Terminal::new(TestBackend::new(area.width, area.height))
                .map_err(io::Error::other)?;
        }
        #[cfg(test)]
        {
            self.profile_ns[0] += stage.elapsed().as_nanos();
            stage = std::time::Instant::now();
        }
        let mut cursor = None;
        let mut hyperlinks = Vec::new();
        for p in &infos {
            if let Some(frozen) = self.frozen.iter().find(|f| f.id == p.id) {
                hyperlinks.extend(frozen.links(p.inner_rect));
                if p.is_focused {
                    cursor = frozen.cursor(p.inner_rect);
                }
            } else if let Some(view) = views.get(&p.id) {
                hyperlinks.extend(view.terminal.visible_hyperlinks(p.inner_rect));
                if p.is_focused {
                    cursor = view.terminal.cursor_state().map(|c| CursorState {
                        x: p.inner_rect.x.saturating_add(c.x),
                        y: p.inner_rect.y.saturating_add(c.y),
                        visible: c.visible && c.x < p.inner_rect.width && c.y < p.inner_rect.height,
                        shape: c.shape,
                    });
                }
            }
        }
        #[cfg(test)]
        {
            self.profile_ns[1] += stage.elapsed().as_nanos();
            stage = std::time::Instant::now();
        }
        let mut canvas = self.terminal.get_frame();
        canvas.buffer_mut().reset();
        for p in &infos {
            canvas.render_widget(
                Block::default()
                    .borders(p.borders)
                    .border_style(Style::default().fg(if p.is_focused {
                        config.palette.accent
                    } else {
                        config.palette.overlay0
                    })),
                p.rect,
            );
            if let Some(frozen) = self.frozen.iter().find(|f| f.id == p.id) {
                frozen.paint(canvas.buffer_mut(), p.inner_rect);
            } else if let Some(view) = views.get(&p.id) {
                view.terminal
                    .render(&mut canvas, p.inner_rect, p.is_focused);
            }
        }
        #[cfg(test)]
        {
            self.profile_ns[2] += stage.elapsed().as_nanos();
            stage = std::time::Instant::now();
        }
        let frame = FrameData::from_ratatui_buffer_with_hyperlinks(
            canvas.buffer_mut(),
            cursor.clone(),
            &hyperlinks,
        );
        #[cfg(test)]
        {
            self.profile_ns[3] += stage.elapsed().as_nanos();
            stage = std::time::Instant::now();
        }
        let panes: Vec<_> = infos
            .iter()
            .filter_map(|p| {
                let view = views.get(&p.id)?;
                let record = model.records.get(&p.id)?;
                let frozen = self.frozen.iter().find(|f| f.id == p.id);
                let scroll = if let Some(frozen) = frozen {
                    frozen.scroll
                } else {
                    view.terminal
                        .scroll_metrics()
                        .map(|s| PaneSurfaceScrollMetrics {
                            offset_from_bottom: s.offset_from_bottom as u64,
                            max_offset_from_bottom: s.max_offset_from_bottom as u64,
                            viewport_rows: s.viewport_rows as u64,
                        })
                };
                Some(RetainedPane {
                    id: p.id,
                    channel: view.channel,
                    pane: PaneSurfacePane {
                        pane_id: record.address.clone(),
                        content_revision: frozen.map_or(view.revision, |f| f.revision),
                        rect: p.rect.into(),
                        inner_rect: p.inner_rect.into(),
                        scrollbar_rect: None,
                        scroll,
                        focused: p.is_focused,
                        mouse_reporting: !view.closed && view.terminal.mouse_reporting_enabled(),
                        sgr_pixel_mouse: !view.closed && view.terminal.sgr_pixel_mouse_enabled(),
                        alternate_screen_active: view.terminal.alternate_screen_active(),
                        pixel_width: view.size.2.saturating_mul(u32::from(view.size.0)),
                        pixel_height: view.size.3.saturating_mul(u32::from(view.size.1)),
                    },
                })
            })
            .collect();
        let splits: Vec<_> = model
            .layout()
            .filter(|_| !model.active_zoomed())
            .map(|l| {
                l.splits(area)
                    .into_iter()
                    .map(|s| {
                        let horizontal = s.direction == Direction::Horizontal;
                        let hit = if horizontal {
                            Rect::new(
                                s.pos.saturating_sub(1),
                                s.area.y,
                                2.min(s.area.width),
                                s.area.height,
                            )
                        } else {
                            Rect::new(
                                s.area.x,
                                s.pos.saturating_sub(1),
                                s.area.width,
                                2.min(s.area.height),
                            )
                        };
                        PaneSurfaceSplit {
                            direction: if horizontal {
                                PaneSurfaceSplitDirection::Horizontal
                            } else {
                                PaneSurfaceSplitDirection::Vertical
                            },
                            pos: s.pos,
                            area: s.area.into(),
                            hit_rect: hit.into(),
                            path: s.path,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let graphics = self.compose_graphics(model, views, &infos);
        let public_panes = panes.iter().map(|p| p.pane.clone()).collect();
        self.retained = Some(RetainedSurface {
            panes,
            splits: splits.clone(),
            cursor: cursor.clone(),
            hyperlinks,
            graphics: graphics.clone(),
        });
        #[cfg(test)]
        {
            self.profile_ns[4] += stage.elapsed().as_nanos();
        }
        self.revision += 1;
        Ok(PaneSurfaceFrame {
            boot_id: model.boot.clone(),
            projection_revision: model.revision,
            surface_revision: self.revision,
            frame,
            panes: public_panes,
            splits,
            graphics,
        })
    }

    /// Same collector the retired server used: visible panes of the composed
    /// surface plus retained (hidden) panes for offscreen asset retention.
    fn compose_graphics(
        &mut self,
        model: &Model,
        views: &HashMap<PaneId, View>,
        infos: &[PaneInfo],
    ) -> SurfaceGraphicsScene {
        if !crate::kitty_graphics::is_enabled() || !self.cell_size.is_known() {
            self.delivery = Default::default();
            return SurfaceGraphicsScene::default();
        }
        let pane = |info: &PaneInfo| {
            if self.frozen.iter().any(|f| f.id == info.id) {
                return None;
            }
            let view = views.get(&info.id)?;
            let record = model.records.get(&info.id)?;
            Some(crate::kitty_graphics::surface::ScenePane {
                id: info.id,
                public_id: record.address.clone(),
                inner_rect: info.inner_rect,
                terminal: &view.terminal,
            })
        };
        let surface = infos.iter().filter_map(pane).collect::<Vec<_>>();
        let retained = model
            .records
            .iter()
            .filter(|(id, _)| {
                !infos.iter().any(|info| info.id == **id)
                    && !self.frozen.iter().any(|f| f.id == **id)
            })
            .filter_map(|(id, record)| {
                views
                    .get(id)
                    .map(|view| crate::kitty_graphics::surface::ScenePane {
                        id: *id,
                        public_id: record.address.clone(),
                        inner_rect: Rect::default(),
                        terminal: &view.terminal,
                    })
            })
            .collect::<Vec<_>>();
        let placement_limit = crate::kitty_graphics::surface::MAX_SURFACE_GRAPHICS_PLACEMENTS
            .saturating_sub(self.frozen.iter().map(FrozenPane::placement_count).sum());
        let (mut scene, mut delivery, _sources) = if self.frozen.is_empty() {
            crate::kitty_graphics::surface::collect_scene(
                &surface,
                &retained,
                self.cell_size,
                &self.delivery,
            )
        } else {
            crate::kitty_graphics::surface::collect_scene_with_limit(
                &surface,
                &retained,
                self.cell_size,
                &self.delivery,
                placement_limit,
            )
        };
        for frozen in &self.frozen {
            if let Some(info) = infos.iter().find(|p| p.id == frozen.id) {
                frozen.graphics(info.inner_rect, &mut scene, &mut delivery);
            }
        }
        self.delivery = delivery;
        scene
    }
}
