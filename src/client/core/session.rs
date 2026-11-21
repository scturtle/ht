use serde_json::{json, Value};
use std::collections::HashMap;

#[path = "events.rs"]
mod events;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

use super::{
    app_layout,
    model::{invalid, Model},
    transport::{CoreRelay, Frame, CLOSE, RESIZE},
    view::{self, Renderer, View},
    EndpointEvent,
};
use crate::layout::PaneId;
use crate::protocol::*;

#[cfg(test)]
#[path = "synchronized_tests.rs"]
mod synchronized_tests;
#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

pub(super) fn ensure_private_directory(path: &std::path::Path) -> io::Result<()> {
    match crate::platform::create_remote_private_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if std::fs::symlink_metadata(path)?.is_dir() {
                Ok(())
            } else {
                Err(invalid("Core directory must be a real directory"))
            }
        }
        Err(error) => Err(error),
    }
}

pub(super) fn client_preferences(
    home: &std::path::Path,
    client: Option<&str>,
) -> io::Result<Option<(PathBuf, File)>> {
    let Some(client) = client else {
        return Ok(None);
    };
    let clients = home.join("clients");
    ensure_private_directory(&clients)?;
    let home = clients.join(client);
    ensure_private_directory(&home)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(home.join("client.lock"))?;
    lock.try_lock().map_err(|e| io::Error::other(format!("Core presentation identity {client:?} is already attached; choose a different HERDR_CORE_CLIENT_ID: {e}")))?;
    Ok(Some((home, lock)))
}

pub(in crate::client) struct Session {
    pub core: CoreRelay,
    /// Launch identity/homes; a reconnect repeats the same Core home.
    options: Option<super::Options>,
    pub(super) model: Model,
    pub(super) shell: Vec<String>,
    scrollback: crate::config::AdvancedConfig,
    pub(super) views: HashMap<PaneId, View>,
    channels: HashMap<u32, PaneId>,
    pub(super) document_version: u64,
    exit_when_empty: bool,
    pub size: ClientSurfaceSize,
    pub cells: (u32, u32),
    renderer: Renderer,
    pub dirty: bool,
    pub snapshot_dirty: bool,
    synchronized_deadline: Option<Instant>,
    pub focused: bool,
    last_focus: Option<PaneId>,
    pub home: Option<PathBuf>,
    _lock: Option<File>,
    events: Vec<u8>,
    seq: u64,
    theme: crate::terminal::TerminalTheme,
    display: Value,
}

pub(super) fn configured_shell(config: &crate::config::Config) -> Vec<String> {
    let shell = config.terminal.default_shell.trim();
    if shell.is_empty() {
        Vec::new()
    } else {
        vec![shell.to_owned()]
    }
}

/// Core returns a finished screen as rows joined by `\n`. A terminal emulator
/// moves the cursor down on `\n` without returning to column 0, so replaying that
/// text verbatim staircases every row further right than the one above it. Feed
/// each row as its own line instead.
pub(super) fn replay_lines(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 8);
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(line.as_bytes());
    }
    out
}

impl Session {
    pub fn connect(
        options: super::Options,
        size: ClientSurfaceSize,
        cells: (u32, u32),
        config: &crate::config::Config,
    ) -> io::Result<Self> {
        if let Some(parent) = options.home.parent() {
            std::fs::create_dir_all(parent)?;
        }
        ensure_private_directory(&options.home)?;
        ensure_private_directory(&options.home.join("run"))?;
        let preferences = client_preferences(&options.home, options.client_id.as_deref())?;
        // Only an explicit external socket is attach-only on initial launch.
        // Default and isolated sessions let the Core relay connect or start.
        let attach_only = !options.isolate && options.socket.is_some();
        let (mut core, subscription) = Self::open_core(&options, attach_only)?;
        let shell = configured_shell(config);
        let (mut model, document_version) = app_layout::bootstrap(&mut core, &subscription)?;
        if let Some((home, _)) = &preferences {
            super::shared::restore_preferences(&home.join("app-preferences.json"), &mut model)?;
        }
        let (home, lock) = match preferences {
            Some((home, lock)) => (Some(home), Some(lock)),
            None => (None, None),
        };
        let mut renderer = Renderer::new()?;
        renderer.set_cell_size(cells);
        let mut session = Self {
            core,
            exit_when_empty: options.isolate && !model.records.is_empty(),
            options: Some(options),
            model,
            shell,
            scrollback: config.advanced.clone(),
            views: HashMap::new(),
            channels: HashMap::new(),
            document_version,
            size,
            cells,
            renderer,
            dirty: true,
            snapshot_dirty: true,
            synchronized_deadline: None,
            focused: true,
            last_focus: None,
            home,
            _lock: lock,
            events: Vec::new(),
            seq: Self::subscription_seq(&subscription)?,
            theme: Default::default(),
            display: json!({}),
        };
        session.sync_views()?;
        session.resize(size, cells.0, cells.1)?;
        session.save()?;
        Ok(session)
    }

    /// Reach a Core for this session's homes. Never touches client-local state,
    /// so a failed attempt leaves this session exactly as it was.
    fn open_core(options: &super::Options, existing: bool) -> io::Result<(CoreRelay, Value)> {
        let mut core = if existing {
            CoreRelay::connect_existing(
                &options.binary,
                &options.home,
                options.socket.as_deref(),
                options.isolate,
            )?
        } else {
            CoreRelay::connect(
                &options.binary,
                &options.home,
                options.socket.as_deref(),
                options.isolate,
            )?
        };
        let subscription = core.subscribe()?;
        Ok((core, subscription))
    }

    pub(super) fn is_closed(&self) -> bool {
        self.exit_when_empty && self.model.records.is_empty()
    }

    pub(super) fn seed_private_layout(&mut self) -> io::Result<()> {
        if self.document_version != 0
            || !self.options.as_ref().is_some_and(|options| options.isolate)
        {
            return Ok(());
        }
        self.seed_app_layout()?;
        self.sync_views()?;
        self.save()
    }

    pub(super) fn refresh_shared(&mut self) -> io::Result<bool> {
        let changed =
            app_layout::refresh(&mut self.core, &mut self.model, &mut self.document_version)?;
        self.snapshot_dirty |= changed;
        Ok(changed)
    }

    fn subscription_seq(subscription: &Value) -> io::Result<u64> {
        subscription["seq"]
            .as_u64()
            .ok_or_else(|| invalid("Core subscription lacks sequence"))
    }

    /// Reconnect this session to its Core and re-attach every pane.
    ///
    /// A recovery probe never starts a stopped runtime. Everything the Core owns
    /// is rebuilt from the new connection; everything the client owns (cell size,
    /// theme, active workspace/tab/focus, delivered images) is preserved. Queued
    /// input is the caller's business: it must be discarded, not replayed.
    pub fn resume(&mut self) -> io::Result<()> {
        let options = self
            .options
            .clone()
            .ok_or_else(|| io::Error::other("Core session cannot resume without launch options"))?;
        let (mut core, subscription) = Self::open_core(&options, true)?;
        let (mut model, document_version) = app_layout::bootstrap(&mut core, &subscription)?;
        // Presentation preferences are client-local, so they survive the Core
        // connection. `save` wrote them, and restoring them here is the same path
        // a fresh attach uses.
        if let Some(home) = &self.home {
            super::shared::restore_preferences(&home.join("app-preferences.json"), &mut model)?;
        }
        // Commit only after every fallible step has succeeded.
        self.core = core;
        self.model = model;
        self.document_version = document_version;
        self.seq = Self::subscription_seq(&subscription)?;
        // Channels belong to the dead connection; every pane re-attaches with an
        // authoritative grid, exactly like a first attach.
        self.views.clear();
        self.synchronized_deadline = None;
        self.channels.clear();
        self.events.clear();
        self.renderer.resume_connection();
        self.last_focus = None;
        self.dirty = true;
        self.snapshot_dirty = true;
        self.sync_views()?;
        self.resize(self.size, self.cells.0, self.cells.1)?;
        self.save()?;
        Ok(())
    }

    pub fn save(&self) -> io::Result<()> {
        let Some(home) = &self.home else {
            return Ok(());
        };
        // Organization is saved only by Core CAS. This file contains presentation preferences.
        let focus = self
            .model
            .all_layouts()
            .filter_map(|(key, layout)| {
                self.model
                    .records
                    .get(&layout.focused())
                    .map(|r| (key.to_owned(), r.address.clone()))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let preferences = json!({"active_workspace":self.model.active_workspace,"active_tabs":self.model.active_tabs,"focus":focus});
        let path = home.join("app-preferences.json");
        let temporary = path.with_extension("pending");
        let mut file = crate::platform::create_private_state_file(&temporary)?;
        file.write_all(&serde_json::to_vec(&preferences)?)?;
        file.sync_all()?;
        drop(file);
        crate::platform::replace_file(&temporary, &path)
    }

    pub(super) fn sync_views(&mut self) -> io::Result<()> {
        self.exit_when_empty |= !self.model.records.is_empty()
            && self.options.as_ref().is_some_and(|options| options.isolate);
        let gone = self
            .views
            .keys()
            .filter(|id| !self.model.records.contains_key(id))
            .copied()
            .collect::<Vec<_>>();
        for id in gone {
            self.close_view(id)?;
        }
        let added = self
            .model
            .records
            .keys()
            .filter(|id| !self.views.contains_key(id))
            .copied()
            .collect::<Vec<_>>();
        let focused = self.model.focused();
        for id in added {
            self.attach(id, focused == Some(id))?;
        }
        self.resize(self.size, self.cells.0, self.cells.1)?;
        self.focus()
    }

    pub fn attach(&mut self, id: PaneId, visible: bool) -> io::Result<()> {
        // Passive attaches follow Core's grid without changing it. When
        // revealed, every pane claims the same full client area.
        let (cols, rows) = self.desired_grid();
        let address = &self
            .model
            .records
            .get(&id)
            .ok_or_else(|| invalid("pane missing"))?
            .address;
        if self.model.records[&id].facts["state"] != "live"
            || self.model.records[&id].facts["pid"].as_u64().is_none()
        {
            let mut view = View::with_scrollback(cols, rows, 0, &self.scrollback)?;
            view.closed = true;
            view.visible = visible;
            view.terminal.apply_host_terminal_theme(self.theme);
            let result = self.core.request(
                "terminal.read",
                json!({"terminal":address,"lines":self.scrollback.scrollback_limit_lines}),
            );
            if let Ok(result) = result {
                if let Some(text) = result["text"].as_str() {
                    view.process_output(id, &replay_lines(text));
                }
            }
            self.views.insert(id, view);
            self.update_synchronized_deadline();
            return Ok(());
        }
        let result=self.core.request("terminal.attach_ordered",json!({"terminal":address,"cols":cols,"rows":rows,"history":self.scrollback.scrollback_limit_lines,"resize_notifications":true,"resumed":!visible,"display":self.display}))?;
        let channel = result["channel"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| invalid("missing attach channel"))?;
        if result["automatic_input"] != true || result["resize_notifications"] != true {
            return Err(invalid(
                "Core ordered attach did not confirm geometry/automatic-input contract",
            ));
        }
        if channel == 0
            || Some(channel) == self.core.events_channel
            || self.channels.contains_key(&channel)
        {
            return Err(invalid("Core reused an active/reserved attach channel"));
        }
        let mut view = View::with_scrollback(cols, rows, channel, &self.scrollback)?;
        view.visible = visible;
        if let Some(old) = self.views.get(&id) {
            view.revision = old.revision.saturating_add(2);
        }
        view.terminal.apply_host_terminal_theme(self.theme);
        if let Some(old) = self.views.insert(id, view) {
            self.channels.remove(&old.channel);
            if !old.closed {
                self.core.send(old.channel, CLOSE, &[])?;
            }
        }
        self.channels.insert(channel, id);
        self.update_synchronized_deadline();
        if self.last_focus == Some(id) {
            self.last_focus = None;
        }
        self.focus()?;
        Ok(())
    }

    pub(super) fn close_view(&mut self, id: PaneId) -> io::Result<()> {
        if let Some(view) = self.views.remove(&id) {
            self.update_synchronized_deadline();
            self.channels.remove(&view.channel);
            if !view.closed {
                self.core.send(view.channel, CLOSE, &[])?;
            }
        }
        Ok(())
    }

    fn desired_grid(&self) -> (u16, u16) {
        (
            self.size.cols.max(crate::pane::MIN_PANE_COLS),
            self.size.rows.max(crate::pane::MIN_PANE_ROWS),
        )
    }

    pub fn resize(&mut self, size: ClientSurfaceSize, width: u32, height: u32) -> io::Result<()> {
        self.size = size;
        self.cells = (width, height);
        self.renderer.set_cell_size(self.cells);
        if width > 0 && height > 0 {
            self.display["cell"] = json!([width, height]);
        }
        // Unmapped views stay attached and follow Core without sending RESIZE.
        let focused = self.model.focused();
        let (cols, rows) = self.desired_grid();
        for (id, view) in &mut self.views {
            let Some((grid_changed, display_changed)) =
                view.request_visible_size(focused == Some(*id), (cols, rows, width, height))
            else {
                continue;
            };
            if view.closed {
                view.authoritative_size((cols, rows, width, height));
            } else {
                if grid_changed {
                    self.core
                        .send(view.channel, RESIZE, &view.resize_request())?;
                }
                if display_changed {
                    self.core
                        .send(view.channel, 5, &serde_json::to_vec(&self.display)?)?;
                    for response in view.display_cells(width, height) {
                        self.core.send(view.channel, 6, &response)?;
                    }
                }
            }
        }
        self.dirty = true;
        Ok(())
    }

    pub fn focus(&mut self) -> io::Result<()> {
        let target = self
            .focused
            .then(|| self.model.focused())
            .flatten()
            .filter(|id| self.views.get(id).is_some_and(|v| !v.closed));
        if self.last_focus != target {
            if let Some(view) = self
                .last_focus
                .and_then(|id| self.views.get(&id))
                .filter(|v| !v.closed)
            {
                self.core.send(view.channel, 4, &[0])?;
            }
            if let Some(view) = target.and_then(|id| self.views.get(&id)) {
                self.core.send(view.channel, 4, &[1])?;
                // `latest` policy: taking focus claims this pane's geometry, so a
                // client that was reading another client's grid (or a stale one)
                // reclaims it instead of waiting for a layout change.
                let request = view.resize_request();
                self.core.send(view.channel, RESIZE, &request)?;
            }
            self.last_focus = target;
        }
        Ok(())
    }

    pub fn theme(&mut self, update: ClientHostThemeUpdate) -> io::Result<()> {
        match update {
            ClientHostThemeUpdate::DefaultColor { kind, color } => {
                let (key, target) = match kind {
                    ClientHostDefaultColorKind::Foreground => ("fg", &mut self.theme.foreground),
                    ClientHostDefaultColorKind::Background => ("bg", &mut self.theme.background),
                };
                *target = Some(color.into());
                self.display[key] =
                    json!(format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b));
            }
            ClientHostThemeUpdate::PaletteColors(colors) => {
                for (index, color) in colors {
                    self.theme = self.theme.with_palette_color(index, color.into());
                }
            }
        }
        for view in self.views.values() {
            view.terminal.apply_host_terminal_theme(self.theme);
            if !view.closed {
                self.core
                    .send(view.channel, 5, &serde_json::to_vec(&self.display)?)?;
            }
        }
        self.dirty = true;
        Ok(())
    }

    fn update_synchronized_deadline(&mut self) {
        // Only synchronization transitions/lifecycle changes rescan these scalar
        // deadlines, never terminal state or screen contents on every DATA frame.
        self.synchronized_deadline = self
            .views
            .values()
            .filter_map(View::synchronized_deadline)
            .min();
    }

    pub fn wait_deadline(&self) -> Option<Instant> {
        self.core
            .write_deadline()
            .into_iter()
            .chain(self.synchronized_deadline)
            .min()
    }

    fn expire_synchronized(&mut self, now: Instant) {
        if self.synchronized_deadline.is_none_or(|until| now < until) {
            return;
        }
        for view in self.views.values_mut() {
            if view.expire_synchronized(now) {
                self.dirty |= view.visible && !view.closed;
            }
        }
        self.update_synchronized_deadline();
    }

    pub fn publish(&mut self) -> io::Result<Vec<EndpointEvent>> {
        if self.synchronized_deadline.is_some() {
            self.expire_synchronized(Instant::now());
        }
        if !self.dirty && !self.snapshot_dirty {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let snapshot_changed = self.snapshot_dirty;
        if self.snapshot_dirty {
            out.push(EndpointEvent::Snapshot(Box::new(self.model.snapshot())));
            self.snapshot_dirty = false;
        }
        if self.dirty {
            let surface = self.renderer.compose(&self.model, &self.views, self.size)?;
            out.push(EndpointEvent::PaneSurface(surface));
            let view = self
                .model
                .focused()
                .and_then(|id| self.views.get(&id))
                .filter(|view| !view.closed);
            out.push(EndpointEvent::KeyboardReportAll {
                enabled: view.is_some_and(|view| {
                    view.terminal
                        .keyboard_protocol(crate::input::KeyboardProtocol::Legacy)
                        .reports_all_keys()
                }),
            });
            out.push(EndpointEvent::MouseCapture {
                enabled: view.is_some_and(|view| view.terminal.mouse_reporting_enabled()),
                sgr_pixels: view.is_some_and(|view| view.terminal.sgr_pixel_mouse_enabled()),
            });
            // Keep projecting until every budgeted image asset is delivered,
            // even if no new Core output or input arrives between projections.
            self.dirty = self.renderer.graphics_pending();
        } else if snapshot_changed {
            if let Some(surface) = self.renderer.retain_projection(&self.model) {
                out.push(EndpointEvent::PaneSurface(surface));
            }
        }
        Ok(out)
    }
}
