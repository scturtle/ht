//! Host theme baselines and child default-color ownership share one state,
//! protected by the existing terminal-core lock.

use tracing::debug;

use super::GhosttyPaneTerminal;
use crate::pane::osc::DefaultColorQuery;

#[derive(Default)]
pub(super) struct ThemeState {
    pub(super) initial_foreground: Option<crate::ghostty::RgbColor>,
    pub(super) initial_background: Option<crate::ghostty::RgbColor>,
    pub(super) host: crate::terminal::TerminalTheme,
    pub(super) transient_owner_pgid: Option<u32>,
    pub(super) child_foreground_changed: bool,
    pub(super) child_background_changed: bool,
}

impl ThemeState {
    pub(super) fn mark_child_default_color_changed(
        &mut self,
        query: DefaultColorQuery,
        changed: bool,
    ) {
        match query {
            DefaultColorQuery::Foreground => self.child_foreground_changed = changed,
            DefaultColorQuery::Background => self.child_background_changed = changed,
            DefaultColorQuery::Cursor => {}
        }
    }

    pub(super) fn apply_cached_host_default_color(
        &self,
        terminal: &mut crate::ghostty::Terminal,
        query: DefaultColorQuery,
    ) {
        write_host_terminal_theme_selective(
            terminal,
            self.host,
            matches!(query, DefaultColorQuery::Foreground),
            matches!(query, DefaultColorQuery::Background),
        );
    }
}

impl GhosttyPaneTerminal {
    pub fn apply_host_terminal_theme(&self, theme: crate::terminal::TerminalTheme) {
        if let Ok(mut core) = self.core.lock() {
            let foreground_unowned = !core.theme.child_foreground_changed;
            let background_unowned = !core.theme.child_background_changed;
            core.theme.host = theme;
            if foreground_unowned && background_unowned {
                core.theme.transient_owner_pgid = None;
            }

            let mut palette = crate::ghostty::default_palette();
            for (index, color) in theme.palette.iter().enumerate() {
                if let Some(color) = color {
                    palette[index] = crate::ghostty::RgbColor {
                        r: color.r,
                        g: color.g,
                        b: color.b,
                    };
                }
            }
            if let Err(err) = core.terminal.set_default_palette(&palette) {
                debug!(err = %err, "failed to apply host terminal palette");
            }

            write_host_terminal_theme_selective(
                &mut core.terminal,
                theme,
                foreground_unowned,
                background_unowned,
            );
        }
    }
}

fn write_host_terminal_theme_selective(
    terminal: &mut crate::ghostty::Terminal,
    theme: crate::terminal::TerminalTheme,
    foreground: bool,
    background: bool,
) {
    if foreground {
        write_host_default_color(
            terminal,
            crate::terminal::DefaultColorKind::Foreground,
            theme.foreground,
        );
    }
    if background {
        write_host_default_color(
            terminal,
            crate::terminal::DefaultColorKind::Background,
            theme.background,
        );
    }
}

fn write_host_default_color(
    terminal: &mut crate::ghostty::Terminal,
    kind: crate::terminal::DefaultColorKind,
    color: Option<crate::terminal::RgbColor>,
) {
    let sequence = if let Some(color) = color {
        crate::terminal::osc_set_default_color_sequence(kind, color)
    } else {
        crate::terminal::osc_reset_default_color_sequence(kind).to_string()
    };
    terminal.write(sequence.as_bytes());
}
