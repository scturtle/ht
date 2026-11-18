//! Projection and scoped edits of the desktop App's Core document.
mod creation;
mod editing;
pub(super) use super::shared::DocumentRpc;
use super::{
    model::{invalid, program_completed, Model, MAX_PANES},
    organization::{Organization, Tab, Workspace},
    shared::{hydrate, projection_boot},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    io,
};

pub(super) const DOCUMENT: &str = "herdr-app";

pub(super) fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "this action is not implemented in ht",
    )
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Document {
    spaces: Vec<Space>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Space {
    id: String,
    name: String,
    tabs: Vec<AppTab>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AppTab {
    id: String,
    name: String,
    root: Value,
}

fn visit_terminals<'a>(
    node: &'a Value,
    visit: &mut impl FnMut(&'a str, &Value) -> io::Result<()>,
) -> io::Result<()> {
    match node["type"].as_str() {
        Some("block") => {
            if let Some(address) = node["content"]["terminal"].as_str() {
                visit(address, node)?;
            }
        }
        Some("row" | "column" | "stack") => {
            if let Some(children) = node["children"].as_array() {
                for child in children {
                    visit_terminals(child, visit)?;
                }
            }
        }
        _ => {} // Mods and unknown nodes aren't terminal panes.
    }
    Ok(())
}

fn project(doc: &Document) -> io::Result<Organization<Vec<String>>> {
    let mut org = Organization {
        workspaces: Vec::new(),
    };
    let mut seen = HashSet::new();
    for space in &doc.spaces {
        let mut tabs = Vec::new();
        for tab in &space.tabs {
            let mut leaves = Vec::new();
            visit_terminals(&tab.root, &mut |address, _| {
                // A PTY appears at most once. The block ID is not its address.
                if seen.insert(address) {
                    if seen.len() > MAX_PANES {
                        return Err(invalid("herdr-app layout has too many terminals"));
                    }
                    leaves.push(address.to_owned());
                }
                Ok(())
            })?;
            if leaves.is_empty() {
                continue;
            }
            tabs.push(Tab {
                id: tab.id.clone(),
                label: tab.name.clone(),
                panes: leaves,
            });
        }
        if !tabs.is_empty() {
            org.workspaces.push(Workspace {
                id: space.id.clone(),
                label: space.name.clone(),
                tabs,
            });
        }
    }
    org.validate()?;
    Ok(org)
}

pub(super) fn read(core: &mut impl DocumentRpc) -> io::Result<(u64, Value)> {
    let mut result = core.request("doc.get", json!({"name":DOCUMENT}))?;
    let version = result["version"]
        .as_u64()
        .ok_or_else(|| invalid("herdr-app document lacks version"))?;
    Ok((version, result["value"].take()))
}

fn decode(value: Value) -> io::Result<Document> {
    if value.is_null() {
        Ok(Document::default())
    } else {
        Ok(serde_json::from_value(value)?)
    }
}

pub(super) fn array_field<'a>(value: &'a mut Value, field: &str) -> io::Result<&'a mut Vec<Value>> {
    value
        .as_object_mut()
        .ok_or_else(|| invalid("expected an object in the App layout"))?
        .entry(field)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| invalid(format!("expected an App layout array for {field}")))
}

pub(super) fn validate_value(value: &Value) -> io::Result<()> {
    project(&decode(value.clone())?)?;
    Ok(())
}

pub(super) fn write_version(
    core: &mut impl DocumentRpc,
    version: u64,
    value: &Value,
) -> io::Result<u64> {
    let next = version
        .checked_add(1)
        .ok_or_else(|| invalid("App layout version exhausted"))?;
    let reply = core.request(
        "doc.set",
        json!({"name":DOCUMENT,"version":version,"value":value}),
    )?;
    if reply["version"].as_u64() != Some(next) {
        return Err(invalid("unexpected App layout version after edit"));
    }
    Ok(next)
}

/// Apply a pure edit to the latest version. Retry only definitive conflicts;
/// an uncertain reply cannot safely be replayed.
fn edit_document(
    core: &mut impl DocumentRpc,
    (mut version, mut value): (u64, Value),
    mut edit: impl FnMut(&mut Value) -> io::Result<bool>,
) -> io::Result<(u64, Value)> {
    for attempt in 0..4 {
        if !edit(&mut value)? {
            return Ok((version, value));
        }
        validate_value(&value)?;
        match write_version(core, version, &value) {
            Ok(next) => return Ok((next, value)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && attempt < 3 => {
                (version, value) = read(core)?;
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "App layout changed repeatedly; retry the action",
    ))
}

pub(super) fn terminal_ended(facts: &Value) -> bool {
    program_completed(facts) || facts["reason"] == "removed"
}

pub(super) fn close_terminal(core: &mut impl DocumentRpc, address: &str) -> io::Result<()> {
    // Re-check membership and protection in the current App layout. A stale
    // tab-bar button must not target a terminal that the App has parked.
    let (_, value) = read(core)?;
    let mut found = false;
    for space in decode(value)?.spaces {
        for tab in space.tabs {
            visit_terminals(&tab.root, &mut |terminal, node| {
                if terminal == address {
                    if node["closable"] == false {
                        return Err(invalid("the App marked this pane as not closable"));
                    }
                    found = true;
                }
                Ok(())
            })?;
        }
    }
    if !found {
        return Err(invalid("terminal is no longer in the App layout"));
    }
    end_terminal(core, address)
}

fn end_terminal(core: &mut impl DocumentRpc, address: &str) -> io::Result<()> {
    let check_address = |facts: &Value| {
        if facts["address"] == address {
            Ok(())
        } else {
            Err(invalid("Core terminal identity changed"))
        }
    };
    let params = json!({"terminal":address});
    let facts = match core.request("terminal.get", params.clone()) {
        Ok(facts) => facts,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    check_address(&facts)?;
    if terminal_ended(&facts) {
        return Ok(());
    }
    if facts["state"] != "live" || facts["pid"].as_u64().is_none() {
        return Err(invalid("Core terminal is not currently running"));
    }
    // Kill only the explicitly selected terminal. Never replay after an
    // uncertain reply; Core events or reattach can confirm completion later.
    let result = core.request("terminal.kill", params)?;
    check_address(&result)?;
    if !terminal_ended(&result) {
        return Err(io::Error::other(
            "Core has not confirmed that the terminal ended",
        ));
    }
    Ok(())
}

fn install(
    core: &mut impl DocumentRpc,
    model: &mut Model,
    value: Value,
    mut facts: HashMap<String, Value>,
) -> io::Result<()> {
    let projection = project(&decode(value)?)?;
    hydrate(core, model, &projection, &mut facts)?;
    model.install_projection(projection, &facts)?;
    let ended = model
        .records
        .iter()
        .filter_map(|(id, record)| terminal_ended(&record.facts).then_some(*id))
        .collect::<Vec<_>>();
    for id in ended {
        model.remove(id);
    }
    Ok(())
}

pub(super) fn bootstrap(
    core: &mut impl DocumentRpc,
    subscription: &Value,
) -> io::Result<(Model, u64)> {
    let terminals = subscription["terminals"]
        .as_array()
        .ok_or_else(|| invalid("Core subscription lacks terminals"))?;
    let facts = terminals
        .iter()
        .filter_map(|v| v["address"].as_str().map(|a| (a.to_owned(), v.clone())))
        .collect();
    let mut model = Model::new(projection_boot()?);
    let (version, value) = read(core)?;
    install(core, &mut model, value, facts)?;
    Ok((model, version))
}

pub(super) fn refresh(
    core: &mut impl DocumentRpc,
    model: &mut Model,
    version: &mut u64,
) -> io::Result<bool> {
    let (next, value) = read(core)?;
    if next == *version {
        return Ok(false);
    }
    install(core, model, value, HashMap::new())?;
    *version = next;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    pub(super) struct Rpc(pub(super) VecDeque<(&'static str, Value, io::Result<Value>)>);

    impl DocumentRpc for Rpc {
        fn request(&mut self, method: &str, params: Value) -> io::Result<Value> {
            let (expected_method, expected_params, reply) = self.0.pop_front().unwrap();
            assert_eq!(method, expected_method);
            assert_eq!(params, expected_params);
            reply
        }
    }

    fn document(root: Value) -> (&'static str, Value, io::Result<Value>) {
        (
            "doc.get",
            json!({"name":DOCUMENT}),
            Ok(json!({"version":1,"value":{"spaces":[{"tabs":[{"root":root}]}]}})),
        )
    }

    #[test]
    fn app_projection_uses_tree_order_and_preserves_local_focus() {
        let block = |address: &str| json!({"type":"block","content":{"terminal":address}});
        let tab = |root: Value| json!({"id":"tab","root":root});
        let doc = |version, tabs: Vec<Value>| {
            (
                "doc.get",
                json!({"name":DOCUMENT}),
                Ok(json!({"version":version,"value":{"spaces":[{"id":"space","tabs":tabs}]}})),
            )
        };
        let other = json!({"id":"other","root":block("d")});
        let mut core = Rpc([
            doc(
                1,
                vec![
                    tab(json!({"type":"row","weights":[0.9,0.1],"children":[
                        block("a"),
                        {"type":"column","children":[
                            {"type":"block","content":{"mod":"notes"}},
                            {"type":"stack","children":[block("b"),block("c"),block("removed")]}
                        ]}
                    ]})),
                    other,
                ],
            ),
            doc(
                2,
                vec![
                    json!({"id":"other","root":{"type":"row","children":[block("d"),block("new")]}}),
                    tab(json!({"type":"stack","children":[block("c"),block("b"),block("a")]})),
                ],
            ),
            (
                "terminal.get",
                json!({"terminal":"new"}),
                Ok(json!({"address":"new","state":"live","cwd":"/new"})),
            ),
        ]
        .into());
        let terminals = ["a", "b", "c", "d", "removed"]
            .map(|address| json!({"address":address,"state":"live"}));
        let (mut model, mut version) =
            bootstrap(&mut core, &json!({"terminals":terminals})).unwrap();
        let order = |model: &Model| {
            model
                .snapshot()
                .panes
                .into_iter()
                .map(|p| p.pane_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&model), ["a", "b", "c", "removed", "d"]);

        let b = model.id("b").unwrap();
        let record = model.records.get_mut(&b).unwrap();
        record.facts["cwd"] = json!("/retained");
        record.seq = 41;
        model.focus(b).unwrap();
        assert!(refresh(&mut core, &mut model, &mut version).unwrap());
        assert_eq!(version, 2);
        assert!(core.0.is_empty());
        assert_eq!(order(&model), ["d", "new", "c", "b", "a"]);
        assert_eq!(model.id("b"), Some(b));
        assert_eq!(model.focused(), Some(b));
        assert_eq!(model.records[&b].facts["cwd"], "/retained");
        assert_eq!(model.records[&b].seq, 41);
        assert!(model.id("removed").is_none());
        assert_eq!(
            model.records[&model.id("new").unwrap()].facts["cwd"],
            "/new"
        );

        // The prior local focus wins on close, even when a different pane follows in bar order.
        let c = model.id("c").unwrap();
        model.focus(c).unwrap();
        model.focus(model.id("d").unwrap()).unwrap();
        model.focus_tab("tab").unwrap();
        assert_eq!(model.focused(), Some(c));
        model.remove(c);
        assert_eq!(model.focused(), Some(b));
        model.remove(b);
        assert_eq!(model.focused(), model.id("a"));
    }

    #[test]
    fn close_refuses_removed_or_protected_membership_and_aliases() {
        for root in [
            json!({"type":"block","content":{"terminal":"another"}}),
            json!({"type":"block","content":{"terminal":"t1"},"closable":false}),
        ] {
            let mut core = Rpc([document(root)].into());
            assert!(close_terminal(&mut core, "t1").is_err());
            assert!(core.0.is_empty());
        }
        let mut core = Rpc([
            document(json!({"type":"block","content":{"terminal":"t1"}})),
            (
                "terminal.get",
                json!({"terminal":"t1"}),
                Ok(json!({"address":"t2","state":"live","pid":1})),
            ),
        ]
        .into());
        assert!(close_terminal(&mut core, "t1").is_err());
        assert!(core.0.is_empty());
    }

    #[test]
    fn uncertain_close_is_not_replayed_or_assumed_complete() {
        let params = json!({"terminal":"t1"});
        let mut core = Rpc([
            document(json!({"type":"block","content":{"terminal":"t1"}})),
            (
                "terminal.get",
                params.clone(),
                Ok(json!({"address":"t1","state":"live","pid":1})),
            ),
            (
                "terminal.kill",
                params,
                Err(io::Error::from(io::ErrorKind::TimedOut)),
            ),
        ]
        .into());
        assert_eq!(
            close_terminal(&mut core, "t1").unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(core.0.is_empty());
    }
}
