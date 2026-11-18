//! Rename and reorder only the addressed App nodes; never serialize the TUI
//! projection back over fields or plugin tabs that it does not display.
use super::{
    array_field, decode, edit_document, end_terminal, install, read, visit_terminals, DocumentRpc,
};
use crate::{
    actions::{
        ActionResult, TabMoveParams, TabRenameParams, WorkspaceCloseParams, WorkspaceRenameParams,
    },
    client::core::{model::invalid, organization::validate_label, session::Session},
};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashSet},
    io,
};

fn rename(target: &mut Value, name: &str) -> io::Result<bool> {
    if target["name"].as_str().unwrap_or_default() == name {
        return Ok(false);
    }
    target
        .as_object_mut()
        .ok_or_else(|| invalid("invalid App layout node"))?
        .insert("name".into(), name.into());
    Ok(true)
}

fn find_workspace<'a>(value: &'a mut Value, id: &str) -> io::Result<&'a mut Value> {
    array_field(value, "spaces")?
        .iter_mut()
        .find(|space| space["id"] == id)
        .ok_or_else(|| invalid("App Space no longer exists"))
}

fn tab_list<'a>(value: &'a mut Value, tab_id: &str) -> io::Result<&'a mut Vec<Value>> {
    let spaces = array_field(value, "spaces")?;
    let space = spaces
        .iter_mut()
        .find(|space| {
            space["tabs"]
                .as_array()
                .is_some_and(|tabs| tabs.iter().any(|tab| tab["id"] == tab_id))
        })
        .ok_or_else(|| invalid("App tab no longer exists"))?;
    array_field(space, "tabs")
}

/// Resolve the tab-bar gap to a stable tab ID before a CAS can rebase. Hidden
/// plugin tabs are not part of the index; their contents and order stay intact.
fn move_tab(tabs: &mut Vec<Value>, id: &str, anchor: &str, after: bool) -> io::Result<bool> {
    let from = tabs
        .iter()
        .position(|tab| tab["id"] == id)
        .ok_or_else(|| invalid("App tab no longer exists"))?;
    let at = tabs
        .iter()
        .position(|tab| tab["id"] == anchor)
        .ok_or_else(|| invalid("tab reorder destination changed; retry the move"))?;
    let gap = at + usize::from(after);
    let to = gap - usize::from(from < gap);
    if from == to {
        return Ok(false);
    }
    let tab = tabs.remove(from);
    tabs.insert(to, tab);
    Ok(true)
}

/// Select only terminals exclusive to this Space. App placements in another
/// Space take precedence over an attempt to close the shared reference here.
fn exclusive_terminals(value: &Value, space_id: &str) -> io::Result<BTreeSet<String>> {
    let doc = decode(value.clone())?;
    let mut inside = BTreeSet::new();
    let mut outside = HashSet::new();
    let mut protected = HashSet::new();
    for space in &doc.spaces {
        for tab in &space.tabs {
            visit_terminals(&tab.root, &mut |address, node| {
                if space.id == space_id {
                    inside.insert(address.to_owned());
                    if node["closable"] == false {
                        protected.insert(address);
                    }
                } else {
                    outside.insert(address);
                }
                Ok(())
            })?;
        }
    }
    inside.retain(|address| !outside.contains(address.as_str()));
    if inside
        .iter()
        .any(|address| protected.contains(address.as_str()))
    {
        return Err(invalid(
            "this Space contains a pane the App marked as not closable",
        ));
    }
    Ok(inside)
}

fn close_workspace(core: &mut impl DocumentRpc, space_id: &str) -> io::Result<(u64, Value)> {
    let (_, value) = read(core)?;
    let planned = exclusive_terminals(&value, space_id)?;
    let mut ended = BTreeSet::new();
    for address in &planned {
        // App may have moved a pane while a preceding kill was in progress.
        let (_, latest) = read(core)?;
        let members = exclusive_terminals(&latest, space_id)?;
        if !members.is_subset(&planned) {
            return Err(invalid(
                "Space gained terminals during close; retry the action",
            ));
        }
        if members.contains(address) {
            end_terminal(core, address).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("could not end terminal {address}; Space was not removed: {error}"),
                )
            })?;
            ended.insert(address.clone());
        }
    }
    // Keep the layout intact until Core confirmed termination. Removing it
    // first would let the App re-adopt the still-running programs by label.
    let current = read(core)?;
    edit_document(core, current, |value| {
        if !exclusive_terminals(value, space_id)?.is_subset(&ended) {
            return Err(invalid(
                "Space changed during close; remaining terminals were left running",
            ));
        }
        if value.is_null() {
            return Ok(false);
        }
        let spaces = array_field(value, "spaces")?;
        let Some(index) = spaces.iter().position(|space| space["id"] == space_id) else {
            return Ok(false); // The App already reconciled the ended terminals.
        };
        spaces.remove(index);
        Ok(true)
    })
}

impl Session {
    fn install_app_edit(&mut self, (version, value): (u64, Value)) -> io::Result<()> {
        if self.document_version != version {
            install(&mut self.core, &mut self.model, value, Default::default())?;
            self.document_version = version;
            self.dirty = true;
            self.snapshot_dirty = true;
        }
        Ok(())
    }

    fn edit_app_layout(
        &mut self,
        edit: impl FnMut(&mut Value) -> io::Result<bool>,
    ) -> io::Result<()> {
        let current = read(&mut self.core)?;
        let edited = edit_document(&mut self.core, current, edit)?;
        self.install_app_edit(edited)
    }

    pub(in crate::client::core) fn rename_app_tab(
        &mut self,
        p: TabRenameParams,
    ) -> io::Result<ActionResult> {
        let name = p.label.trim();
        validate_label(name)?;
        self.edit_app_layout(|value| {
            let tab = tab_list(value, &p.tab_id)?
                .iter_mut()
                .find(|tab| tab["id"] == p.tab_id)
                .ok_or_else(|| invalid("App tab no longer exists"))?;
            rename(tab, name)
        })?;
        Ok(ActionResult::TabInfo {
            tab: self.model.tab_info(&p.tab_id)?,
        })
    }

    pub(in crate::client::core) fn move_app_tab(
        &mut self,
        p: TabMoveParams,
    ) -> io::Result<ActionResult> {
        let (workspace, from) = self
            .model
            .organization
            .workspaces
            .iter()
            .find_map(|space| {
                let index = space.tabs.iter().position(|tab| tab.id == p.tab_id)?;
                Some((space, index))
            })
            .ok_or_else(|| invalid("App tab no longer exists"))?;
        let tabs = &workspace.tabs;
        let gap = p.insert_index.min(tabs.len());
        // Adjacent gaps must not move the source past a hidden plugin tab.
        if gap != from && gap != from + 1 {
            let (anchor, after) = if let Some(next) = tabs.get(gap) {
                (&next.id, false)
            } else {
                (&tabs[gap - 1].id, true)
            };
            let anchor = anchor.clone();
            let space_id = workspace.id.clone();
            self.edit_app_layout(|value| {
                move_tab(
                    array_field(find_workspace(value, &space_id)?, "tabs")?,
                    &p.tab_id,
                    &anchor,
                    after,
                )
            })?;
        }
        Ok(ActionResult::TabInfo {
            tab: self.model.tab_info(&p.tab_id)?,
        })
    }

    pub(in crate::client::core) fn rename_app_workspace(
        &mut self,
        p: WorkspaceRenameParams,
    ) -> io::Result<ActionResult> {
        let name = p.label.trim();
        validate_label(name)?;
        self.edit_app_layout(|value| rename(find_workspace(value, &p.workspace_id)?, name))?;
        Ok(ActionResult::WorkspaceInfo {
            workspace: self.model.workspace_info(&p.workspace_id)?,
        })
    }

    pub(in crate::client::core) fn close_app_workspace(
        &mut self,
        p: WorkspaceCloseParams,
    ) -> io::Result<ActionResult> {
        let edited = close_workspace(&mut self.core, &p.workspace_id)?;
        self.install_app_edit(edited)?;
        Ok(ActionResult::Ok {})
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{tests::Rpc, DOCUMENT},
        *,
    };
    use serde_json::json;

    fn terminal(id: &str) -> Value {
        json!({"id":format!("tab-{id}"),"root":{"type":"block","content":{"terminal":id}}})
    }

    fn read_reply(version: u64, document: &Value) -> (&'static str, Value, io::Result<Value>) {
        (
            "doc.get",
            json!({"name":DOCUMENT}),
            Ok(json!({"version":version,"value":document})),
        )
    }

    #[test]
    fn tab_move_preserves_nodes_and_anchors_past_hidden_tabs() {
        let a = terminal("a");
        let b = terminal("b");
        let c = terminal("c");
        let hidden =
            json!({"id":"mod","root":{"type":"block","content":{"mod":"notes"},"future":123}});
        let mut tabs = vec![a.clone(), hidden.clone(), b.clone(), c.clone()];
        assert!(move_tab(&mut tabs, "tab-a", "tab-c", false).unwrap());
        assert_eq!(tabs, [hidden.clone(), b.clone(), a.clone(), c.clone()]);
        assert!(!move_tab(&mut tabs, "tab-a", "tab-c", false).unwrap());
        assert!(move_tab(&mut tabs, "tab-a", "tab-c", true).unwrap());
        assert_eq!(tabs, [hidden, b, c, a]);
        assert!(move_tab(&mut tabs, "tab-a", "unavailable", false).is_err());
    }

    #[test]
    fn closing_space_ends_only_exclusive_terminals_still_in_that_space() {
        let initial = json!({"spaces":[
            {"id":"target","tabs":[terminal("a"),terminal("b"),terminal("shared")]},
            {"id":"other","tabs":[terminal("shared")],"future":{"keep":true}}
        ]});
        let mut moved = initial.clone();
        let moved_tab = moved["spaces"][0]["tabs"].as_array_mut().unwrap().remove(1);
        moved["spaces"][1]["tabs"]
            .as_array_mut()
            .unwrap()
            .push(moved_tab);
        let mut expected = moved.clone();
        expected["spaces"].as_array_mut().unwrap().remove(0);
        let params = json!({"terminal":"a"});
        let mut core = Rpc([
            read_reply(1, &initial),
            read_reply(1, &initial),
            (
                "terminal.get",
                params.clone(),
                Ok(json!({"address":"a","state":"live","pid":7})),
            ),
            (
                "terminal.kill",
                params,
                Ok(json!({"address":"a","state":"finished","reason":"killed"})),
            ),
            read_reply(2, &moved),
            read_reply(2, &moved),
            (
                "doc.set",
                json!({"name":DOCUMENT,"version":2,"value":expected}),
                Ok(json!({"version":3})),
            ),
        ]
        .into());
        assert_eq!(close_workspace(&mut core, "target").unwrap(), (3, expected));
        assert!(core.0.is_empty());
    }

    #[test]
    fn closing_space_does_not_end_new_members_after_confirming_original_set() {
        let initial = json!({"spaces":[{"id":"target","tabs":[terminal("a")]}]});
        let changed = json!({"spaces":[{"id":"target","tabs":[terminal("a"),terminal("b")]}]});
        let mut core = Rpc([read_reply(1, &initial), read_reply(2, &changed)].into());
        assert!(close_workspace(&mut core, "target").is_err());
        assert!(core.0.is_empty());
    }
}
