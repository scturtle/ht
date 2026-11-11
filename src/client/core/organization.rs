//! Core-backed organization facts. No focus, viewport, process or PTY state.
use super::model::{invalid, MAX_PANES};
use crate::{
    api::schema as api,
    layout::{Node, PaneId},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashSet},
    io,
};

pub(super) const DOCUMENT: &str = "herdr-session-v1";
pub(super) const MAX_DOCUMENT_BYTES: usize = 512 * 1024;
const MAX_DEPTH: usize = 48;
const MAX_RETIRED: usize = 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Organization {
    pub schema: u8,
    pub next_workspace: usize,
    pub workspaces: Vec<Workspace>,
    /// Persistent close intent. Never infer completion from a lost kill reply.
    /// Conservatively retained: automatic GC requires a proven Core lifecycle contract.
    pub retiring: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Workspace {
    pub id: String,
    /// Stable public number, never reused. The API reports the workspace's
    /// *position* instead, exactly as the original did.
    pub number: usize,
    /// Custom name. Empty means "no custom name" and the name is derived from the
    /// root pane cwd, matching the original's `custom_name: Option<String>`.
    pub label: String,
    pub next_tab: usize,
    pub tabs: Vec<Tab>,
}

impl Workspace {
    /// A stored name is a custom name, exactly as in the original. Empty names and
    /// the generated text older adapters persisted are not custom names.
    pub fn is_custom_named(&self) -> bool {
        !(self.label.is_empty()
            || self.number == 1 && self.label == "Core"
            || self.label == format!("Workspace {}", self.number))
    }

    /// Original `Workspace::display_name_from_terminals`: a custom name wins,
    /// otherwise the name comes from the first tab's root pane cwd, and an
    /// unknown cwd falls back to "workspace".
    pub fn display_name(&self, root_cwd: Option<&str>) -> String {
        if self.is_custom_named() {
            return self.label.clone();
        }
        match root_cwd {
            Some(cwd) => crate::client::identity::derive_label_from_cwd(std::path::Path::new(cwd)),
            None => "workspace".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Tab {
    pub id: String,
    pub number: usize,
    pub label: String,
    /// Explicit naming intent for new documents. Absent on pre-alignment data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_label: Option<bool>,
    /// The pane this tab was created with. The original named a workspace after
    /// its first tab's *root pane*, which splits and swaps never change. Absent
    /// on pre-alignment data, where the leftmost leaf stands in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_terminal: Option<String>,
    pub root: Branch,
}

impl Tab {
    pub fn is_custom_named(&self) -> bool {
        self.custom_label.unwrap_or_else(|| {
            // Old adapters stored generated labels without their naming intent.
            // Keep the stored text intact; recognize only their exact defaults.
            let old_default = self.label.strip_prefix("Tab ").is_some_and(|number| {
                !number.starts_with('0')
                    && number.bytes().all(|b| b.is_ascii_digit())
                    && number.parse::<usize>().ok() == Some(self.number)
            });
            !(old_default || self.number == 1 && self.label == "Core terminals")
        })
    }

    pub fn display_name(&self, index: usize) -> String {
        crate::app::tab_display_name(self.is_custom_named().then_some(&self.label), index)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Branch {
    Pane {
        terminal: String,
    },
    Split {
        direction: api::SplitDirection,
        ratio: f32,
        first: Box<Branch>,
        second: Box<Branch>,
    },
}

impl Default for Organization {
    fn default() -> Self {
        Self {
            schema: 1,
            next_workspace: 1,
            workspaces: Vec::new(),
            retiring: BTreeSet::new(),
        }
    }
}

fn valid_text(text: &str, limit: usize) -> bool {
    !text.is_empty() && text.len() <= limit && !text.chars().any(char::is_control)
}

/// A custom name is optional: an empty one means "no custom name", and the
/// original stored exactly that (`custom_name: Option<String>`).
fn valid_name(text: &str, limit: usize) -> bool {
    text.len() <= limit && !text.chars().any(char::is_control)
}

pub(super) fn validate_label(label: Option<&str>) -> io::Result<()> {
    if label.is_some_and(|label| !label.is_empty() && !valid_text(label, 256)) {
        return Err(invalid(
            "invalid shared label (maximum 256 UTF-8 bytes, no controls)",
        ));
    }
    Ok(())
}

impl Branch {
    pub fn from_node(node: &Node, address: &impl Fn(PaneId) -> String) -> Self {
        match node {
            Node::Pane(id) => Self::Pane {
                terminal: address(*id),
            },
            Node::Split {
                direction,
                ratio,
                first,
                second,
            } => Self::Split {
                direction: if *direction == ratatui::layout::Direction::Horizontal {
                    api::SplitDirection::Right
                } else {
                    api::SplitDirection::Down
                },
                ratio: *ratio,
                first: Box::new(Self::from_node(first, address)),
                second: Box::new(Self::from_node(second, address)),
            },
        }
    }

    pub fn node(&self, id: &impl Fn(&str) -> io::Result<PaneId>) -> io::Result<Node> {
        Ok(match self {
            Self::Pane { terminal } => Node::Pane(id(terminal)?),
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => Node::Split {
                direction: if *direction == api::SplitDirection::Right {
                    ratatui::layout::Direction::Horizontal
                } else {
                    ratatui::layout::Direction::Vertical
                },
                ratio: *ratio,
                first: Box::new(first.node(id)?),
                second: Box::new(second.node(id)?),
            },
        })
    }

    /// Point a pane at a different terminal, keeping its place in the tree.
    pub fn repoint(&mut self, from: &str, to: &str) -> bool {
        match self {
            Self::Pane { terminal } => {
                if terminal != from {
                    return false;
                }
                *terminal = to.to_owned();
                true
            }
            Self::Split { first, second, .. } => {
                first.repoint(from, to) || second.repoint(from, to)
            }
        }
    }

    pub fn addresses<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Self::Pane { terminal } => out.push(terminal),
            Self::Split { first, second, .. } => {
                first.addresses(out);
                second.addresses(out);
            }
        }
    }

    fn validate<'a>(&'a self, depth: usize, seen: &mut HashSet<&'a str>) -> io::Result<()> {
        if depth > MAX_DEPTH {
            return Err(invalid("shared layout exceeds maximum depth"));
        }
        match self {
            Self::Pane { terminal } => {
                if !valid_text(terminal, 512) || !seen.insert(terminal) || seen.len() > MAX_PANES {
                    return Err(invalid(
                        "invalid, duplicate or excessive shared terminal membership",
                    ));
                }
            }
            Self::Split {
                ratio,
                first,
                second,
                ..
            } => {
                if !ratio.is_finite() || !(0.05..=0.95).contains(ratio) {
                    return Err(invalid("invalid shared split ratio"));
                }
                first.validate(depth + 1, seen)?;
                second.validate(depth + 1, seen)?;
            }
        }
        Ok(())
    }
}

impl Organization {
    /// Replace a program identity everywhere, including the root used for naming.
    pub fn replace_terminal(&mut self, from: &str, to: &str) -> bool {
        let mut replaced = false;
        for workspace in &mut self.workspaces {
            for tab in &mut workspace.tabs {
                replaced |= tab.root.repoint(from, to);
                if tab.root_terminal.as_deref() == Some(from) {
                    tab.root_terminal = Some(to.to_owned());
                }
            }
        }
        replaced
    }

    pub fn decode(value: serde_json::Value) -> io::Result<Self> {
        if serde_json::to_vec(&value)?.len() > MAX_DOCUMENT_BYTES {
            return Err(invalid("shared organization exceeds 512 KiB"));
        }
        let organization: Self = serde_json::from_value(value)?;
        organization.validate()?;
        Ok(organization)
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.schema != 1
            || self.next_workspace == 0
            || self.workspaces.len() > MAX_PANES
            || self.retiring.len() > MAX_RETIRED
        {
            return Err(invalid("unsupported or oversized shared organization"));
        }
        let mut workspaces = HashSet::new();
        let mut workspace_numbers = HashSet::new();
        let mut tabs = HashSet::new();
        let mut addresses = HashSet::new();
        for workspace in &self.workspaces {
            if !valid_text(&workspace.id, 128)
                || !workspaces.insert(&workspace.id)
                || !valid_name(&workspace.label, 256)
                || workspace.number == 0
                || workspace.number >= self.next_workspace
                || !workspace_numbers.insert(workspace.number)
                || workspace.tabs.is_empty()
                || workspace.tabs.len() > MAX_PANES
            {
                return Err(invalid("invalid shared workspace identity or counters"));
            }
            let mut tab_numbers = HashSet::new();
            for tab in &workspace.tabs {
                if !valid_text(&tab.id, 128)
                    || !tabs.insert(&tab.id)
                    || !valid_text(&tab.label, 256)
                    || tab.number == 0
                    || tab.number >= workspace.next_tab
                    || !tab_numbers.insert(tab.number)
                {
                    return Err(invalid("invalid shared tab identity or counters"));
                }
                tab.root.validate(0, &mut addresses)?;
            }
        }
        if tabs.len() > MAX_PANES
            || self
                .retiring
                .iter()
                .any(|a| !valid_text(a, 512) || addresses.contains(a.as_str()))
        {
            return Err(invalid("retired terminal cannot be published or adopted"));
        }
        if serde_json::to_vec(self)?.len() > MAX_DOCUMENT_BYTES {
            return Err(invalid("shared organization exceeds 512 KiB"));
        }
        Ok(())
    }

    pub fn addresses(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for workspace in &self.workspaces {
            for tab in &workspace.tabs {
                tab.root.addresses(&mut out);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Organization {
        Organization {
            schema: 1,
            next_workspace: 2,
            retiring: BTreeSet::new(),
            workspaces: vec![Workspace {
                id: "opaque/workspace".into(),
                number: 1,
                label: "Workspace".into(),
                next_tab: 2,
                tabs: vec![Tab {
                    id: "opaque/tab".into(),
                    number: 1,
                    label: "Tab".into(),
                    custom_label: None,
                    root_terminal: None,
                    root: Branch::Pane {
                        terminal: "remote/opaque/t01".into(),
                    },
                }],
            }],
        }
    }
    #[test]
    fn legacy_default_names_keep_stored_data_and_new_custom_intent_round_trips() {
        let mut doc = sample();
        let tab = &mut doc.workspaces[0].tabs[0];
        tab.label = "Core terminals".into();
        assert_eq!(tab.display_name(0), "1");
        assert_eq!(tab.label, "Core terminals");
        tab.label = "Tab 1".into();
        assert_eq!(tab.display_name(4), "5");
        tab.custom_label = Some(true);
        assert_eq!(
            tab.display_name(4),
            "Tab 1",
            "an explicit name is never inferred automatic"
        );
        assert_eq!(
            Organization::decode(serde_json::to_value(&doc).unwrap()).unwrap(),
            doc
        );
        doc.workspaces[0].tabs[0].custom_label = None;
        doc.workspaces[0].tabs[0].label = "notes".into();
        assert_eq!(doc.workspaces[0].tabs[0].display_name(4), "notes");
    }

    #[test]
    fn shared_document_contains_no_client_focus_or_viewport() {
        let doc = sample();
        let value = serde_json::to_value(&doc).unwrap();
        assert_eq!(Organization::decode(value.clone()).unwrap(), doc);
        for forbidden in ["focused", "focus", "scroll", "active_tab", "pane_id"] {
            assert!(!value.to_string().contains(forbidden));
        }
        let mut value = value;
        value["focused_workspace"] = "opaque/workspace".into();
        assert!(Organization::decode(value).is_err());
    }
    #[test]
    fn rejects_duplicates_retired_addresses_and_invalid_counters() {
        let mut doc = sample();
        doc.retiring.insert("remote/opaque/t01".into());
        assert!(doc.validate().is_err());
        doc.retiring.clear();
        let pane = doc.workspaces[0].tabs[0].root.clone();
        doc.workspaces[0].tabs[0].root = Branch::Split {
            direction: api::SplitDirection::Right,
            ratio: 0.5,
            first: Box::new(pane.clone()),
            second: Box::new(pane),
        };
        assert!(doc.validate().is_err());
        let mut doc = sample();
        doc.next_workspace = 1;
        assert!(doc.validate().is_err());
        let mut doc = sample();
        doc.workspaces[0].next_tab = 1;
        assert!(doc.validate().is_err());
    }
    #[test]
    fn rejects_nonfinite_ratios_and_unbounded_depth() {
        for ratio in [f32::NAN, f32::INFINITY, -0.1, 0.0, 1.0] {
            let mut doc = sample();
            doc.workspaces[0].tabs[0].root = Branch::Split {
                direction: api::SplitDirection::Right,
                ratio,
                first: Box::new(Branch::Pane {
                    terminal: "a".into(),
                }),
                second: Box::new(Branch::Pane {
                    terminal: "b".into(),
                }),
            };
            assert!(doc.validate().is_err());
        }
        let mut doc = sample();
        let mut root = Branch::Pane {
            terminal: "base".into(),
        };
        for i in 0..=MAX_DEPTH {
            root = Branch::Split {
                direction: api::SplitDirection::Down,
                ratio: 0.5,
                first: Box::new(root),
                second: Box::new(Branch::Pane {
                    terminal: format!("opaque/{i}"),
                }),
            };
        }
        doc.workspaces[0].tabs[0].root = root;
        assert!(doc.validate().is_err());
    }
}
