use ghostty_vt as ghostty;
use ghostty_vt::pane_graphics_files;
use std::io;
use std::path::PathBuf;
mod actions;
mod build_info;
mod cli;
mod client;
// Historical configuration keys and projections remain parseable even when Core
// does not implement the corresponding feature; no runtime is constructed here.
#[allow(dead_code)]
mod config;
mod copy_mode;
mod input;
mod kitty_graphics;
mod layout;
mod logging;
#[cfg(test)]
mod noninteractive_process;
mod pane;
mod platform;
// Shared semantic messages and presentation data, not a legacy socket codec.
#[allow(dead_code)]
mod protocol;
mod raw_input;
mod render_prof;
mod selection;
mod session;
mod terminal_effects;
mod terminal_modes;
mod terminal_notify;
mod terminal_theme;
mod ui;

const DEFAULT_CONFIG: &str = r##"# herdr configuration
# Place this file at ~/.config/herdr/config.toml

# UI colors follow the host terminal. Optional single-layer overrides:
# Accepts hex (#rrggbb), named ANSI colors, rgb(r,g,b), or "reset".
# [theme.custom]
# panel_bg = "reset"
# accent = "#f5c2e7"
# red = "#ff6188"
# green = "#a6e3a1"

[terminal]
# Executable used for new interactive panes.
# Empty means $SHELL, then /bin/sh.
# default_shell = ""

[keys]
# Prefix key to enter prefix mode (default: "ctrl+b")
# Examples: "ctrl+b", "f12", "esc", "-"
# Action bindings use explicit syntax: "prefix+n" requires the prefix;
# "ctrl+alt+n" is a direct terminal-mode shortcut.
# Accepted key syntax: plain keys, ctrl/shift/alt/cmd/super modifiers, and special keys like enter/tab/esc/left/right/up/down.
# Named punctuation such as minus, comma, ampersand, plus, and backtick is also accepted.
# Most reliable direct bindings are ctrl+letter, function keys, and explicit modified chords.
# alt+..., cmd/super, and punctuation-with-modifiers may depend on your terminal/tmux setup.
# prefix = "ctrl+b"

# Prefix-mode actions
# help = "prefix+?"
# detach = "prefix+q"
# reload_config = "prefix+shift+r"
# open_notification_target = "prefix+o"
# workspace_picker = "prefix+w"
# goto = "prefix+g"
# new_workspace = "prefix+shift+n"
# rename_workspace = "prefix+shift+w"
# close_workspace = "prefix+shift+d"
# previous_workspace = "" # optional, unset by default
# next_workspace = ""     # optional, unset by default
# previous_agent = ""     # optional, unset by default
# next_agent = ""         # optional, unset by default
# focus_agent = ""        # optional indexed binding, e.g. "prefix+alt+1..9"
# remote_image_paste = "ctrl+v" # only active in herdr --remote; empty disables raw-key image paste
# new_tab = "prefix+c"
# rename_tab = "prefix+shift+t"
# previous_tab = "prefix+p"
# next_tab = "prefix+n"
# move_tab_previous = ""   # optional, e.g. "alt+shift+left" moves the tab toward the front
# move_tab_next = ""       # optional, e.g. "alt+shift+right" moves the tab toward the back
# switch_tab = "prefix+1..9"
# switch_workspace = ""   # optional indexed binding, e.g. "prefix+shift+1..9"
# close_tab = "prefix+shift+x"
# rename_pane = "prefix+shift+p"
# edit_scrollback = "prefix+e"
# clear_pane = ""                  # unbound; e.g. "prefix+ctrl+k"
# focus_pane_left = "prefix+h"
# focus_pane_down = "prefix+j"
# focus_pane_up = "prefix+k"
# focus_pane_right = "prefix+l"
# cycle_pane_next = "prefix+tab"
# cycle_pane_previous = "prefix+shift+tab"
# last_pane = ""          # optional, unset by default; bind e.g. "prefix+tab" for global back-and-forth
# split_vertical = "prefix+v"
# split_horizontal = "prefix+minus"
# close_pane = "prefix+x"
# zoom = "prefix+z"       # legacy alias: fullscreen
# resize_mode = "prefix+r"
# resize_pane_left = ""   # optional, e.g. "ctrl+shift+alt+left" resizes without entering resize mode
# resize_pane_down = ""   # optional, e.g. "ctrl+shift+alt+down"
# resize_pane_up = ""     # optional, e.g. "ctrl+shift+alt+up"
# resize_pane_right = ""  # optional, e.g. "ctrl+shift+alt+right"

# Navigate-mode movement. These local shortcuts win while navigate mode is open.
# They are independent from focus_pane_*. Do not include prefix+, esc, enter, tab, or 1..9 here.
# navigate_workspace_up = "up"
# navigate_workspace_down = "down"
# navigate_pane_left = "h"      # left arrow always focuses the pane to the left
# navigate_pane_down = "j"
# navigate_pane_up = "k"
# navigate_pane_right = "l"     # right arrow always focuses the pane to the right

# Legacy indexed shortcut config is still parsed for compatibility.
# Prefer switch_tab, switch_workspace, and focus_agent for new configs.
# [keys.indexed]
# tabs = ""       # e.g. "ctrl" makes ctrl+1..9 switch tabs directly
# workspaces = "" # e.g. "ctrl+shift" makes ctrl+shift+1..9 switch workspaces directly
# agents = ""     # e.g. "alt" makes alt+1..9 focus agent rows directly

[ui]
# Mouse capture and automatic selection copying are always enabled.
# The cursor is native. Creation and closing are immediate, without prompts.
# Panes share inner dividers only: no outer frame, gaps, or scrollbars.
# The desktop tab bar stays visible even with a single tab.

# Terminal width at or below which Herdr uses the mobile single-column layout.
# Increase this for foldables, tablets, or wide phone terminals.
# mobile_width_threshold = 64

# Optional modifier that forwards right-click hold/drag gestures to pane apps instead of opening Herdr's pane menu.
# Empty/off disables this. Shift is intentionally unsupported because terminals commonly reserve Shift+mouse.
# right_click_passthrough_modifier = ""

# Force a full redraw when the outer terminal regains focus.
# Set false to reduce visible flashing when switching back to Herdr.
# Trade-off: rare host terminal surface corruption may persist until the next full redraw.
# redraw_on_focus_gained = true

# Pane scrollback lines to scroll per mouse wheel notch.
# mouse_scroll_lines = 3

# Desktop tab row placement: "top" or "bottom".
# tab_bar_position = "top"

# Agent navigation ordering: "spaces" (grouped by space) or "priority" (attention queue).
# "workspaces" is accepted as an alias for "spaces".
# agent_panel_sort = "spaces"

# Agent status indicators: "dots" preserves the compact color marks; "symbols" uses
# distinct static glyphs for blocked, working, done, idle, and unknown states.
# status_indicators = "dots"

# Background notification popup delivery
[ui.toast]
# off = disable pop-up notifications
# herdr = show in-app toasts
# terminal = ask the outer terminal to show a desktop notification
# system = ask the OS notification service directly
# delivery = "off"
# delay_seconds = 1

[ui.toast.herdr]
# position = "bottom-right"

[ui.toast.clipboard]
# enabled = true
# position = "bottom-center"

[advanced]
# Physical scrollback rows retained per client pane (default: 1000; 0 disables).
# Applied on client startup; Ghostty prunes whole pages, so counts are approximate.
# Attach/reconnect requests this many rows; Core may retain less.
# scrollback_limit_lines = 1000
"##;

// Bundled at build time so the printed skill always matches this binary's release.

fn args_as_utf8<I>(args: I) -> Result<Vec<String>, String>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    args.into_iter()
        .enumerate()
        .map(|(index, arg)| {
            arg.into_string()
                .map_err(|_| format!("argument {index} is not valid UTF-8"))
        })
        .collect()
}

const CORE_ONLY_UNAVAILABLE: &str = "the legacy Herdr runtime has been removed; this command/option is unavailable in the Core-only client. Remote Core transport and the CLI API bridge are not implemented. No legacy fallback is used. Manage terminals with hc and select a separate Core home with --core-home <dir>.";
/// Pull `--core-home <path>` out of the argument list.
///
/// Test harnesses and the isolated launcher use it to run against a private
/// Core. It is an argument rather than an environment variable so a client can
/// never be retargeted by something that merely exported a variable.
fn take_core_home(args: &mut Vec<String>) -> io::Result<Option<PathBuf>> {
    let Some(index) = args.iter().position(|arg| arg == "--core-home") else {
        return Ok(None);
    };
    let path = args
        .get(index + 1)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--core-home needs a directory argument",
            )
        })?
        .clone();
    args.drain(index..=index + 1);
    let path = PathBuf::from(path);
    Ok(Some(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    }))
}

/// Select which `hc` session socket this client drives.
///
/// An isolated test uses a named session so it never touches the one a person
/// is working in. The name is restricted to a safe alphabet because it is
/// interpolated into a socket path.
fn take_core_session(args: &mut Vec<String>) -> io::Result<Option<String>> {
    let Some(index) = args.iter().position(|arg| arg == "--core-session") else {
        return Ok(None);
    };
    let name = args
        .get(index + 1)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--core-session needs a name argument",
            )
        })?
        .clone();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--core-session accepts letters, digits, '-' and '_' only",
        ));
    }
    args.drain(index..=index + 1);
    Ok(Some(name))
}

fn main() -> io::Result<()> {
    let args = args_as_utf8(std::env::args_os())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut args = args;
    if let Some(home) = take_core_home(&mut args)? {
        session::set_data_dir(home);
    }
    if let Some(session_name) = take_core_session(&mut args)? {
        session::set_core_session(session_name);
    }
    let args = args;
    let tail = &args[1..];
    match tail {
        [] => {}
        [flag] if flag == "--core" => {}
        [flag] if flag == "--help" || flag == "-h" || flag == "help" => {
            println!("ht — existing TUI, powered exclusively by herdr-core\n\nUsage: ht [--core] [--core-home <dir>] [--core-session <name>]\n       ht --version | --default-config\n       ht config check | config reset-keys\n\nCore executable: HERDR_CORE_BIN (default: herdr-core)\nCore home (client state): --core-home <dir>\nCore session: --core-session <name> (default: default, the session hc uses)\nCore socket: HC_SOCKET overrides the default socket (cannot combine with --core-session)\nState source: OSC 7501 only; unreported programs have no agent state.\n\nLegacy server, --remote, --machine, --session, direct attach, integration installers, and socket CLI commands are unavailable. There is no old-runtime fallback.");
            return Ok(());
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("herdr {} (Core-only)", build_info::version());
            return Ok(());
        }
        [flag] if flag == "--default-config" => {
            print!("# Core-only client: legacy server/detection/integration and semantic agent-notification settings are inert; clipboard toasts and terminal bells remain active.\n{DEFAULT_CONFIG}");
            return Ok(());
        }
        [group, ..] if group == "config" => {
            let outcome = cli::maybe_run(&args)?;
            if let cli::CommandOutcome::Handled(code) = outcome {
                std::process::exit(code)
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported client command",
            ));
        }
        // Updating this personal fork with upstream binaries would restore the
        // retired runtime, so `update` is not a known command.
        _ => {
            eprintln!("herdr: {CORE_ONLY_UNAVAILABLE}");
            std::process::exit(2)
        }
    }
    // Loading here surfaces configuration diagnostics before the client starts;
    // the client re-reads the live configuration itself.
    let _ = config::Config::load();
    client::run_core_client()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_only_advertises_the_single_override_layer() {
        let config: config::Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert!(config.theme.custom.is_none());
        assert!(DEFAULT_CONFIG.contains("# [theme.custom]"));
        for retired in [
            "auto_switch",
            "dark_name",
            "light_name",
            "ui.sound",
            "# settings =",
            "theme.custom.light",
            "theme.custom.dark",
            "mouse_capture",
            "copy_on_select",
            "host_cursor",
            "confirm_close",
            "prompt_new_tab_name",
            "prompt_new_workspace_name",
            "pane_borders",
            "pane_outer_borders",
            "pane_scrollbars",
            "pane_gaps",
            "show_agent_labels_on_pane_borders",
            "hide_tab_bar_when_single_tab",
            "window_title",
            "tab_bar_right",
            "tab_bar_right_separator",
            "agent_panel_scope",
            "scrollback_limit_bytes",
            "scrollback_lines",
        ] {
            assert!(!DEFAULT_CONFIG.contains(retired), "{retired}");
        }
    }

    fn invalid_utf8_arg() -> std::ffi::OsString {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0xff])
    }

    #[test]
    fn core_home_flag_is_taken_out_of_the_argument_list() {
        let mut args = vec![
            "herdr".to_owned(),
            "--core-home".to_owned(),
            "/x".to_owned(),
        ];
        assert_eq!(
            take_core_home(&mut args).unwrap(),
            Some(PathBuf::from("/x"))
        );
        assert_eq!(args, vec!["herdr".to_owned()]);
    }

    #[test]
    fn core_home_flag_makes_a_relative_path_absolute() {
        let mut args = vec![
            "herdr".to_owned(),
            "--core-home".to_owned(),
            "rel".to_owned(),
        ];
        let home = take_core_home(&mut args).unwrap().unwrap();
        assert!(home.is_absolute(), "{home:?} must be resolved");
        assert!(home.ends_with("rel"));
    }

    #[test]
    fn core_home_flag_requires_a_directory() {
        let mut args = vec!["herdr".to_owned(), "--core-home".to_owned()];
        let error = take_core_home(&mut args).expect_err("a missing value must be refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn args_as_utf8_reports_the_offending_argument_instead_of_panicking() {
        let args = vec![
            std::ffi::OsString::from("herdr"),
            std::ffi::OsString::from("pane"),
            invalid_utf8_arg(),
        ];
        assert_eq!(
            args_as_utf8(args).unwrap_err(),
            "argument 2 is not valid UTF-8"
        );
    }
}
