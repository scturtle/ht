//! Ordered Core invalidations and presentation streams. No implicit adoption.
use super::super::transport::DATA;
use super::view;
use super::*;

impl Session {
    pub fn consume(&mut self, frame: Frame) -> io::Result<Vec<ServerMessage>> {
        let mut messages = Vec::new();
        if Some(frame.channel) == self.core.events_channel && frame.kind == DATA {
            if self.events.len() + frame.payload.len() > 2 * 1024 * 1024 {
                return Err(invalid("Core event exceeds 2 MiB"));
            }
            self.events.extend_from_slice(&frame.payload);
            while let Some(end) = self.events.iter().position(|b| *b == b'\n') {
                let event: Value = serde_json::from_slice(&self.events[..end])?;
                self.events.drain(..=end);
                let p = &event["params"];
                if event["method"] == "missed" {
                    let seq = p["seq"]
                        .as_u64()
                        .ok_or_else(|| invalid("missed snapshot lacks sequence"))?;
                    if seq >= self.seq {
                        self.seq = seq;
                        let terminals = p["terminals"]
                            .as_array()
                            .ok_or_else(|| invalid("missed snapshot lacks terminals"))?;
                        for terminal in terminals {
                            self.reconcile(terminal.clone(), seq)?;
                        }
                    }
                    // The missed snapshot does NOT include documents.
                    self.refresh_shared()?;
                    self.sync_views()?;
                    self.snapshot_dirty = true;
                    continue;
                }
                if event["method"] != "event" {
                    continue;
                }
                let seq = p["seq"]
                    .as_u64()
                    .ok_or_else(|| invalid("Core event lacks sequence"))?;
                if seq <= self.seq {
                    continue;
                }
                self.seq = seq;
                if p["type"] == "doc" {
                    if p["data"]["name"] == super::super::organization::DOCUMENT
                        && p["data"]["version"]
                            .as_u64()
                            .is_some_and(|v| v > self.document_version)
                        && self.refresh_shared()?
                    {
                        self.retire_completed_programs()?;
                        self.sync_views()?;
                    }
                    continue;
                }
                let Some(address) = p["terminal"].as_str() else {
                    continue;
                };
                let Some(id) = self.model.id(address) else {
                    // Even a tagged live terminal is not membership. Only the CAS
                    // document may introduce it, so failed creates cannot reinsert panes.
                    continue;
                };
                if p["type"] == "removed"
                    || (p["type"] == "labels" && p["data"]["labels"]["herdr"] != "tui")
                {
                    self.change(|model| match model.id(address) {
                        Some(current) => model.retire(current),
                        None => Ok(()),
                    })?;
                    self.sync_views()?;
                    continue;
                }
                let mut terminal = self.model.records[&id].facts.clone();
                if p["data"]["record"].is_object() {
                    terminal = p["data"]["record"].clone();
                }
                if let (Some(target), Some(data)) =
                    (terminal.as_object_mut(), p["data"].as_object())
                {
                    for (key, value) in data {
                        target.insert(key.clone(), value.clone());
                    }
                }
                if matches!(p["type"].as_str(), Some("lost" | "restarted")) {
                    self.close_view(id)?;
                }
                if matches!(p["type"].as_str(), Some("finished" | "lost" | "restarted")) {
                    // In particular, `lost` can still be live with a pending Core
                    // restart. Read Core's facts rather than inventing lifecycle/status.
                    terminal = self
                        .core
                        .request("terminal.get", json!({"terminal":address}))?;
                }
                self.reconcile(terminal, seq)?;
            }
        } else if let Some(id) = self.channels.get(&frame.channel).copied() {
            let cells = self.cells;
            let visible = self.views.get(&id).is_some_and(|view| view.visible);
            if let Some(view) = self.views.get_mut(&id) {
                let outcome = view::apply_frame(view, id, frame.kind, &frame.payload, cells)?;
                match outcome {
                    Some(view::FrameOutcome::Output(result)) => {
                        super::super::effects::append(&result, &mut messages);
                        for response in result.terminal_responses {
                            self.core.send(view.channel, 6, &response)?;
                        }
                        self.dirty |= result.request_render;
                    }
                    Some(view::FrameOutcome::Changed) => self.dirty |= visible,
                    None => {}
                }
            }
        }
        self.core.credit(&frame)?;
        Ok(messages)
    }

    fn reconcile(&mut self, facts: Value, seq: u64) -> io::Result<()> {
        let address = facts["address"]
            .as_str()
            .ok_or_else(|| invalid("terminal event lacks address"))?;
        let Some(id) = self.model.id(address) else {
            return Ok(());
        };
        let ready = facts["state"] == "live" && facts["pid"].as_u64().is_some();
        let completed = super::super::model::program_completed(&facts);
        let record = self
            .model
            .records
            .get_mut(&id)
            .ok_or_else(|| invalid("Core record missing"))?;
        record.facts = facts;
        record.seq = seq;
        self.model.revision += 1;
        self.snapshot_dirty = true;
        if !ready {
            if let Some(view) = self.views.get_mut(&id) {
                view.closed = true;
                self.dirty |= view.visible;
            }
            if self.last_focus == Some(id) {
                self.last_focus = None;
            }
        } else if self.views.get(&id).is_none_or(|v| v.closed) {
            self.attach(id)?;
            self.resize(self.size, self.cells.0, self.cells.1)?;
            self.focus()?;
        }
        if completed {
            self.retire_completed_programs()?;
        }
        Ok(())
    }
}
