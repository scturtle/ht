//! The single Core command lane, correlated by projection boot and request ID.
use super::{
    connection::{EndpointConnection, SendOutcome},
    shell::ClientShellEndpointError,
};
use crate::{
    actions::{ActionRequest, ActionResult},
    protocol::ClientMessage,
};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
const ENDPOINT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
struct QueuedCommand {
    boot_id: String,
    request: Box<ActionRequest>,
}
struct InFlightCommand {
    boot_id: String,
    request_id: String,
    sent_at: Instant,
}
pub(super) struct EndpointCommandResult {
    pub(super) boot_id: String,
    pub(super) request_id: String,
    pub(super) result: Result<ActionResult, ClientShellEndpointError>,
}
#[derive(Default)]
pub(super) struct EndpointCommands {
    queued: VecDeque<QueuedCommand>,
    in_flight: Option<InFlightCommand>,
}
impl EndpointCommands {
    pub(super) fn enqueue(&mut self, boot_id: String, request: Box<ActionRequest>) {
        self.queued.push_back(QueuedCommand { boot_id, request });
    }
    pub(super) fn send_next(&mut self, connection: &mut EndpointConnection) -> Vec<String> {
        let mut cancelled = Vec::new();
        if self.in_flight.is_some() {
            return cancelled;
        }
        while let Some(command) = self.queued.pop_front() {
            let request_id = command.request.id.clone();
            if !connection.is_connected() {
                cancelled.push(request_id);
                continue;
            }
            let message = ClientMessage::ClientShellEndpointRequest {
                boot_id: command.boot_id.clone(),
                request: command.request,
            };
            if connection.send(&message) != SendOutcome::Sent {
                cancelled.push(request_id);
                continue;
            }
            self.in_flight = Some(InFlightCommand {
                boot_id: command.boot_id,
                request_id,
                sent_at: Instant::now(),
            });
            break;
        }
        cancelled
    }
    fn matches(&self, boot: &str, id: &str) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|c| c.boot_id == boot && c.request_id == id)
    }
    #[cfg(test)]
    pub(super) fn accepts_response(&self, boot: &str, id: &str) -> bool {
        self.matches(boot, id)
    }
    #[cfg(test)]
    pub(super) fn is_idle(&self) -> bool {
        self.queued.is_empty() && self.in_flight.is_none()
    }
    /// A replacement Core cannot inherit actions issued to an older boot.
    /// Retire uncertain in-flight delivery; never replay the old request.
    pub(super) fn observe_snapshot(&mut self, boot: &str) -> Vec<String> {
        let mut cancelled = Vec::new();
        if self.in_flight.as_ref().is_some_and(|c| c.boot_id != boot) {
            if let Some(c) = self.in_flight.take() {
                cancelled.push(c.request_id);
            }
        }
        self.queued.retain(|c| {
            if c.boot_id == boot {
                true
            } else {
                cancelled.push(c.request.id.clone());
                false
            }
        });
        cancelled
    }
    pub(super) fn expire(&mut self, now: Instant) -> Option<EndpointCommandResult> {
        let sent_at = self.in_flight.as_ref()?.sent_at;
        if now.saturating_duration_since(sent_at) < ENDPOINT_COMMAND_TIMEOUT {
            return None;
        }
        let command = self.in_flight.take()?;
        Some(EndpointCommandResult {
            boot_id: command.boot_id,
            request_id: command.request_id,
            result: Err(ClientShellEndpointError {
                code: Some("endpoint_timeout".into()),
                message:
                    "Core did not respond to the action; delivery is uncertain and was not retried"
                        .into(),
            }),
        })
    }
    pub(super) fn receive_response(
        &mut self,
        boot: &str,
        id: &str,
        result: Result<Box<ActionResult>, crate::protocol::EndpointResponseError>,
    ) -> Option<EndpointCommandResult> {
        // Request IDs advance for the lifetime of the UI, even across a reconnect.
        // Once a command leaves the lane, its late reply cannot complete another one.
        if !self.matches(boot, id) {
            return None;
        }
        let command = self.in_flight.take()?;
        Some(EndpointCommandResult {
            boot_id: command.boot_id,
            request_id: command.request_id,
            result: result.map(|r| *r).map_err(|e| ClientShellEndpointError {
                code: e.code,
                message: e.message,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn commands() -> EndpointCommands {
        EndpointCommands {
            in_flight: Some(InFlightCommand {
                boot_id: "boot-a".into(),
                request_id: "request-a".into(),
                sent_at: Instant::now(),
            }),
            ..Default::default()
        }
    }
    #[test]
    fn response_completion_is_correlated_and_clears_the_lane() {
        let mut c = commands();
        for (boot, id) in [("boot-b", "request-a"), ("boot-a", "request-b")] {
            assert!(c
                .receive_response(boot, id, Ok(Box::new(ActionResult::Ok {})))
                .is_none());
            assert!(c.in_flight.is_some());
        }
        assert!(c
            .receive_response("boot-a", "request-a", Ok(Box::new(ActionResult::Ok {})))
            .is_some());
        assert!(c.in_flight.is_none());
    }
    #[test]
    fn large_selection_response_survives_without_truncation() {
        let mut c = commands();
        let selection = "selected".repeat(160_000);
        let completed = c
            .receive_response(
                "boot-a",
                "request-a",
                Ok(Box::new(ActionResult::PaneSelection {
                    pane_id: "w1:p1".into(),
                    text: selection.clone(),
                })),
            )
            .unwrap();
        assert!(
            matches!(completed.result,Ok(ActionResult::PaneSelection {text,..}) if text==selection)
        );
    }
    #[test]
    fn timeout_does_not_replay_a_command_and_its_late_reply_cannot_complete_the_next_one() {
        let mut c = commands();
        let expired = c.expire(Instant::now() + ENDPOINT_COMMAND_TIMEOUT).unwrap();
        assert_eq!(expired.request_id, "request-a");
        assert_eq!(
            expired.result.unwrap_err().code.as_deref(),
            Some("endpoint_timeout")
        );
        assert!(c.is_idle());
        assert!(c
            .expire(Instant::now() + ENDPOINT_COMMAND_TIMEOUT)
            .is_none());
        c.in_flight = Some(InFlightCommand {
            boot_id: "boot-a".into(),
            request_id: "next".into(),
            sent_at: Instant::now(),
        });
        assert!(c
            .receive_response("boot-a", "request-a", Ok(Box::new(ActionResult::Ok {})))
            .is_none());
        assert!(c.accepts_response("boot-a", "next"));
    }
    #[test]
    fn replacement_boot_retires_uncertain_delivery_without_replaying_it() {
        let mut c = commands();
        assert_eq!(c.observe_snapshot("boot-b"), ["request-a"]);
        assert!(c.in_flight.is_none() && c.queued.is_empty());
        assert!(c.observe_snapshot("boot-b").is_empty());
        assert!(c
            .receive_response("boot-a", "request-a", Ok(Box::new(ActionResult::Ok {})))
            .is_none());
    }
}
