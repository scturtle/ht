// Keep the raw fields in their serialized order. Validation uses the same sections,
// but visits navigate first and last_pane before pane_cycle to preserve precedence.
macro_rules! key_fields {
    ($consumer:ident $($args:tt)*) => {
        $consumer! {
            ($($args)*);
            workspace {
                /// Open keybinding help.
                help: BindingConfig = BindingConfig::one("prefix+?") => action;
                /// Create a new workspace.
                new_workspace: BindingConfig = BindingConfig::one("prefix+shift+n") => action;
                /// Rename the selected workspace.
                rename_workspace: BindingConfig = BindingConfig::one("prefix+shift+w") => action;
                /// Close the selected workspace. Unbound by default.
                close_workspace: BindingConfig = BindingConfig::empty() => action;
                /// Open the workspace navigation surface.
                workspace_picker: BindingConfig = BindingConfig::one("prefix+w") => action;
                /// Open the session navigator.
                goto: BindingConfig = BindingConfig::one("prefix+g") => action;
            }
            navigate {
                /// Move workspace selection up in navigate mode.
                navigate_workspace_up: BindingConfig = BindingConfig::one("up") => navigate(workspace_up);
                /// Move workspace selection down in navigate mode.
                navigate_workspace_down: BindingConfig = BindingConfig::one("down") => navigate(workspace_down);
                // Retired; continue to read existing configs without claiming the shortcut.
                navigate_pane_left: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                navigate_pane_down: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                navigate_pane_up: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                navigate_pane_right: BindingConfig = BindingConfig::empty() => config_only;
            }
            actions {
                /// Detach the current client from its Herdr server.
                detach: BindingConfig = BindingConfig::one("prefix+q") => action;
                /// Reload config.toml in the running app/server.
                reload_config: BindingConfig = BindingConfig::one("prefix+shift+r") => action;
                /// Focus the currently visible notification target.
                open_notification_target: BindingConfig = BindingConfig::one("prefix+o") => action;
                /// Select the previous workspace.
                previous_workspace: BindingConfig = BindingConfig::empty() => action;
                /// Select the next workspace.
                next_workspace: BindingConfig = BindingConfig::empty() => action;
                /// Focus the previous agent shown in the agent panel.
                previous_agent: BindingConfig = BindingConfig::empty() => action;
                /// Focus the next agent shown in the agent panel.
                next_agent: BindingConfig = BindingConfig::empty() => action;
                /// Focus an agent by index 1-9.
                focus_agent: BindingConfig = BindingConfig::empty() => indexed(agents);
                /// Local-client shortcut that sends a clipboard image to a remote Herdr session.
                remote_image_paste: String = "ctrl+v".into() => config_only;
                /// Create a new tab in the active workspace.
                new_tab: BindingConfig = BindingConfig::one("prefix+c") => action;
                /// Rename the active tab.
                rename_tab: BindingConfig = BindingConfig::one("prefix+shift+t") => action;
                /// Select the previous tab.
                previous_tab: BindingConfig = BindingConfig::one("prefix+p") => action;
                /// Select the next tab.
                next_tab: BindingConfig = BindingConfig::one("prefix+n") => action;
                /// Move the active tab one position toward the front.
                move_tab_previous: BindingConfig = BindingConfig::empty() => action;
                /// Move the active tab one position toward the back.
                move_tab_next: BindingConfig = BindingConfig::empty() => action;
                /// Switch to tab 1-9.
                switch_tab: BindingConfig = BindingConfig::one("prefix+1..9") => indexed(tabs);
                /// Switch to workspace 1-9 from prefix mode.
                switch_workspace: BindingConfig = BindingConfig::empty() => indexed(workspaces);
                // Retired; continue to read existing configs without claiming the shortcut.
                close_tab: BindingConfig = BindingConfig::empty() => config_only;
                /// Rename the focused pane.
                rename_pane: BindingConfig = BindingConfig::one("prefix+shift+p") => action;
                /// Open the focused pane scrollback in $EDITOR.
                edit_scrollback: BindingConfig = BindingConfig::one("prefix+e") => action;
                clear_pane: BindingConfig = BindingConfig::empty() => action;
                /// Enter keyboard copy mode for the focused pane.
                copy_mode: BindingConfig = BindingConfig::one("prefix+[") => action;
                // Retired; continue to read existing configs without claiming the shortcut.
                focus_pane_left: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                focus_pane_down: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                focus_pane_up: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                focus_pane_right: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                swap_pane_left: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                swap_pane_down: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                swap_pane_up: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                swap_pane_right: BindingConfig = BindingConfig::empty() => config_only;
            }
            pane_cycle {
                /// Cycle to the next pane.
                cycle_pane_next: BindingConfig = BindingConfig::one("prefix+tab") => action;
                /// Cycle to the previous pane.
                cycle_pane_previous: BindingConfig = BindingConfig::one("prefix+shift+tab") => action;
            }
            last_pane {
                /// Focus the last focused pane across workspaces and tabs.
                last_pane: BindingConfig = BindingConfig::empty() => action;
            }
            pane_controls {
                // Retired; continue to read existing configs without claiming the shortcut.
                split_vertical: BindingConfig = BindingConfig::empty() => config_only;
                // Retired; continue to read existing configs without claiming the shortcut.
                split_horizontal: BindingConfig = BindingConfig::empty() => config_only;
                /// Close the focused pane.
                close_pane: BindingConfig = BindingConfig::one("prefix+x") => action;
                // Accept old config without reserving any of these shortcuts.
                #[serde(alias = "fullscreen")]
                zoom: BindingConfig = BindingConfig::empty() => config_only;
                resize_mode: BindingConfig = BindingConfig::empty() => config_only;
                resize_pane_left: BindingConfig = BindingConfig::empty() => config_only;
                resize_pane_down: BindingConfig = BindingConfig::empty() => config_only;
                resize_pane_up: BindingConfig = BindingConfig::empty() => config_only;
                resize_pane_right: BindingConfig = BindingConfig::empty() => config_only;
            }
        }
    };
}

pub(super) use key_fields;
