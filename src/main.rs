use ghostty_vt as ghostty;
use ghostty_vt::pane_graphics_files;
use std::io;
use std::path::PathBuf;
mod agent_resume;
mod api;
mod app;
mod build_info;
// API argument projections stay available for shared UI/schema tests,
// including methods that the Core endpoint deliberately does not advertise.
#[allow(dead_code)]
mod cli;
mod client;
// Historical configuration keys and projections remain parseable even when Core
// does not implement the corresponding feature; no runtime is constructed here.
#[allow(dead_code)]
mod config;
mod copy_mode;
mod detect;
mod input;
mod kitty_graphics;
mod layout;
mod logging;
mod pane;
mod platform;
// Shared semantic messages and presentation data, not a legacy socket codec.
mod endpoint_paths;
#[cfg(test)]
mod noninteractive_process;
#[allow(dead_code)]
mod protocol;
mod raw_input;
mod render_prof;
mod selection;
mod session;
mod sound;
mod terminal_effects;
mod terminal_modes;
mod terminal_notify;
mod terminal_theme;
mod ui;
mod worktree;
pub(crate) const HERDR_ENV_VAR: &str = "HERDR_ENV";

pub(crate) const HERDR_ENV_VALUE: &str = "1";

const NESTED_HERDR_MESSAGES: [&str; 6] = [
    "inception detected. we need to go deeper... said no one ever.",
    "recursion is a pathway to many abilities some consider to be... unnatural.",
    "you were so preoccupied with whether you could, you didn't stop to think if you should. — dr. malcolm",
    "recursive herdring is disabled. somewhere, a call stack breathes a sigh of relief.",
    "recursive descent denied. there is, in fact, such a thing as too much herdr.",
    "recursion detected. base case not found. aborting.",
];

const DEFAULT_CONFIG: &str = r##"# herdr configuration
# Place this file at ~/.config/herdr/config.toml

# Show first-run notification setup on startup.
# Missing also shows onboarding; set false after you've chosen.
# onboarding = true

[theme]
# Built-in themes: catppuccin, terminal, tokyo-night, dracula, nord,
#                  gruvbox, one-dark, solarized, kanagawa, rose-pine,
#                  vesper
# name = "catppuccin"

# Follow host terminal light/dark appearance and switch Herdr UI themes.
# Existing manual behavior is unchanged unless this is true.
# auto_switch = false
# dark_name = "catppuccin"
# light_name = "catppuccin-latte"

# Override individual color tokens on top of the base theme.
# Accepts: hex (#rrggbb), named colors, rgb(r,g,b), or panel_bg = "reset"
# [theme.custom]
# sidebar_bg = "#181825"
# active_row_bg = "#1e1e2e"
# selection_bg = "#313244"
# panel_bg = "reset"
# accent = "#f5c2e7"
# red = "#ff6188"
# green = "#a6e3a1"

# Layer appearance-specific overrides on top when auto_switch is enabled.
# [theme.custom.light]
# panel_bg = "#eff1f5"
# text = "#4c4f69"
#
# [theme.custom.dark]
# panel_bg = "#1e1e2e"
# text = "#cdd6f4"

[terminal]
# Executable used for new interactive panes.
# Empty means $SHELL, then /bin/sh.
# default_shell = ""

# Startup mode for new interactive pane shells: "auto", "login", or "non_login".
# "auto" uses login shells on macOS and keeps the current behavior elsewhere.
# shell_mode = "auto"

# CWD policy for new panes, tabs, and workspaces when no explicit --cwd is provided.
# Use "follow" to inherit the source pane/workspace, "home" for $HOME,
# "current" for Herdr's process directory, or a fixed path such as "~/Projects".
# new_cwd = "follow"

# Render pane images in Kitty graphics-compatible outer terminals.
# kitty_graphics = true

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
# settings = "prefix+s"
# detach = "prefix+q"
# reload_config = "prefix+shift+r"
# open_notification_target = "prefix+o"
# workspace_picker = "prefix+w"
# goto = "prefix+g"
# new_workspace = "prefix+shift+n"
# new_worktree = "prefix+shift+g"
# open_worktree = ""    # optional, unset by default
# remove_worktree = ""  # optional, unset by default; opens confirmation
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
# toggle_sidebar = "prefix+b"

# Navigate-mode movement. These local shortcuts win while navigate mode is open.
# They are independent from focus_pane_*. Do not include prefix+, esc, enter, tab, or 1..9 here.
# navigate_workspace_up = "up"
# navigate_workspace_down = "down"
# navigate_pane_left = "h"      # left arrow always focuses the pane to the left
# navigate_pane_down = "j"
# navigate_pane_up = "k"
# navigate_pane_right = "l"     # right arrow always focuses the pane to the right

# Custom commands use the same binding syntax.
# type = "shell" runs detached in the background.
# type = "pane" opens a temporary pane and closes it when the command exits.
# [[keys.command]]
# key = "prefix+alt+g"
# type = "pane"
# command = "lazygit"

# Legacy indexed shortcut config is still parsed for compatibility.
# Prefer switch_tab, switch_workspace, and focus_agent for new configs.
# [keys.indexed]
# tabs = ""       # e.g. "ctrl" makes ctrl+1..9 switch tabs directly
# workspaces = "" # e.g. "ctrl+shift" makes ctrl+shift+1..9 switch workspaces directly
# agents = ""     # e.g. "alt" makes alt+1..9 focus agent rows directly

# Size of the virtual terminal used when no client is attached.
# Attached clients always use their own terminal size.
[server]
# Windows only: allow ordinary same-account clients to control an elevated server.
# Requires a server restart.
# allow_unelevated_clients = false
# headless_cols = 120
# headless_rows = 40

# [worktrees]
# directory = "~/.herdr/worktrees"

[ui]
# Sidebar width (auto-scaled based on workspace names, this sets the default)
# sidebar_width = 26

# Minimum sidebar width when expanded (columns)
# sidebar_min_width = 18

# Maximum sidebar width when expanded (columns)
# sidebar_max_width = 36

# Start with the sidebar collapsed. Changes take effect on the next launch.
# sidebar_start_collapsed = false

# Collapsed sidebar presentation: "compact" keeps the narrow status rail, "hidden" uses zero width.
# sidebar_collapsed_mode = "compact"

# Terminal width at or below which Herdr uses the mobile single-column layout.
# Increase this for foldables, tablets, or wide phone terminals.
# mobile_width_threshold = 64

# Capture mouse input for Herdr's mouse UI.
# Set false to let the terminal handle normal clicks, such as Cmd-clicking URLs.
# Pane apps like lazygit and btop can still receive mouse when they request it.
# mouse_capture = true

# Automatically copy text selected with the mouse.
# Set false to retain drag or double-click word selection until Ctrl+C,
# or Cmd+C when the host forwards it, copies and clears it.
# copy_on_select = true

# Host cursor policy: "auto", "native", or "drawn".
# "auto" draws Herdr's own cursor on native Windows builds and WSL to avoid ConPTY cursor flicker, and uses the native terminal cursor elsewhere.
# "native" always uses the outer terminal cursor. "drawn" always draws Herdr's cursor as terminal cell content.
# host_cursor = "auto"

# Optional modifier that forwards right-click hold/drag gestures to pane apps instead of opening Herdr's pane menu.
# Empty/off disables this. Shift is intentionally unsupported because terminals commonly reserve Shift+mouse.
# right_click_passthrough_modifier = ""

# Force a full redraw when the outer terminal regains focus.
# Set false to reduce visible flashing when switching back to Herdr.
# Trade-off: rare host terminal surface corruption may persist until the next full redraw.
# redraw_on_focus_gained = true

# Pane scrollback lines to scroll per mouse wheel notch.
# mouse_scroll_lines = 3

# Ask for confirmation before closing a workspace
# confirm_close = true

# Ask for a tab name before creating a new tab.
# Set false to create tabs immediately with generated names.
# prompt_new_tab_name = true

# Ask for a workspace name before interactive creation.
# prompt_new_workspace_name = false

# Draw borders around split panes.
# "auto" draws them only for split panes, "always" also frames a lone pane
# (only while pane_outer_borders is enabled), "off" disables them.
# Legacy booleans still parse: true = "auto", false = "off".
# pane_borders = "auto"

# Draw borders along the outside edge of the pane area.
# Disable for tmux-style internal splitters without an outside frame.
# pane_outer_borders = true

# Draw interactive scrollbars beside terminal panes.
# Set false to reclaim the scrollbar column and keep it out of terminal-native selections.
# pane_scrollbars = true

# Keep split panes visually separated instead of sharing divider borders.
# pane_gaps = true

# Show detected/reported agent labels in split pane borders when no manual pane name is set.
# show_agent_labels_on_pane_borders = false

# Hide the tab row when a workspace has exactly one tab.
# New tabs can still be created with the configured keybinding.
# hide_tab_bar_when_single_tab = false

# Desktop tab row placement: "top" or "bottom".
# tab_bar_position = "top"

# Ordered status entries at the right edge of the desktop tab bar.
# Supported types: zoom, hostname, datetime, text, and command.
# Hostname, datetime, and command entries resolve on the Herdr server.
# tab_bar_right = []
# tab_bar_right_separator = " "

# Title Herdr writes to the terminal it runs in, which is what window managers
# show in title, tab, and group bars. Tokens are {hostname}, {workspace}, {tab},
# {pane}, and {terminal_title}; {{ and }} are literal braces.
# The title renders on the Herdr server, so {hostname} names the host the panes
# run on even when attaching from a remote client.
# Set to "" to leave the outer terminal title alone.
# window_title = "{hostname}: {workspace}"

# Agent panel ordering: "spaces" (grouped by space) or "priority" (attention queue).
# "workspaces" is accepted as an alias for "spaces".
# agent_panel_sort = "spaces"

# Agent status indicators: "dots" preserves the compact color marks; "symbols" uses
# distinct static glyphs for blocked, working, done, idle, and unknown states.
# status_indicators = "dots"

# Accent color for highlights, borders, and navigation UI.
# Accepts: hex (#89b4fa), named colors (cyan, blue, magenta), or rgb(r,g,b)
# accent = "cyan"

# Expanded agent rows. Built-ins are state_icon, state_text, machine, workspace, tab,
# pane, agent, terminal_title, and terminal_title_stripped.
# Custom values reported through pane metadata use a $name token.
# A token occurrence may be styled with { token = "workspace", fg = "#89b4fa", bold = true, dim = false }.
# Omitted style fields preserve the contextual default.
# [ui.sidebar.agents]
# Blank rows between agent entries. Set to 1 to restore the previous spacing.
# row_gap = 0
# rows = [["state_icon", "machine", "workspace", "tab"], ["agent"]]
# Optional canonical agent IDs replace the default rows for matching agents.
# [ui.sidebar.agents.rows_by_agent]
# claude = [["state_icon", "machine", "workspace", "tab"], ["terminal_title_stripped"], ["agent"]]

# Expanded space rows. Built-ins are state_icon, state_text, workspace, branch, and git_status.
# Custom values reported through workspace metadata use a $name token, for example $jj_status.
# Inline token styles accept strict #RGB/#RRGGBB foregrounds plus bold and dim booleans.
# [ui.sidebar.spaces]
# Blank rows between space entries. Set to 1 to restore the previous spacing.
# row_gap = 0
# rows = [["state_icon", "workspace"], ["branch", "git_status"]]

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

# Play sounds when agents change state in background workspaces
[ui.sound]
# enabled = true
# Optional custom mp3 sound files. Relative paths are resolved from this config file's directory.
# path = "sounds/notification.mp3"   # one mp3 file for all sound notifications
# done_path = "sounds/done.mp3"      # overrides only finished notifications
# request_path = "sounds/request.mp3" # overrides only needs-attention notifications

# Per-agent overrides: default | on | off
# By default, droid is muted.
# [ui.sound.agents]
# droid = "off"

[session]
# Resume supported AI-agent panes into their native conversation sessions after
# a Herdr server restart. Requires official integrations that report session refs.
# resume_agents_on_restore = true
# Milliseconds between automatic agent restores; 0 starts them without spacing.
# startup_per_agent_delay_ms = 100

[remote]
# Whether herdr manages the ssh config used for `herdr --remote`.
# When true (default), herdr runs remote ssh through a generated config that
# includes your ~/.ssh/config first and adds ServerAliveInterval/
# ServerAliveCountMax as fallbacks (so any keepalive values you set yourself
# still win) to survive idle network/NAT timeouts. Herdr also uses a private
# per-attach OpenSSH control socket to reuse the first authenticated connection.
# Set false to run plain ssh against your ssh config unchanged — this does not
# force keepalive or multiplexing off, it only stops herdr from adding its own.
# manage_ssh_config = true

[experimental]
# Allow launching herdr from inside a herdr-managed pane.
# allow_nested = false
# Save recent pane screen history across full server restarts.
pane_history = false
# While prefix mode is active, temporarily switch the host input source to
# an ASCII-capable mode so prefix commands register even when an IME is
# active, then restore the previous input source when prefix mode exits. On
# macOS this selects the ASCII-capable keyboard layout; on Windows it toggles
# a Korean IME between Hangul and English (other IME languages are left
# unchanged). macOS and Windows only; best-effort. Default: false.
# switch_ascii_input_source_in_prefix = false
# Expose the focused pane's cursor to the outer terminal so macOS input
# methods keep tracking the candidate window when TUIs paint their own
# cursor (Claude Code, pi, codex). Trade-off: extra cursor visible for
# apps that hide it without painting a replacement (vim normal mode, etc.).
# reveal_hidden_cursor_for_cjk_ime = false
# Optional allow-list: only reveal for focused panes whose detected agent
# matches one of these names. Empty means apply to any focused pane.
# If the list contains no valid names, the reveal does not apply.
# Accepted: pi, claude, codex, gemini, cursor, devin, cline, opencode,
# copilot, kimi, kiro, droid, amp, grok, hermes, kilo, qodercli, qoder, qwen,
# qwen-code, letta, letta-code, maki.
# cjk_ime_agents = []
# Cursor shape rendered when reveal_hidden_cursor_for_cjk_ime is true.
# Values: block, steady_block (default), underline, steady_underline, bar, steady_bar.
# cjk_ime_cursor_shape = "steady_block"

[advanced]
# Maximum scrollback buffer size in bytes retained per pane terminal.
# Matches Ghostty's default scrollback-limit behavior.
# scrollback_limit_bytes = 10000000
"##;

// Bundled at build time so the printed skill always matches this binary's release.

fn should_block_nested(config: &config::Config) -> bool {
    should_block_nested_for_env(config, std::env::var(HERDR_ENV_VAR).ok().as_deref())
}

fn should_block_nested_for_env(config: &config::Config, herdr_env: Option<&str>) -> bool {
    !config.experimental.allow_nested && herdr_env == Some(HERDR_ENV_VALUE)
}

fn random_nested_message() -> &'static str {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as usize)
        .unwrap_or(0);
    let index = (nanos ^ (std::process::id() as usize)) % NESTED_HERDR_MESSAGES.len();
    NESTED_HERDR_MESSAGES[index]
}

fn exit_if_nested_disabled(config: &config::Config) {
    if should_block_nested(config) {
        eprintln!("\x1b[1merror:\x1b[0m nested herdr is disabled by default.");
        eprintln!("see configuration if you want to enable it.");
        eprintln!();
        eprintln!("\x1b[2m\"{}\"\x1b[0m", random_nested_message());
        std::process::exit(1);
    }
}

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
/// Pull `--core-home <path>` or `--core-session <name>` out of the arguments.
///
/// `--core-home` moves client-local state; `--core-session` selects which `hc`
/// session socket to drive, which is how an isolated test runs without touching
/// the session a person is working in.
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
        [] => {},
        [flag] if flag=="--core" => {},
        [flag] if flag=="--help" || flag=="-h" || flag=="help" => {
            println!("ht — existing TUI, powered exclusively by herdr-core\n\nUsage: ht [--core] [--core-home <dir>] [--core-session <name>]\n       ht --version | --default-config\n       ht config check | config reset-keys\n\nCore executable: HERDR_CORE_BIN (default: herdr-core)\nCore home (client state): --core-home <dir>\nCore session: --core-session <name> (default: default, the session hc uses)\nCore socket: HC_SOCKET overrides the default socket (cannot combine with --core-session)\nState source: OSC 7501 only; unreported programs have no agent state.\n\nLegacy server, --remote, --machine, --session, direct attach, integration installers, and socket CLI commands are unavailable. There is no old-runtime fallback.");
            return Ok(())
        },
        [flag] if flag=="--version" || flag=="-V" => {println!("herdr {} (Core-only)",build_info::version());return Ok(())},
        [flag] if flag=="--default-config" => {print!("# Core-only client: legacy server/detection/integration and semantic agent-notification settings are inert; clipboard toasts and terminal bells remain active.\n{DEFAULT_CONFIG}");return Ok(())},
        [group,..] if group=="config" => {
            let outcome=cli::maybe_run(&args)?;
            if let cli::CommandOutcome::Handled(code)=outcome {std::process::exit(code)}
            return Err(io::Error::new(io::ErrorKind::InvalidInput,"unsupported client command"));
        },
        // Updating this personal fork with upstream binaries would restore the retired runtime.
        [group,..] if group=="update" => return Err(io::Error::new(io::ErrorKind::Unsupported,"self-update is disabled in this Core-only fork; rebuild this checkout so the legacy runtime cannot be restored by an upstream binary")),
        _ => { eprintln!("herdr: {CORE_ONLY_UNAVAILABLE}");std::process::exit(2) }
    }
    let loaded = config::Config::load();
    exit_if_nested_disabled(&loaded.config);
    client::run_core_client()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_lists_ui_accent_before_nested_tables() {
        let accent_marker = "# accent = \"cyan\"";
        assert_eq!(DEFAULT_CONFIG.matches(accent_marker).count(), 1);

        let accent = DEFAULT_CONFIG.find(accent_marker).unwrap();
        let sidebar = DEFAULT_CONFIG.find("# [ui.sidebar.agents]").unwrap();

        assert!(accent < sidebar);
    }

    #[test]
    fn nested_herdr_blocks_when_env_is_set() {
        let config = config::Config::default();
        assert!(should_block_nested_for_env(&config, Some(HERDR_ENV_VALUE)));
    }

    #[test]
    fn nested_herdr_does_not_block_when_allowed() {
        let config: config::Config =
            toml::from_str("[experimental]\nallow_nested = true\n").unwrap();
        assert!(!should_block_nested_for_env(&config, Some(HERDR_ENV_VALUE)));
    }

    #[test]
    fn nested_herdr_does_not_block_without_env() {
        let config = config::Config::default();
        assert!(!should_block_nested_for_env(&config, None));
    }

    #[test]
    fn random_nested_message_comes_from_known_set() {
        let message = random_nested_message();
        assert!(NESTED_HERDR_MESSAGES.contains(&message));
    }

    #[test]
    fn nested_message_strings_no_longer_repeat_herdr_prefix() {
        assert!(NESTED_HERDR_MESSAGES
            .iter()
            .all(|message| !message.starts_with("herdr:")));
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
    fn args_without_a_core_home_flag_are_left_alone() {
        let mut args = vec!["herdr".to_owned(), "--core".to_owned()];
        assert_eq!(take_core_home(&mut args).unwrap(), None);
        assert_eq!(args, vec!["herdr".to_owned(), "--core".to_owned()]);
    }

    #[test]
    fn args_as_utf8_passes_through_valid_arguments() {
        let args = ["herdr", "pane", "get", "pane-1"].map(std::ffi::OsString::from);
        assert_eq!(
            args_as_utf8(args).unwrap(),
            ["herdr", "pane", "get", "pane-1"]
        );
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
