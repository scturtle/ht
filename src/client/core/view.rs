//! Presentation-only terminals. No PTYs, process inspection or agent detection.
use std::collections::HashMap;
use std::io;
use std::time::Instant;

mod synchronized;
use synchronized::{FrozenPane, RetainedPane};

use ratatui::{backend::TestBackend, layout::Rect, Terminal};

use super::model::Model;
use crate::layout::PaneId;
use crate::pane::{GhosttyPaneTerminal, PaneTerminal};
use crate::protocol::*;

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
        // target is reached. Only the row limit bounds retained history.
        let bytes = if scrollback.scrollback_limit_lines == 0 {
            0
        } else {
            usize::MAX
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

struct RetainedSurface {
    pane: Option<RetainedPane>,
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
    frozen: Option<FrozenPane>,
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
        self.frozen = None;
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
            frozen: None,
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
            panes: retained.pane.iter().map(|p| p.pane.clone()).collect(),
            graphics: retained.graphics.clone(),
        })
    }

    pub fn compose(
        &mut self,
        model: &Model,
        views: &HashMap<PaneId, View>,
        size: ClientSurfaceSize,
    ) -> io::Result<PaneSurfaceFrame> {
        #[cfg(test)]
        let mut stage = std::time::Instant::now();
        let area = Rect::new(0, 0, size.cols.max(1), size.rows.max(1));
        let current = model.focused().and_then(|id| Some((id, views.get(&id)?)));
        let held = current.filter(|(_, view)| view.synchronized_held());
        match held {
            Some((id, view)) => {
                if !self
                    .frozen
                    .as_ref()
                    .is_some_and(|pane| pane.id == id && pane.channel == view.channel)
                {
                    let previous = self.retained.as_ref().and_then(|surface| {
                        let pane = surface.pane.as_ref()?;
                        (pane.id == id && pane.channel == view.channel).then_some((pane, surface))
                    });
                    self.frozen = Some(FrozenPane::capture(
                        id,
                        view.channel,
                        previous,
                        self.terminal.current_buffer_mut(),
                        &self.delivery,
                    ));
                }
            }
            None => self.frozen = None,
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
        let (cursor, hyperlinks) = if let Some(frozen) = &self.frozen {
            (frozen.cursor(area), frozen.links(area).collect())
        } else if let Some((_, view)) = current {
            let cursor = view.terminal.cursor_state().map(|c| CursorState {
                x: c.x,
                y: c.y,
                visible: c.visible && c.x < area.width && c.y < area.height,
                shape: c.shape,
            });
            (cursor, view.terminal.visible_hyperlinks(area))
        } else {
            (None, Vec::new())
        };
        #[cfg(test)]
        {
            self.profile_ns[1] += stage.elapsed().as_nanos();
            stage = std::time::Instant::now();
        }
        let mut canvas = self.terminal.get_frame();
        canvas.buffer_mut().reset();
        if let Some(frozen) = &self.frozen {
            frozen.paint(canvas.buffer_mut(), area);
        } else if let Some((_, view)) = current {
            view.terminal.render(&mut canvas, area, true);
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
        let pane = current.and_then(|(id, view)| {
            let record = model.records.get(&id)?;
            let frozen = self.frozen.as_ref();
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
                id,
                channel: view.channel,
                pane: PaneSurfacePane {
                    pane_id: record.address.clone(),
                    content_revision: frozen.map_or(view.revision, |f| f.revision),
                    rect: area.into(),
                    inner_rect: area.into(),
                    scrollbar_rect: None,
                    scroll,
                    focused: true,
                    mouse_reporting: !view.closed && view.terminal.mouse_reporting_enabled(),
                    sgr_pixel_mouse: !view.closed && view.terminal.sgr_pixel_mouse_enabled(),
                    alternate_screen_active: view.terminal.alternate_screen_active(),
                    pixel_width: view.size.2.saturating_mul(u32::from(view.size.0)),
                    pixel_height: view.size.3.saturating_mul(u32::from(view.size.1)),
                },
            })
        });
        let graphics = self.compose_graphics(model, views, current.map(|(id, _)| id), area);
        let public_panes = pane.iter().map(|p| p.pane.clone()).collect();
        self.retained = Some(RetainedSurface {
            pane,
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
            graphics,
        })
    }

    /// The visible pane supplies the scene; hidden panes only retain already
    /// delivered offscreen assets.
    fn compose_graphics(
        &mut self,
        model: &Model,
        views: &HashMap<PaneId, View>,
        visible: Option<PaneId>,
        area: Rect,
    ) -> SurfaceGraphicsScene {
        if !self.cell_size.is_known() {
            self.delivery = Default::default();
            return SurfaceGraphicsScene::default();
        }
        use crate::kitty_graphics::surface::ScenePane;
        let surface = visible.filter(|_| self.frozen.is_none()).and_then(|id| {
            Some(ScenePane {
                id,
                public_id: model.records.get(&id)?.address.clone(),
                inner_rect: area,
                terminal: &views.get(&id)?.terminal,
            })
        });
        let retained = model
            .records
            .iter()
            .filter(|(id, _)| visible != Some(**id))
            .filter_map(|(id, record)| {
                views.get(id).map(|view| ScenePane {
                    id: *id,
                    public_id: record.address.clone(),
                    inner_rect: Rect::default(),
                    terminal: &view.terminal,
                })
            })
            .collect::<Vec<_>>();
        let surface = surface.as_slice();
        let (scene, delivery) = if let Some(frozen) = &self.frozen {
            let limit = crate::kitty_graphics::surface::MAX_SURFACE_GRAPHICS_PLACEMENTS
                .saturating_sub(frozen.placement_count());
            let (mut scene, mut delivery, _) =
                crate::kitty_graphics::surface::collect_scene_with_limit(
                    surface,
                    &retained,
                    self.cell_size,
                    &self.delivery,
                    limit,
                );
            frozen.graphics(area, &mut scene, &mut delivery);
            (scene, delivery)
        } else {
            let (scene, delivery, _) = crate::kitty_graphics::surface::collect_scene(
                surface,
                &retained,
                self.cell_size,
                &self.delivery,
            );
            (scene, delivery)
        };
        self.delivery = delivery;
        scene
    }
}
