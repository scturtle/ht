// Keep raw field serialization and validation in the same order.
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
            actions {
                /// Detach the current client from its Herdr server.
                detach: BindingConfig = BindingConfig::one("prefix+d") => action;
                /// Reload config.toml in the running app/server.
                reload_config: BindingConfig = BindingConfig::one("prefix+shift+r") => action;
                /// Focus the currently visible notification target.
                open_notification_target: BindingConfig = BindingConfig::one("prefix+o") => action;
                /// Create a new tab in the active workspace.
                new_tab: BindingConfig = BindingConfig::one("prefix+c") => action;
                /// Rename the active tab.
                rename_tab: BindingConfig = BindingConfig::one("prefix+shift+t") => action;
                /// Select the previous tab.
                previous_tab: BindingConfig = BindingConfig::one("prefix+p") => action;
                /// Select the next tab.
                next_tab: BindingConfig = BindingConfig::one("prefix+n") => action;
                /// Move the active tab one position toward the front.
                move_tab_previous: BindingConfig = BindingConfig::one("ctrl+alt+comma") => action;
                /// Move the active tab one position toward the back.
                move_tab_next: BindingConfig = BindingConfig::one("ctrl+alt+period") => action;
                /// Switch to tab 1-9.
                switch_tab: BindingConfig = BindingConfig::one("prefix+1..9") => indexed;
                /// Rename the focused pane.
                rename_pane: BindingConfig = BindingConfig::one("prefix+shift+p") => action;
                /// Open the focused pane scrollback in $EDITOR.
                edit_scrollback: BindingConfig = BindingConfig::one("prefix+e") => action;
                clear_pane: BindingConfig = BindingConfig::empty() => action;
                /// Enter keyboard copy mode for the focused pane.
                copy_mode: BindingConfig = BindingConfig::one("prefix+[") => action;
                /// Focus the last focused pane across workspaces and tabs.
                last_pane: BindingConfig = BindingConfig::empty() => action;
                /// Close the focused pane.
                close_pane: BindingConfig = BindingConfig::one("prefix+x") => action;
            }
        }
    };
}

pub(super) use key_fields;
