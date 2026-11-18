//! Displayable organization, without Core wire geometry or local focus fields.
use super::model::{invalid, MAX_PANES};
use std::{collections::HashSet, io};

pub(super) struct Organization<R> {
    pub workspaces: Vec<Workspace<R>>,
}

pub(super) struct Workspace<R> {
    pub id: String,
    pub label: String,
    pub tabs: Vec<Tab<R>>,
}

pub(super) struct Tab<R> {
    pub id: String,
    pub label: String,
    pub panes: R,
}

impl<R> Tab<R> {
    pub fn display_name(&self, position: usize) -> String {
        if self.label.is_empty() {
            (position + 1).to_string()
        } else {
            self.label.clone()
        }
    }
}

pub(super) fn valid_text(text: &str, limit: usize) -> bool {
    !text.is_empty() && text.len() <= limit && !text.chars().any(char::is_control)
}

pub(super) fn validate_label(label: &str) -> io::Result<()> {
    if !label.is_empty() && !valid_text(label, 256) {
        return Err(invalid(
            "invalid shared label (maximum 256 UTF-8 bytes, no controls)",
        ));
    }
    Ok(())
}

impl Organization<Vec<String>> {
    pub fn validate(&self) -> io::Result<()> {
        let mut spaces = HashSet::new();
        let mut tabs = HashSet::new();
        let mut addresses = HashSet::new();
        for space in &self.workspaces {
            validate_label(&space.label)?;
            if !valid_text(&space.id, 128) || !spaces.insert(&space.id) || space.tabs.is_empty() {
                return Err(invalid("invalid or duplicate App Space"));
            }
            for tab in &space.tabs {
                validate_label(&tab.label)?;
                if !valid_text(&tab.id, 128) || !tabs.insert(&tab.id) || tab.panes.is_empty() {
                    return Err(invalid("invalid or duplicate App tab"));
                }
                for address in &tab.panes {
                    if !valid_text(address, 512)
                        || !addresses.insert(address)
                        || addresses.len() > MAX_PANES
                    {
                        return Err(invalid(
                            "invalid, duplicate or excessive App terminal membership",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn addresses(&self) -> impl Iterator<Item = &str> {
        self.workspaces
            .iter()
            .flat_map(|w| &w.tabs)
            .flat_map(|t| t.panes.iter().map(String::as_str))
    }
}

impl<R> Organization<R> {
    /// Keep workspace and tab identity while resolving the panes to local IDs.
    pub fn resolve<T>(
        self,
        mut map: impl FnMut(&str, R) -> io::Result<T>,
    ) -> io::Result<Organization<T>> {
        let workspaces = self
            .workspaces
            .into_iter()
            .map(|space| {
                let tabs = space
                    .tabs
                    .into_iter()
                    .map(|tab| {
                        let panes = map(&tab.id, tab.panes)?;
                        Ok(Tab {
                            id: tab.id,
                            label: tab.label,
                            panes,
                        })
                    })
                    .collect::<io::Result<_>>()?;
                Ok(Workspace {
                    id: space.id,
                    label: space.label,
                    tabs,
                })
            })
            .collect::<io::Result<_>>()?;
        Ok(Organization { workspaces })
    }
}
