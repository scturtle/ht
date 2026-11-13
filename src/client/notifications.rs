#[cfg(test)]
use std::io;

#[cfg(test)]
use tracing::debug;
use tracing::warn;

#[cfg(test)]
use crate::protocol::NotifyKind;

use super::shell;

pub(super) fn handle_shell_notification_effects(
    effects: Vec<shell::ClientShellNotificationEffect>,
) {
    for effect in effects {
        match effect {
            shell::ClientShellNotificationEffect::Terminal { title, body } => {
                if let Err(err) = crate::terminal_notify::show_notification(&title, body.as_deref())
                {
                    warn!(err = %err, "failed to emit terminal notification");
                }
            }
            shell::ClientShellNotificationEffect::System { title, body } => {
                let result = crate::platform::show_desktop_notification(&title, body.as_deref());
                if let Err(err) = result {
                    warn!(err = %err, "failed to emit system notification");
                }
            }
        }
    }
}

#[cfg(test)]
pub(super) fn handle_notify_with_notifiers(
    kind: NotifyKind,
    message: &str,
    body: Option<&str>,
    mut show_terminal_notification: impl FnMut(&str, Option<&str>) -> io::Result<bool>,
    mut show_system_notification: impl FnMut(&str, Option<&str>) -> io::Result<bool>,
) {
    match kind {
        NotifyKind::Toast => {
            debug!(
                message = message,
                "received terminal toast notification from server"
            );
            if let Err(err) = show_terminal_notification(message, body) {
                warn!(err = %err, "failed to emit terminal notification");
            }
        }
        NotifyKind::SystemToast => {
            debug!(
                message = message,
                "received system toast notification from server"
            );
            if let Err(err) = show_system_notification(message, body) {
                warn!(err = %err, "failed to emit system notification");
            }
        }
    }
}
