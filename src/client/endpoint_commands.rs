//! The one Core command lane. Correlation and retirement survive the removal of federation.
use super::{
    endpoint::{EndpointRegistry, EndpointSendOutcome},
    shell::ClientShellEndpointError,
};
use crate::{
    api::schema::{Request, ResponseResult},
    protocol::ClientMessage,
};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
const ENDPOINT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RETIRED_REQUESTS: usize = 128;
struct QueuedCommand {
    generation: u64,
    boot_id: String,
    request: Box<Request>,
}
struct InFlightCommand {
    generation: u64,
    boot_id: String,
    request_id: String,
    sent_at: Instant,
}
pub(super) struct EndpointCommandResult {
    pub(super) generation: u64,
    pub(super) boot_id: String,
    pub(super) request_id: String,
    pub(super) result: Result<ResponseResult, ClientShellEndpointError>,
}
#[derive(Default)]
pub(super) struct EndpointCommands {
    queued: VecDeque<QueuedCommand>,
    in_flight: Option<InFlightCommand>,
    retired: VecDeque<(u64, String, String)>,
}
impl EndpointCommands {
    pub(super) fn enqueue(&mut self, generation: u64, boot_id: String, request: Box<Request>) {
        self.queued.push_back(QueuedCommand {
            generation,
            boot_id,
            request,
        });
    }
    pub(super) fn send_next(&mut self, connection: &mut EndpointRegistry) -> Vec<String> {
        let mut cancelled = Vec::new();
        if self.in_flight.is_some() {
            return cancelled;
        }
        while let Some(command) = self.queued.pop_front() {
            let request_id = command.request.id.clone();
            if !connection.accepts(command.generation) {
                cancelled.push(request_id);
                continue;
            }
            let message = ClientMessage::ClientShellEndpointRequest {
                boot_id: command.boot_id.clone(),
                request: command.request,
            };
            if connection.send(&message) != EndpointSendOutcome::Sent {
                cancelled.push(request_id);
                continue;
            }
            self.in_flight = Some(InFlightCommand {
                generation: command.generation,
                boot_id: command.boot_id,
                request_id,
                sent_at: Instant::now(),
            });
            break;
        }
        cancelled
    }
    fn matches(&self, generation: u64, boot: &str, id: &str) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|c| c.generation == generation && c.boot_id == boot && c.request_id == id)
    }
    #[cfg(test)]
    pub(super) fn accepts_response(&self, generation: u64, boot: &str, id: &str) -> bool {
        self.matches(generation, boot, id)
    }
    pub(super) fn diagnostic_snapshot(&self) -> serde_json::Value {
        serde_json::json!({"queued":self.queued.len(),"retired":self.retired.len(),
            "in_flight":self.in_flight.as_ref().map(|c|serde_json::json!({"generation":c.generation,"boot_id":c.boot_id,"request_id":c.request_id,"elapsed_ms":c.sent_at.elapsed().as_millis()}))})
    }
    /// A replacement Core cannot inherit actions issued to an older boot.
    /// Retire uncertain in-flight delivery; never replay the old request.
    pub(super) fn observe_snapshot(&mut self, generation: u64, boot: &str) -> Vec<String> {
        let mut cancelled = Vec::new();
        if self
            .in_flight
            .as_ref()
            .is_some_and(|c| c.generation != generation || c.boot_id != boot)
        {
            if let Some(c) = self.in_flight.take() {
                if self.retired.len() == MAX_RETIRED_REQUESTS {
                    self.retired.pop_front();
                }
                self.retired
                    .push_back((c.generation, c.boot_id, c.request_id.clone()));
                cancelled.push(c.request_id);
            }
        }
        self.queued.retain(|c| {
            if c.generation == generation && c.boot_id == boot {
                true
            } else {
                cancelled.push(c.request.id.clone());
                false
            }
        });
        cancelled
    }
    pub(super) fn expire(&mut self, now: Instant) -> Vec<EndpointCommandResult> {
        if self
            .in_flight
            .as_ref()
            .is_none_or(|c| now.saturating_duration_since(c.sent_at) < ENDPOINT_COMMAND_TIMEOUT)
        {
            return Vec::new();
        }
        let Some(command) = self.in_flight.take() else {
            return Vec::new();
        };
        if self.retired.len() == MAX_RETIRED_REQUESTS {
            self.retired.pop_front();
        }
        self.retired.push_back((
            command.generation,
            command.boot_id.clone(),
            command.request_id.clone(),
        ));
        vec![EndpointCommandResult {
            generation: command.generation,
            boot_id: command.boot_id,
            request_id: command.request_id,
            result: Err(ClientShellEndpointError {
                code: Some("endpoint_timeout".into()),
                message:
                    "Core did not respond to the action; delivery is uncertain and was not retried"
                        .into(),
            }),
        }]
    }
    pub(super) fn receive_response(
        &mut self,
        generation: u64,
        boot: &str,
        id: &str,
        result: Result<Box<ResponseResult>, crate::protocol::EndpointResponseError>,
    ) -> Option<EndpointCommandResult> {
        if let Some(index) = self
            .retired
            .iter()
            .position(|(g, b, i)| *g == generation && b == boot && i == id)
        {
            self.retired.remove(index);
            return None;
        }
        if !self.matches(generation, boot, id) {
            return None;
        }
        let command = self.in_flight.take()?;
        Some(EndpointCommandResult {
            generation: command.generation,
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
                generation: 1,
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
        for (generation, boot, id) in [
            (2, "boot-a", "request-a"),
            (1, "boot-b", "request-a"),
            (1, "boot-a", "request-b"),
        ] {
            assert!(c
                .receive_response(generation, boot, id, Ok(Box::new(ResponseResult::Ok {})))
                .is_none());
            assert!(c.in_flight.is_some());
        }
        assert!(c
            .receive_response(
                1,
                "boot-a",
                "request-a",
                Ok(Box::new(ResponseResult::Ok {}))
            )
            .is_some());
        assert!(c.in_flight.is_none());
    }
    #[test]
    fn large_selection_response_survives_without_truncation() {
        let mut c = commands();
        let selection = "selected".repeat(160_000);
        let completed = c
            .receive_response(
                1,
                "boot-a",
                "request-a",
                Ok(Box::new(ResponseResult::PaneSelection {
                    pane_id: "w1:p1".into(),
                    text: selection.clone(),
                })),
            )
            .unwrap();
        assert!(
            matches!(completed.result,Ok(ResponseResult::PaneSelection {text,..}) if text==selection)
        );
    }
    #[test]
    fn timeout_does_not_replay_a_command_and_its_late_reply_cannot_complete_the_next_one() {
        let mut c = commands();
        let expired = c.expire(Instant::now() + ENDPOINT_COMMAND_TIMEOUT);
        assert_eq!(expired.len(), 1);
        assert!(c.in_flight.is_none());
        assert!(c.queued.is_empty());
        assert!(c
            .expire(Instant::now() + ENDPOINT_COMMAND_TIMEOUT)
            .is_empty());
        c.in_flight = Some(InFlightCommand {
            generation: 1,
            boot_id: "boot-a".into(),
            request_id: "next".into(),
            sent_at: Instant::now(),
        });
        assert!(c
            .receive_response(
                1,
                "boot-a",
                "request-a",
                Ok(Box::new(ResponseResult::Ok {}))
            )
            .is_none());
        assert!(c.accepts_response(1, "boot-a", "next"));
    }
    #[test]
    fn replacement_boot_retires_uncertain_delivery_without_replaying_it() {
        let mut c = commands();
        assert_eq!(c.observe_snapshot(1, "boot-b"), ["request-a"]);
        assert!(c.in_flight.is_none() && c.queued.is_empty());
        assert!(c.observe_snapshot(1, "boot-b").is_empty());
        assert!(c
            .receive_response(
                1,
                "boot-a",
                "request-a",
                Ok(Box::new(ResponseResult::Ok {}))
            )
            .is_none());
    }
    #[test]
    fn retired_uncertain_replies_remain_bounded() {
        let mut c = commands();
        for n in 0..300 {
            c.in_flight = Some(InFlightCommand {
                generation: 1,
                boot_id: "boot-a".into(),
                request_id: n.to_string(),
                sent_at: Instant::now(),
            });
            c.expire(Instant::now() + ENDPOINT_COMMAND_TIMEOUT);
        }
        assert_eq!(c.retired.len(), MAX_RETIRED_REQUESTS);
    }
}
