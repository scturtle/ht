use super::*;
use std::collections::VecDeque;

fn fact(address: &str) -> Value {
    json!({"address":address,"state":"live","labels":{"herdr":"tui"}})
}
struct Store {
    doc: Organization,
    version: u64,
    winners: VecDeque<Organization>,
    facts: HashMap<String, Value>,
    calls: Vec<String>,
    lose_reply: bool,
    reject_write: bool,
    null: bool,
    runs: usize,
    kills: Vec<String>,
    fail_kill: bool,
}
impl Store {
    fn new(model: &Model) -> Self {
        Self {
            doc: model.shared().unwrap(),
            version: 1,
            winners: VecDeque::new(),
            facts: HashMap::new(),
            calls: Vec::new(),
            lose_reply: false,
            reject_write: false,
            null: false,
            runs: 0,
            kills: Vec::new(),
            fail_kill: false,
        }
    }
}
impl DocumentRpc for Store {
    fn request(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.calls.push(method.into());
        match method {
            "doc.get" => Ok(
                json!({"name":DOCUMENT,"version":self.version,"value":if self.null { Value::Null } else { serde_json::to_value(&self.doc).unwrap() }}),
            ),
            "doc.set" => {
                if let Some(winner) = self.winners.pop_front() {
                    self.doc = winner;
                    self.version += 1;
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "another writer won",
                    ));
                }
                assert_eq!(params["version"], self.version);
                if self.reject_write {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "write did not reach server",
                    ));
                }
                self.doc = Organization::decode(params["value"].clone())?;
                self.version += 1;
                if self.lose_reply {
                    self.lose_reply = false;
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "committed reply lost",
                    ));
                }
                Ok(json!({"version":self.version}))
            }
            "terminal.get" => self
                .facts
                .get(params["terminal"].as_str().unwrap())
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "confirmed absent")),
            "terminal.run" => {
                self.runs += 1;
                let address = format!("repair-{}", self.runs);
                let record = json!({"address":address,"state":"live","pid":100 + self.runs,
                    "labels":params["labels"],"cwd":params["cwd"],
                    "cols":params["size"]["cols"],"rows":params["size"]["rows"]});
                self.facts.insert(address, record.clone());
                Ok(record)
            }
            "terminal.kill" => {
                let address = params["terminal"].as_str().unwrap();
                assert!(
                    self.doc.retiring.contains(address),
                    "kill requires a committed intent"
                );
                assert!(!self.doc.addresses().contains(&address));
                self.kills.push(address.into());
                if self.fail_kill {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "lost kill reply"));
                }
                self.facts.get_mut(address).unwrap()["state"] = json!("finished");
                Ok(json!({}))
            }
            other => panic!("unexpected transaction RPC: {other}"),
        }
    }
}
fn initial() -> Model {
    let mut model = Model::new("client-one".into());
    model.insert(fact("base")).unwrap();
    model
}

fn repair_fixture() -> (Store, Organization, HashMap<String, Value>) {
    let model = initial();
    let mut store = Store::new(&model);
    store.doc.workspaces[0].tabs[0].root_terminal = Some("base".into());
    let document = store.doc.clone();
    let facts = HashMap::from([(
        "base".into(),
        json!({"address":"base", "state":"finished",
        "labels":{"herdr":"tui"},"cwd":"/work/project", "cols":80,"rows":24}),
    )]);
    (store, document, facts)
}

fn repair(
    core: &mut Store,
    document: Organization,
    facts: &mut HashMap<String, Value>,
) -> io::Result<(Organization, u64)> {
    repair_finished_panes(
        core,
        document,
        facts,
        ClientSurfaceSize {
            cols: 120,
            rows: 40,
        },
        &["fish".into()],
        1,
    )
}

#[test]
fn each_rehydration_gets_a_fresh_projection_scope_without_restarting_core() {
    let first = projection_boot().unwrap();
    let second = projection_boot().unwrap();
    assert_ne!(first, second);
    assert!(first.starts_with(&format!("core-client-{}-", std::process::id())));
}

#[test]
fn finished_pane_repair_updates_layout_and_naming_root_in_place() {
    let (mut core, document, mut facts) = repair_fixture();
    let (doc, version) = repair(&mut core, document.clone(), &mut facts).unwrap();
    assert_eq!(version, 2);
    assert_eq!(core.runs, 1);
    assert!(core.kills.is_empty());
    let mut expected = document;
    assert!(expected.replace_terminal("base", "repair-1"));
    assert_eq!(doc, expected);
    assert_eq!(
        doc.workspaces[0].tabs[0].root_terminal.as_deref(),
        Some("repair-1")
    );
    assert_eq!(facts["repair-1"]["cwd"], "/work/project");
    assert_eq!(facts["repair-1"]["cols"], 80);
}

#[test]
fn repair_rebases_other_client_edits_without_spawning_twice() {
    let (mut core, document, mut facts) = repair_fixture();
    let mut winner = document.clone();
    winner.workspaces[0].label = "other client".into();
    core.winners.push_back(winner);
    let (doc, version) = repair(&mut core, document, &mut facts).unwrap();
    assert_eq!(version, 3);
    assert_eq!(core.runs, 1);
    assert_eq!(doc.workspaces[0].label, "other client");
    assert_eq!(doc.addresses(), ["repair-1"]);
}

#[test]
fn competing_repair_keeps_winner_and_retires_only_our_unused_shell() {
    for fail_kill in [false, true] {
        let (mut core, document, mut facts) = repair_fixture();
        let mut winner = document.clone();
        winner.replace_terminal("base", "foreign");
        core.facts.insert("foreign".into(), fact("foreign"));
        core.winners.push_back(winner.clone());
        core.fail_kill = fail_kill;
        let (doc, version) = repair(&mut core, document, &mut facts).unwrap();
        winner.retiring.insert("repair-1".into());
        assert_eq!(doc, winner);
        assert_eq!(version, 3);
        assert_eq!(core.runs, 1);
        assert_eq!(core.kills, ["repair-1"]);
        assert!(
            facts.contains_key("foreign"),
            "bootstrap must install freshly hydrated winner facts"
        );
    }
}

#[test]
fn concurrent_close_cannot_be_undone_by_bootstrap_repair() {
    let (mut core, document, mut facts) = repair_fixture();
    core.winners.push_back(Organization {
        workspaces: Vec::new(),
        ..document.clone()
    });
    let (doc, _) = repair(&mut core, document, &mut facts).unwrap();
    assert!(doc.workspaces.is_empty());
    assert_eq!(core.runs, 1);
    assert_eq!(core.kills, ["repair-1"]);
}

#[test]
fn repair_observes_a_winner_before_spawning() {
    let (mut core, document, mut facts) = repair_fixture();
    core.doc.replace_terminal("base", "foreign");
    core.facts.insert("foreign".into(), fact("foreign"));
    core.version = 2;
    let (doc, version) = repair(&mut core, document, &mut facts).unwrap();
    assert_eq!(doc, core.doc);
    assert_eq!(version, 2);
    assert_eq!(core.runs, 0);
    assert!(facts.contains_key("foreign"));
}

#[test]
fn repair_recognizes_a_confirmed_commit_after_its_reply_is_lost() {
    let (mut core, document, mut facts) = repair_fixture();
    core.lose_reply = true;
    let (doc, version) = repair(&mut core, document, &mut facts).unwrap();
    assert_eq!(version, 2);
    assert_eq!(doc.addresses(), ["repair-1"]);
    assert_eq!(core.runs, 1);
    assert!(core.kills.is_empty());
}

#[test]
fn uncertain_repair_never_kills_or_retries_a_possibly_published_program() {
    let (mut core, document, mut facts) = repair_fixture();
    core.reject_write = true;
    let error = repair(&mut core, document.clone(), &mut facts).unwrap_err();
    assert!(error.to_string().contains("repair-1"));
    assert_eq!(core.doc, document);
    assert_eq!(core.runs, 1);
    assert!(core.kills.is_empty());
}

#[test]
fn repeated_repair_contention_is_bounded_without_repeating_process_creation() {
    let (mut core, document, mut facts) = repair_fixture();
    for n in 0..4 {
        let mut winner = document.clone();
        winner.workspaces[0].label = format!("winner {n}");
        core.winners.push_back(winner);
    }
    let error = repair(&mut core, document, &mut facts).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert_eq!(core.runs, 1);
    assert!(core.kills.is_empty());
    assert_eq!(core.calls.iter().filter(|m| *m == "doc.set").count(), 4);
}

#[test]
fn semantic_noop_does_not_publish_or_dirty_the_client() {
    let mut model = initial();
    let mut core = Store::new(&model);
    let before = model.revision;
    let mut version = 1;
    transact(&mut core, &mut model, &mut version, |_| Ok(())).unwrap();
    assert_eq!(version, 1);
    assert_eq!(model.revision, before);
    assert_eq!(core.calls, ["doc.get"]);
}

#[test]
fn directional_swap_rebases_against_new_bsp_without_replaying_any_external_effect() {
    use crate::actions;
    let mut model = initial();
    model.insert(fact("second")).unwrap();
    model.insert(fact("third")).unwrap();
    let base = model.id("base").unwrap();
    model.focus(base).unwrap();
    model.zoom_pane(base, actions::PaneZoomMode::On).unwrap();
    let params = super::super::swap::pin_source(
        &model,
        actions::PaneSwapParams {
            direction: Some(actions::PaneDirection::Right),
            ..Default::default()
        },
    )
    .unwrap();
    let area = ratatui::layout::Rect::new(0, 0, 100, 28);
    let before = model.clone().swap_panes(&params, area).unwrap();
    let old_neighbor = model.id(before.target_pane_id.as_deref().unwrap()).unwrap();
    let other = model
        .layout()
        .unwrap()
        .pane_ids()
        .into_iter()
        .find(|id| *id != base && *id != old_neighbor)
        .unwrap();
    let expected = model.records[&other].address.clone();
    let mut winner = model.clone();
    let tab = winner.active_tab().unwrap().to_owned();
    winner
        .layouts
        .get_mut(&tab)
        .unwrap()
        .swap_panes(old_neighbor, other);
    let mut core = Store::new(&model);
    core.winners.push_back(winner.shared().unwrap());
    let mut version = 1;
    let result = transact(&mut core, &mut model, &mut version, |m| {
        m.swap_panes(&params, area)
    })
    .unwrap();
    assert!(result.changed && result.layout.zoomed);
    assert_eq!(result.target_pane_id.as_deref(), Some(expected.as_str()));
    assert_eq!(model.focused(), Some(base));
    assert_eq!(model.shared().unwrap(), core.doc);
    assert_eq!(core.calls.iter().filter(|m| *m == "doc.set").count(), 2);
}

#[test]
fn concurrent_neighbor_retirement_makes_swap_a_noop_without_publishing_identical_document() {
    use crate::actions;
    let mut model = initial();
    let neighbor = model.insert(fact("neighbor")).unwrap();
    let mut winner = model.clone();
    winner.retire(neighbor).unwrap();
    let mut core = Store::new(&model);
    core.winners.push_back(winner.shared().unwrap());
    let params = actions::PaneSwapParams {
        pane_id: Some("base".into()),
        direction: Some(actions::PaneDirection::Right),
        ..Default::default()
    };
    let mut version = 1;
    let result = transact(&mut core, &mut model, &mut version, |m| {
        m.swap_panes(&params, ratatui::layout::Rect::new(0, 0, 100, 28))
    })
    .unwrap();
    assert!(!result.changed);
    assert_eq!(result.reason, Some(actions::PaneSwapReason::NoNeighbor));
    assert_eq!(version, 2);
    assert_eq!(core.calls.iter().filter(|m| *m == "doc.set").count(), 1);
    assert!(model.id("neighbor").is_none());
}

#[test]
fn concurrent_source_retirement_never_redirects_a_pinned_swap_to_the_survivor() {
    use crate::actions;
    let mut model = initial();
    model.insert(fact("neighbor")).unwrap();
    let base = model.id("base").unwrap();
    model.focus(base).unwrap();
    let params = super::super::swap::pin_source(
        &model,
        actions::PaneSwapParams {
            direction: Some(actions::PaneDirection::Right),
            ..Default::default()
        },
    )
    .unwrap();
    let mut winner = model.clone();
    winner.retire(base).unwrap();
    let mut core = Store::new(&model);
    core.winners.push_back(winner.shared().unwrap());
    let mut version = 1;
    let error = transact(&mut core, &mut model, &mut version, |m| {
        m.swap_panes(&params, ratatui::layout::Rect::new(0, 0, 100, 28))
    })
    .unwrap_err();
    assert_eq!(
        super::super::swap::error_code(&error),
        Some("pane_not_found")
    );
    assert_eq!(model.records[&model.focused().unwrap()].address, "neighbor");
    assert_eq!(version, 2);
    assert_eq!(core.calls.iter().filter(|m| *m == "doc.set").count(), 1);
}

#[test]
fn completed_program_cleanup_rebases_without_replaying_effects_or_losing_new_tabs() {
    let mut model = initial();
    let mut other = model.clone();
    other
        .create_tab("core", fact("foreign"), Some("new tab".into()), true)
        .unwrap();
    let id = model.id("base").unwrap();
    model.records.get_mut(&id).unwrap().facts =
        json!({"address":"base", "state":"finished", "reason":"exited"});
    let mut core = Store::new(&model);
    core.winners.push_back(other.shared().unwrap());
    core.facts.insert("foreign".into(), fact("foreign"));
    let mut version = 1;
    transact(
        &mut core,
        &mut model,
        &mut version,
        Model::retire_completed_programs,
    )
    .unwrap();
    assert!(model.id("base").is_none());
    assert!(model.id("foreign").is_some());
    assert!(model.organization.retiring.contains("base"));
    assert_eq!(model.organization.workspaces[0].tabs[0].label, "new tab");
    assert_eq!(core.calls.iter().filter(|m| *m == "doc.set").count(), 2);
}

#[test]
fn semantic_cas_rebases_concurrent_creation_without_losing_either_tab_or_local_focus() {
    let mut model = initial();
    let original = model.focused().unwrap();
    let mut other = model.clone();
    let (foreign, _) = other
        .create_tab("core", fact("foreign"), Some("other client".into()), true)
        .unwrap();
    let mut core = Store::new(&model);
    core.facts.insert("foreign".into(), fact("foreign"));
    core.winners.push_back(other.shared().unwrap());
    let mut version = 1;
    let (mine, id) = transact(&mut core, &mut model, &mut version, |candidate| {
        candidate.create_tab("core", fact("mine"), Some("this client".into()), false)
    })
    .unwrap();
    assert_eq!(foreign, "core.t2");
    assert_eq!(mine, "core.t3");
    assert_eq!(model.records[&id].address, "mine");
    assert_eq!(model.focused(), Some(original));
    assert_eq!(model.organization.workspaces[0].tabs.len(), 3);
    assert_eq!(model.shared().unwrap(), core.doc);
    assert_eq!(version, 3);
    assert_eq!(core.calls.iter().filter(|m| *m == "doc.set").count(), 2);
}

#[test]
fn rejected_candidate_does_not_publish_mutation_or_retirement() {
    let mut model = initial();
    let before = model.shared().unwrap();
    let mut core = Store::new(&model);
    let mut version = 1;
    let result: io::Result<()> = transact(&mut core, &mut model, &mut version, |candidate| {
        candidate.close_tab("core.1")?;
        Err(invalid("deliberate validation rejection"))
    });
    assert!(result.is_err());
    assert_eq!(version, 1);
    assert_eq!(model.shared().unwrap(), before);
    assert_eq!(core.calls, ["doc.get"]);
}

#[test]
fn ambiguous_commit_is_refetched_without_undo_or_program_termination() {
    let mut model = initial();
    let mut core = Store::new(&model);
    core.lose_reply = true;
    core.facts.insert("mine".into(), fact("mine"));
    let mut version = 1;
    let error = transact(&mut core, &mut model, &mut version, |candidate| {
        candidate.create_tab("core", fact("mine"), None, false)
    })
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(version, 2);
    assert!(model.id("mine").is_some());
    assert_eq!(model.shared().unwrap(), core.doc);
    assert_eq!(core.calls.iter().filter(|m| *m == "doc.set").count(), 1);
}

#[test]
fn timeout_before_commit_never_installs_speculative_candidate() {
    let mut model = initial();
    let before = model.shared().unwrap();
    let mut core = Store::new(&model);
    core.reject_write = true;
    let mut version = 1;
    assert!(
        transact(&mut core, &mut model, &mut version, |candidate| candidate
            .create_tab("core", fact("orphan"), None, true))
        .is_err()
    );
    assert_eq!(model.shared().unwrap(), before);
    assert!(model.id("orphan").is_none());
    assert_eq!(version, 1);
}

#[test]
fn contention_is_bounded_and_final_refresh_keeps_only_confirmed_membership() {
    let mut model = initial();
    let mut core = Store::new(&model);
    for number in 0..4 {
        let mut winner = core.doc.clone();
        winner.workspaces[0].label = format!("external {number}");
        core.winners.push_back(winner);
    }
    let mut version = 1;
    let error = transact(&mut core, &mut model, &mut version, |candidate| {
        candidate.create_tab("core", fact("orphan"), None, false)
    })
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert_eq!(core.calls.iter().filter(|m| *m == "doc.set").count(), 4);
    assert_eq!(version, 5);
    assert!(model.id("orphan").is_none());
    assert_eq!(model.shared().unwrap(), core.doc);
}

#[test]
fn explicit_terminal_absence_retains_unknown_nonrunning_placeholder() {
    let mut model = initial();
    let mut other = model.clone();
    other
        .create_tab("core", fact("removed"), None, false)
        .unwrap();
    let mut core = Store::new(&other);
    core.version = 2;
    let mut version = 1;
    assert!(refresh(&mut core, &mut model, &mut version).unwrap());
    let record = &model.records[&model.id("removed").unwrap()];
    assert_eq!(record.facts["reason"], "removed");
    assert_eq!(
        super::super::model::status(&record.facts),
        crate::actions::AgentStatus::Unknown
    );
    assert_ne!(record.facts["state"], "live");
}

#[test]
fn foreign_terminal_hydration_and_backwards_versions_are_atomic_failures() {
    let mut model = initial();
    let before = model.shared().unwrap();
    let mut other = model.clone();
    other
        .create_tab("core", fact("foreign"), None, false)
        .unwrap();
    let mut core = Store::new(&other);
    core.version = 2;
    core.facts.insert(
        "foreign".into(),
        json!({"address":"foreign","state":"live","labels":{}}),
    );
    let mut version = 1;
    assert!(refresh(&mut core, &mut model, &mut version).is_err());
    assert_eq!(version, 1);
    assert_eq!(model.shared().unwrap(), before);
    core.version = 0;
    assert!(refresh(&mut core, &mut model, &mut version).is_err());
    assert_eq!(version, 1);
    assert_eq!(model.shared().unwrap(), before);
}

#[test]
fn versioned_null_document_cannot_reseed_or_readopt_orphans() {
    let mut core = Store::new(&initial());
    core.null = true;
    assert!(read(&mut core).is_err());
    core.version = 0;
    assert!(read(&mut core).unwrap().1.is_none());
    assert!(core.calls.iter().all(|method| method == "doc.get"));
}
