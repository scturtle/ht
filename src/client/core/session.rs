use ratatui::layout::Rect;
use serde_json::{json, Value};
use std::collections::HashMap;

#[path = "events.rs"]
mod events;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use super::{
    model::{invalid, Model},
    transport::{Core, Frame, CLOSE, RESIZE},
    view::{self, Renderer, View},
};
use crate::layout::PaneId;
use crate::protocol::*;

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
    pub core: Core,
    /// Launch identity/homes; a reconnect repeats the same Core home.
    options: Option<super::Options>,
    pub(super) model: Model,
    pub(super) shell: Vec<String>,
    pub(super) views: HashMap<PaneId, View>,
    channels: HashMap<u32, PaneId>,
    pub(super) document_version: u64,
    pub size: ClientSurfaceSize,
    pub cells: (u32, u32),
    pub(super) ui: view::ChromeConfig,
    renderer: Renderer,
    pub dirty: bool,
    pub snapshot_dirty: bool,
    pub focused: bool,
    last_focus: Option<PaneId>,
    pub home: Option<PathBuf>,
    _lock: Option<File>,
    events: Vec<u8>,
    seq: u64,
    theme: crate::terminal_theme::TerminalTheme,
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
        let (mut core, subscription) = Self::open_core(&options, false)?;
        let shell = configured_shell(config);
        let (mut model, document_version) =
            super::shared::bootstrap(&mut core, &subscription, size, &options.home, &shell)?;
        if let Some((home, _)) = &preferences {
            super::shared::restore_preferences(home, &mut model)?;
        }
        let (home, lock) = match preferences {
            Some((home, lock)) => (Some(home), Some(lock)),
            None => (None, None),
        };
        let mut renderer = Renderer::new()?;
        renderer.set_cell_size(cells);
        let mut session = Self {
            core,
            options: Some(options),
            model,
            shell,
            views: HashMap::new(),
            channels: HashMap::new(),
            document_version,
            size,
            cells,
            ui: view::ChromeConfig::from_config(config),
            renderer,
            dirty: true,
            snapshot_dirty: true,
            focused: true,
            last_focus: None,
            home,
            _lock: lock,
            events: Vec::new(),
            seq: Self::subscription_seq(&subscription)?,
            theme: Default::default(),
            display: json!({}),
        };
        session.retire_completed_programs()?;
        session.sync_views()?;
        session.recover_retirements(&subscription);
        session.resize(size, cells.0, cells.1)?;
        session.save()?;
        Ok(session)
    }

    /// Reach a Core for this session's homes. Never touches client-local state,
    /// so a failed attempt leaves this session exactly as it was.
    fn open_core(options: &super::Options, existing: bool) -> io::Result<(Core, Value)> {
        let mut core = if existing {
            Core::connect_existing(&options.binary, &options.home)?
        } else {
            Core::connect(&options.binary, &options.home)?
        };
        let subscription = core.subscribe()?;
        Ok((core, subscription))
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
        let (mut model, document_version) = super::shared::bootstrap(
            &mut core,
            &subscription,
            self.size,
            &options.home,
            &self.shell,
        )?;
        // Presentation preferences are client-local, so they survive the Core
        // connection. `save` wrote them, and restoring them here is the same path
        // a fresh attach uses.
        if let Some(home) = &self.home {
            super::shared::restore_preferences(home, &mut model)?;
        }
        // Commit only after every fallible step has succeeded.
        self.core = core;
        self.model = model;
        self.document_version = document_version;
        self.seq = Self::subscription_seq(&subscription)?;
        // Channels belong to the dead connection; every pane re-attaches with an
        // authoritative grid, exactly like a first attach.
        self.views.clear();
        self.channels.clear();
        self.events.clear();
        self.renderer.resume_connection();
        self.last_focus = None;
        self.dirty = true;
        self.snapshot_dirty = true;
        self.retire_completed_programs()?;
        self.sync_views()?;
        self.recover_retirements(&subscription);
        self.resize(self.size, self.cells.0, self.cells.1)?;
        self.save()?;
        Ok(())
    }

    /// The socket a recovery probe reaches. Its absence is the observable
    /// difference between a restart (which recreates it) and an explicit stop.
    pub fn socket_path(&self) -> PathBuf {
        self.options
            .as_ref()
            .map(|options| options.home.join("core.sock"))
            .unwrap_or_default()
    }

    /// Recover close intents Core committed but did not finish.
    fn recover_retirements(&mut self, subscription: &Value) {
        let retiring = subscription["terminals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["state"] == "live" && t["labels"]["herdr"] == "tui")
            .filter_map(|t| t["address"].as_str())
            .filter(|a| self.model.organization.retiring.contains(*a))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if let Err(error) = self.finish_retirements(&retiring) {
            tracing::warn!(%error, "recovering committed Core close intents");
        }
    }

    pub fn area(&self) -> Rect {
        Rect::new(0, 0, self.size.cols.max(1), self.size.rows.max(1))
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
        let zoomed = self
            .model
            .zoomed_tabs
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        let preferences = json!({"active_workspace":self.model.active_workspace,"active_tabs":self.model.active_tabs,"focus":focus,"zoomed_tabs":zoomed});
        let temporary = home.join("preferences.pending");
        let mut file = crate::platform::create_private_state_file(&temporary)?;
        file.write_all(&serde_json::to_vec(&preferences)?)?;
        file.sync_all()?;
        drop(file);
        crate::platform::replace_file(&temporary, &home.join("preferences.json"))
    }

    pub(super) fn sync_views(&mut self) -> io::Result<()> {
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
        for id in added {
            self.attach(id)?;
        }
        self.resize(self.size, self.cells.0, self.cells.1)?;
        self.focus()
    }

    pub fn attach(&mut self, id: PaneId) -> io::Result<()> {
        // First paint is serialized for the requested grid. Never replay it into
        // a full-canvas terminal and immediately shrink it before consumption.
        let tab = self
            .model
            .context(id)
            .map(|(_, t)| t.id.as_str())
            .ok_or_else(|| invalid("Core pane has no tab"))?;
        let layout = self
            .model
            .layouts
            .get(tab)
            .ok_or_else(|| invalid("Core tab has no layout"))?;
        let zoomed = self.model.zoomed_tabs.contains(tab);
        let visible = self.model.active_tab() == Some(tab) && (!zoomed || layout.focused() == id);
        let info = if zoomed && layout.focused() == id {
            view::zoomed_info(layout, self.area(), &self.ui)
        } else {
            view::layout_infos(layout, self.area(), &self.ui)
                .into_iter()
                .find(|p| p.id == id)
                .ok_or_else(|| invalid("Core pane geometry missing"))?
        };
        let cols = info.inner_rect.width.max(crate::pane::MIN_PANE_COLS);
        let rows = info.inner_rect.height.max(crate::pane::MIN_PANE_ROWS);
        let address = &self
            .model
            .records
            .get(&id)
            .ok_or_else(|| invalid("pane missing"))?
            .address;
        if self.model.records[&id].facts["state"] != "live"
            || self.model.records[&id].facts["pid"].as_u64().is_none()
        {
            let mut view = View::new(cols, rows, 0)?;
            view.closed = true;
            view.visible = visible;
            view.terminal.apply_host_terminal_theme(self.theme);
            let result = self.core.request(
                "terminal.read",
                json!({"terminal":address,"format":"ansi","lines":1000}),
            );
            if let Ok(result) = result {
                if let Some(text) = result["text"].as_str() {
                    view.process_output(id, text.as_bytes());
                }
            }
            self.views.insert(id, view);
            return Ok(());
        }
        let result=self.core.request("terminal.attach_ordered",json!({"terminal":address,"cols":cols,"rows":rows,"history":1000,"resize_notifications":true,"resumed":!visible,"display":self.display}))?;
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
        let mut view = View::new(cols, rows, channel)?;
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
        if self.last_focus == Some(id) {
            self.last_focus = None;
        }
        self.focus()?;
        Ok(())
    }

    pub(super) fn close_view(&mut self, id: PaneId) -> io::Result<()> {
        if let Some(view) = self.views.remove(&id) {
            self.channels.remove(&view.channel);
            if !view.closed {
                self.core.send(view.channel, CLOSE, &[])?;
            }
        }
        Ok(())
    }

    pub fn resize(&mut self, size: ClientSurfaceSize, width: u32, height: u32) -> io::Result<()> {
        self.size = size;
        self.cells = (width, height);
        self.renderer.set_cell_size(self.cells);
        if width > 0 && height > 0 {
            self.display["cell"] = json!([width, height]);
        }
        let infos = self
            .model
            .all_layouts()
            .flat_map(|(tab, layout)| {
                let visible = self.model.active_tab() == Some(tab);
                let zoomed = self.model.zoomed_tabs.contains(tab);
                let zoom_info = zoomed.then(|| view::zoomed_info(layout, self.area(), &self.ui));
                view::layout_infos(layout, self.area(), &self.ui)
                    .into_iter()
                    .map(move |info| {
                        let visible = visible && (!zoomed || info.is_focused);
                        let info = if info.is_focused {
                            zoom_info.clone().unwrap_or(info)
                        } else {
                            info
                        };
                        (visible, info)
                    })
            })
            .collect::<Vec<_>>();
        for (visible, info) in infos {
            let Some(view) = self.views.get_mut(&info.id) else {
                continue;
            };
            let cols = info.inner_rect.width.max(crate::pane::MIN_PANE_COLS);
            let rows = info.inner_rect.height.max(crate::pane::MIN_PANE_ROWS);
            let Some((grid_changed, display_changed)) = view.request_visible_size(
                visible && info.inner_rect.width > 0 && info.inner_rect.height > 0,
                (cols, rows, width, height),
            ) else {
                continue;
            };
            if view.closed {
                view.authoritative_size((cols, rows, width, height));
            } else {
                if grid_changed {
                    self.core.send(
                        view.channel,
                        RESIZE,
                        &[cols.to_le_bytes(), rows.to_le_bytes()].concat(),
                    )?;
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
        use crate::terminal_theme::{DefaultColorKind, HostAppearance};
        match update {
            ClientHostThemeUpdate::DefaultColor { kind, color } => {
                let (key, kind) = match kind {
                    ClientHostDefaultColorKind::Foreground => ("fg", DefaultColorKind::Foreground),
                    ClientHostDefaultColorKind::Background => ("bg", DefaultColorKind::Background),
                };
                self.theme = self.theme.with_color(kind, color.into());
                self.display[key] =
                    json!(format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b));
            }
            ClientHostThemeUpdate::PaletteColors(colors) => {
                for (index, color) in colors {
                    self.theme = self.theme.with_palette_color(index, color.into());
                }
            }
            ClientHostThemeUpdate::Appearance(a) => {
                for view in self.views.values() {
                    view.terminal.apply_host_terminal_appearance(match a {
                        ClientHostAppearance::Dark => Some(HostAppearance::Dark),
                        ClientHostAppearance::Light => Some(HostAppearance::Light),
                    });
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

    pub fn publish(&mut self) -> io::Result<Vec<ServerMessage>> {
        if !self.dirty && !self.snapshot_dirty {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let snapshot_changed = self.snapshot_dirty;
        if self.snapshot_dirty {
            out.push(crate::protocol::endpoint::snapshot_message(
                &self.model.snapshot(),
            )?);
            self.snapshot_dirty = false;
        }
        if self.dirty {
            let surface = self
                .renderer
                .compose(&self.model, &self.views, self.size, &self.ui)?;
            out.push(ServerMessage::PaneSurface(surface));
            // Image bytes beyond the frame budget are delivered by the next
            // projection, as the retired server did with `DeliveryCache::pending`.
            self.dirty |= self.renderer.graphics_pending();
            let view = self
                .model
                .focused()
                .and_then(|id| self.views.get(&id))
                .filter(|view| !view.closed);
            out.push(ServerMessage::ClientShellKeyboardReportAll {
                enabled: view.is_some_and(|view| {
                    view.terminal
                        .keyboard_protocol(crate::input::KeyboardProtocol::Legacy)
                        .reports_all_keys()
                }),
            });
            out.push(ServerMessage::MouseCapture {
                enabled: view.is_some_and(|view| view.terminal.mouse_reporting_enabled()),
                sgr_pixels: view.is_some_and(|view| view.terminal.sgr_pixel_mouse_enabled()),
            });
            self.dirty = false;
        } else if snapshot_changed {
            if let Some(surface) = self.renderer.retain_projection(&self.model) {
                out.push(ServerMessage::PaneSurface(surface));
            }
        }
        Ok(out)
    }
}
