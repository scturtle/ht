//! App tab/space creation spawns once, then publishes via a layout CAS.
use super::{array_field, decode, edit_document, install, project, read, DocumentRpc};
use crate::{
    actions::{ActionResult, PaneInfo, TabCreateParams, TabInfo, WorkspaceCreateParams},
    client::core::{
        model::{invalid, MAX_PANES},
        organization::validate_label,
        session::Session,
    },
};
use serde_json::{json, Value};
use std::{
    collections::{hash_map::RandomState, HashMap},
    hash::{BuildHasher, Hasher},
    io,
};

fn id(prefix: &str) -> String {
    let random = || RandomState::new().build_hasher().finish();
    format!("{prefix}{:016x}{:016x}", random(), random())
}

struct NewTab {
    workspace: String,
    new_workspace: Option<NewSpace>,
    tab: String,
    pane: String,
    name: String,
}

struct NewSpace {
    name: String,
    only_if_empty: bool,
}

fn validate_creation(value: &Value, name: &str) -> io::Result<()> {
    validate_label(name)?;
    let projection = project(&decode(value.clone())?)?;
    if projection.addresses().count() >= MAX_PANES {
        return Err(invalid("Core TUI pane limit reached"));
    }
    Ok(())
}

impl NewTab {
    fn plan(value: &Value, workspace: Option<&str>, name: String) -> io::Result<Self> {
        validate_creation(value, &name)?;
        let spaces = value["spaces"].as_array().map_or(&[][..], Vec::as_slice);
        let existing = match workspace {
            Some(workspace) => Some(
                spaces
                    .iter()
                    .find(|space| space["id"] == workspace)
                    .ok_or_else(|| invalid("workspace is no longer in the App layout"))?,
            ),
            None => spaces.first(),
        };
        let workspace = match existing {
            Some(space) => space["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| invalid("App workspace has no ID"))?
                .to_owned(),
            None => id("sp"),
        };
        let new_workspace = existing.is_none().then(|| NewSpace {
            name: String::new(),
            only_if_empty: true,
        });
        Ok(Self::new(workspace, new_workspace, name))
    }

    fn new(workspace: String, new_workspace: Option<NewSpace>, name: String) -> Self {
        Self {
            workspace,
            new_workspace,
            tab: id("tb"),
            pane: id("p"),
            name,
        }
    }

    fn append(&self, value: &mut Value, address: &str, folder: &str) -> io::Result<()> {
        if value.is_null() {
            *value = json!({});
        }
        let spaces = array_field(value, "spaces")?;
        let space = if let Some(new) = &self.new_workspace {
            // Do not overwrite a concurrently created workspace or silently
            // move the tab to someone else's workspace.
            if new.only_if_empty && !spaces.is_empty() {
                return Err(invalid("App layout gained a workspace; retry New Tab"));
            }
            let at = spaces.len();
            spaces.push(json!({"id":self.workspace,"name":new.name,"folder":folder,"tabs":[]}));
            &mut spaces[at]
        } else {
            spaces
                .iter_mut()
                .find(|space| space["id"] == self.workspace)
                .ok_or_else(|| invalid("workspace is no longer in the App layout"))?
        };
        let tabs = array_field(space, "tabs")?;
        tabs.push(json!({"id":self.tab,"name":self.name,"root":{
            "id":self.tab,"type":"row","children":[{
                "id":self.pane,"type":"block","content":{"terminal":address}
            }]
        }}));
        Ok(())
    }
}

fn publish(
    core: &mut impl DocumentRpc,
    current: (u64, Value),
    plan: &NewTab,
    address: &str,
    folder: &str,
) -> io::Result<(u64, Value)> {
    edit_document(core, current, |value| {
        plan.append(value, address, folder)?;
        Ok(true)
    })
}

impl Session {
    pub(in crate::client::core) fn seed_app_layout(&mut self) -> io::Result<()> {
        let current = read(&mut self.core)?;
        if current.0 != 0 || !current.1.is_null() {
            return self.refresh_shared().map(|_| ());
        }
        let plan = NewTab::plan(&current.1, None, String::new())?;
        let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
        self.spawn_app_tab(current, plan, Some(cwd), HashMap::new(), true)?;
        Ok(())
    }

    pub(in crate::client::core) fn create_app_tab(
        &mut self,
        p: TabCreateParams,
    ) -> io::Result<ActionResult> {
        let current = read(&mut self.core)?;
        let plan = NewTab::plan(
            &current.1,
            p.workspace_id.as_deref(),
            p.label.unwrap_or_default(),
        )?;
        let cwd = p
            .cwd
            .or_else(|| self.app_spawn_cwd(&current.1, &plan.workspace));
        let (tab, root_pane) = self.spawn_app_tab(current, plan, cwd, p.env, p.focus)?;
        Ok(ActionResult::TabCreated { tab, root_pane })
    }

    pub(in crate::client::core) fn create_app_workspace(
        &mut self,
        p: WorkspaceCreateParams,
    ) -> io::Result<ActionResult> {
        let current = read(&mut self.core)?;
        let name = p.label.unwrap_or_default();
        validate_creation(&current.1, &name)?;
        if let Some(source) = &p.source_workspace_id {
            self.model.workspace_info(source)?;
        }
        let source = p
            .source_workspace_id
            .as_deref()
            .unwrap_or(&self.model.active_workspace);
        let cwd = p.cwd.or_else(|| self.app_spawn_cwd(&current.1, source));
        let plan = NewTab::new(
            id("sp"),
            Some(NewSpace {
                name,
                only_if_empty: false,
            }),
            String::new(),
        );
        let (tab, root_pane) = self.spawn_app_tab(current, plan, cwd, p.env, p.focus)?;
        Ok(ActionResult::WorkspaceCreated {
            workspace: Box::new(self.model.workspace_info(&tab.workspace_id)?),
            tab,
            root_pane,
        })
    }

    fn app_spawn_cwd(&self, value: &Value, workspace: &str) -> Option<String> {
        self.cwd_for_workspace(workspace).or_else(|| {
            value["spaces"]
                .as_array()?
                .iter()
                .find(|space| space["id"] == workspace)?["folder"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
    }

    fn spawn_app_tab(
        &mut self,
        current: (u64, Value),
        plan: NewTab,
        cwd: Option<String>,
        env: HashMap<String, String>,
        focus: bool,
    ) -> io::Result<(TabInfo, PaneInfo)> {
        // Until the layout is confirmed, do not advertise the placement via
        // labels: the desktop App would independently adopt the new shell.
        let facts = self.run_terminal_with_labels(cwd, env, json!({}))?;
        let address = facts["address"]
            .as_str()
            .ok_or_else(|| invalid("new terminal lacks an address"))?
            .to_owned();
        let (version, value) = publish(
            &mut self.core,
            current,
            &plan,
            &address,
            facts["cwd"].as_str().unwrap_or_default(),
        )
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("new App tab not confirmed; terminal {address} was left running: {error}"),
            )
        })?;
        install(
            &mut self.core,
            &mut self.model,
            value,
            HashMap::from([(address.clone(), facts)]),
        )?;
        self.document_version = version;
        if focus {
            self.model.focus_tab(&plan.tab)?;
        }
        let labels = json!({"herdr.space":plan.workspace,"herdr.tab":plan.tab,"herdr.seen":"0"});
        if let Err(error) = self.core.request(
            "terminal.label",
            json!({"terminal":address,"labels":labels}),
        ) {
            tracing::warn!(%error, %address, "new App tab is published but placement labels could not be set");
        }
        Ok((
            self.model.tab_info(&plan.tab)?,
            self.pane_info(self.target(Some(&address))?)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{tests::Rpc, DOCUMENT},
        *,
    };

    #[test]
    fn create_rebases_only_the_append_on_a_version_conflict() {
        let initial = json!({"spaces":[{"id":"space","name":"original","tabs":[
            {"id":"plugin","root":{"id":"mod","type":"block","content":{"mod":"example"}}}
        ]}]});
        let plan = NewTab::plan(&initial, Some("space"), String::new()).unwrap();
        let mut latest = initial.clone();
        latest["spaces"][0]["name"] = json!("concurrent rename");
        latest["spaces"][0]["tabs"][0]["future_field"] = json!({"preserve":true});
        let mut first = initial.clone();
        plan.append(&mut first, "t8", "").unwrap();
        let mut expected = latest.clone();
        expected["spaces"][0]["tabs"]
            .as_array_mut()
            .unwrap()
            .push(first["spaces"][0]["tabs"][1].clone());
        let mut core = Rpc([
            (
                "doc.set",
                json!({"name":DOCUMENT,"version":1,"value":first}),
                Err(io::Error::from(io::ErrorKind::WouldBlock)),
            ),
            (
                "doc.get",
                json!({"name":DOCUMENT}),
                Ok(json!({"version":2,"value":latest})),
            ),
            (
                "doc.set",
                json!({"name":DOCUMENT,"version":2,"value":expected}),
                Ok(json!({"version":3})),
            ),
        ]
        .into());
        assert_eq!(
            publish(&mut core, (1, initial), &plan, "t8", "").unwrap(),
            (3, expected)
        );
        assert!(core.0.is_empty());
    }

    #[test]
    fn create_does_not_retry_an_uncertain_write() {
        let plan = NewTab::plan(&Value::Null, None, String::new()).unwrap();
        let mut candidate = Value::Null;
        plan.append(&mut candidate, "t8", "/tmp").unwrap();
        let mut core = Rpc([(
            "doc.set",
            json!({"name":DOCUMENT,"version":0,"value":candidate}),
            Err(io::Error::from(io::ErrorKind::TimedOut)),
        )]
        .into());
        assert_eq!(
            publish(&mut core, (0, Value::Null), &plan, "t8", "/tmp")
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(core.0.is_empty());
    }
}
