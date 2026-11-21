use super::*;

/// State tracking for the thin client.
pub(super) struct ClientState {
    /// Stateful semantic-frame encoder used when the server sends FrameData.
    pub(super) blit_encoder: render_ansi::BlitEncoder,
    pub(super) image_files: image_files::FileTransport,
    pub(super) mouse_capture_active: bool,
    /// Lifetime count of color replies requested by this client, observed by the reader.
    pub(super) host_color_replies_expected: Arc<AtomicU64>,
    pub(super) endpoint_mouse_capture_requested: bool,
    pub(super) endpoint_sgr_pixels_requested: bool,
    pub(super) shell_mouse_capture_preference: bool,
    pub(super) pane_keyboard_report_all: bool,
    pub(super) keyboard_report_all_active: bool,
    pub(super) reported_size: (u16, u16),
    pub(super) reported_cell_size: (u32, u32),
    pub(super) kitty_graphics_enabled: bool,
    pub(super) pixel_geometry_exact: bool,
    /// The host's answer to the SGR pixel mouse probe; None when unknown.
    pub(super) host_sgr_pixel_mouse: Option<bool>,
    pub(super) repaint_pending: bool,
    pub(super) detached_process_children: Vec<std::process::Child>,
    pub(super) shell: shell::ClientShellState,
}

impl ClientState {
    pub(super) fn requested_host_mouse_capture(&self) -> bool {
        // TUI chrome needs mouse events even when the focused program does not.
        let local_preference = self.shell_mouse_capture_preference;
        super::effective_mouse_capture(self.endpoint_mouse_capture_requested, local_preference)
    }

    pub(super) fn pixel_mouse_available(&self) -> bool {
        self.pixel_geometry_exact && self.host_sgr_pixel_mouse == Some(true)
    }

    #[cfg(test)]
    pub(super) fn test_new() -> Self {
        Self {
            blit_encoder: render_ansi::BlitEncoder::new(),
            image_files: image_files::FileTransport::default(),
            mouse_capture_active: false,
            host_color_replies_expected: Default::default(),
            endpoint_mouse_capture_requested: false,
            endpoint_sgr_pixels_requested: false,
            shell_mouse_capture_preference: false,
            pane_keyboard_report_all: false,
            keyboard_report_all_active: false,
            reported_size: (100, 30),
            reported_cell_size: (0, 0),
            kitty_graphics_enabled: false,
            pixel_geometry_exact: false,
            host_sgr_pixel_mouse: None,
            repaint_pending: false,
            detached_process_children: Vec::new(),
            shell: shell::ClientShellState::new(shell::ClientShellConfig::from_config(
                &crate::config::Config::default(),
            )),
        }
    }

    pub(super) fn request_repaint(&mut self) {
        self.repaint_pending = true;
    }

    pub(super) fn present_graphics(&mut self, graphics: &[u8]) {
        if graphics.is_empty() || !self.kitty_graphics_enabled {
            return;
        }
        let mut stdout = io::stdout();
        let _ = write_encoded_frame_with_graphics(&mut stdout, &[], graphics);
        let _ = stdout.flush();
    }

    pub(super) fn present_frame(&mut self, frame_data: impl Into<frame_output::ComposedFrame>) {
        let _ = self.try_present_frame_to(&mut io::stdout(), frame_data);
    }

    fn write_composed_output(
        &mut self,
        writer: &mut impl io::Write,
        encoded: &[u8],
        graphics: crate::kitty_graphics::GraphicsOutput,
    ) -> io::Result<()> {
        if self.kitty_graphics_enabled {
            frame_output::write_composed_frame(
                writer.by_ref(),
                encoded,
                &graphics,
                &mut self.image_files,
            )?;
        } else {
            writer.write_all(encoded)?;
        }
        writer.flush()
    }

    /// Commit the blit baseline only after all terminal output is written successfully.
    pub(super) fn try_present_frame_to(
        &mut self,
        writer: &mut impl io::Write,
        frame_data: impl Into<frame_output::ComposedFrame>,
    ) -> bool {
        let frame_output::ComposedFrame {
            frame: mut frame_data,
            graphics,
        } = frame_data.into();
        // With capture enabled, host OSC 8 can intercept Ctrl-click before plugins.
        // Keep semantic links internally and restore native links when capture is off.
        if self.shell_mouse_capture_preference {
            frame_data.hyperlinks.clear();
        }
        let encoded = self.blit_encoder.encode(&frame_data, self.repaint_pending);
        if let Err(error) = self.write_composed_output(writer, &encoded.bytes, graphics) {
            tracing::warn!(%error, "failed to present client frame");
            self.repaint_pending = true;
            return false;
        }
        self.blit_encoder.commit(frame_data, encoded);
        self.repaint_pending = false;
        true
    }
}
