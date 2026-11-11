use serde::{Deserialize, Serialize};

pub mod agents;
pub mod commands;
pub mod common;
pub mod integrations;
pub(crate) mod operations;
pub mod panes;
pub mod response;
pub mod server;
pub mod tabs;
pub mod workspaces;
pub mod worktrees;

pub use agents::*;
pub use commands::*;
pub use common::*;
pub use integrations::*;
pub use panes::*;
pub use response::*;
pub use server::*;
pub use tabs::*;
pub use workspaces::*;
pub use worktrees::*;

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub method: Method,
}

// Variant order and parameter types preserve the existing wire contract.
// The two flags are independent: presentation impact, then client-shell admission.
operations::define_operations! {
    ServerReloadConfig(EmptyParams) => "server.reload_config", true, true;
    ProductAnnouncementDismiss(ProductAnnouncementDismissParams) => "product_announcement.dismiss", true, true;
    ReleaseNotesDismiss(ReleaseNotesDismissParams) => "release_notes.dismiss", true, true;
    CommandInvoke(CommandInvokeParams) => "command.invoke", true, true;
    ClientShellSurfaceSet(ClientShellSurfaceSetParams) => "client_shell.surface.set", false, true;
    WorkspaceCreate(WorkspaceCreateParams) => "workspace.create", true, true;
    WorkspaceList(EmptyParams) => "workspace.list", false, false;
    WorkspaceGet(WorkspaceTarget) => "workspace.get", false, false;
    WorkspaceFocus(WorkspaceTarget) => "workspace.focus", true, true;
    WorkspaceRename(WorkspaceRenameParams) => "workspace.rename", true, true;
    WorkspaceMove(WorkspaceMoveParams) => "workspace.move", true, true;
    WorkspaceMoveBlock(WorkspaceMoveBlockParams) => "workspace.move_block", true, true;
    WorkspaceClose(WorkspaceCloseParams) => "workspace.close", true, true;
    WorktreeList(WorktreeListParams) => "worktree.list", false, true;
    WorktreeCreate(WorktreeCreateParams) => "worktree.create", true, true;
    WorktreeOpen(WorktreeOpenParams) => "worktree.open", true, true;
    WorktreeRemove(WorktreeRemoveParams) => "worktree.remove", true, true;
    TabCreate(TabCreateParams) => "tab.create", true, true;
    TabList(TabListParams) => "tab.list", false, false;
    TabGet(TabTarget) => "tab.get", false, false;
    TabFocus(TabTarget) => "tab.focus", true, true;
    TabRename(TabRenameParams) => "tab.rename", true, true;
    TabMove(TabMoveParams) => "tab.move", true, true;
    TabClose(TabTarget) => "tab.close", true, true;
    PaneSplit(PaneSplitParams) => "pane.split", true, true;
    PaneSwap(PaneSwapParams) => "pane.swap", true, true;
    PaneZoom(PaneZoomParams) => "pane.zoom", true, true;
    LayoutSetSplitRatio(LayoutSetSplitRatioParams) => "layout.set_split_ratio", true, true;
    PaneFocusDirection(PaneFocusDirectionParams) => "pane.focus_direction", true, true;
    PaneResize(PaneResizeParams) => "pane.resize", true, true;
    PaneScroll(PaneScrollParams) => "pane.scroll", true, true;
    PaneClear(PaneTarget) => "pane.clear", true, true;
    PaneEditScrollback(PaneTarget) => "pane.edit_scrollback", true, true;
    PaneSelectionRead(PaneSelectionReadParams) => "pane.selection.read", false, true;
    PaneCopyMotion(PaneCopyMotionParams) => "pane.copy_motion", false, true;
    PaneCopySearch(PaneCopySearchParams) => "pane.copy_search", false, true;
    PaneFocus(PaneTarget) => "pane.focus", true, true;
    PaneInputSet(PaneInputSetParams) => "pane.input.set", true, true;
    PaneLinkActivate(PaneLinkActivateParams) => "pane.link.activate", false, true;
    PaneLinkResolve(PaneLinkActivateParams) => "pane.link.resolve", false, true;
    PaneRename(PaneRenameParams) => "pane.rename", true, true;
    PaneClose(PaneTarget) => "pane.close", true, true;
    IntegrationList(EmptyParams) => "integration.list", false, true;
    IntegrationInstall(IntegrationInstallParams) => "integration.install", false, true;
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod endpoint_method_contract;
