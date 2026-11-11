use serde::{Deserialize, Serialize};

use super::integrations::{
    IntegrationInstallResult, IntegrationTarget, IntegrationUninstallResult,
};
use super::panes::{
    LayoutDescription, PaneEdgesResult, PaneFocusDirectionResult, PaneInfo, PaneLayoutSnapshot,
    PaneResizeResult, PaneSwapResult, PaneTextPoint, PaneTextRange, PaneZoomResult,
};
use super::server::ServerCapabilities;
use super::tabs::TabInfo;
use super::workspaces::WorkspaceInfo;
use super::worktrees::{WorktreeInfo, WorktreeSourceInfo};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SuccessResponse {
    pub id: String,
    pub result: ResponseResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorResponse {
    pub id: String,
    pub error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseResult {
    WorkspaceInfo {
        workspace: WorkspaceInfo,
    },
    WorkspaceCreated {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
    },
    WorkspaceList {
        workspaces: Vec<WorkspaceInfo>,
    },
    WorktreeList {
        source: WorktreeSourceInfo,
        worktrees: Vec<WorktreeInfo>,
    },
    WorktreeCreated {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
        worktree: WorktreeInfo,
    },
    WorktreeOpened {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
        worktree: WorktreeInfo,
        already_open: bool,
    },
    WorktreeRemoved {
        workspace_id: String,
        path: String,
        forced: bool,
    },
    TabInfo {
        tab: TabInfo,
    },
    TabCreated {
        tab: TabInfo,
        root_pane: PaneInfo,
    },
    TabList {
        tabs: Vec<TabInfo>,
    },
    PaneInfo {
        pane: PaneInfo,
    },
    PaneSwap {
        swap: PaneSwapResult,
    },
    PaneZoom {
        zoom: PaneZoomResult,
    },
    LayoutSplitRatioSet {
        layout: LayoutDescription,
    },
    PaneFocusDirection {
        focus: PaneFocusDirectionResult,
    },
    PaneResize {
        resize: PaneResizeResult,
    },
    PaneSelection {
        pane_id: String,
        text: String,
    },
    PaneCopyMotion {
        pane_id: String,
        cursor: PaneTextPoint,
        content_revision: u64,
    },
    PaneCopySearch {
        pane_id: String,
        content_revision: u64,
        matches: Vec<PaneTextRange>,
        total: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current_global: Option<u64>,
    },
    IntegrationList {
        integrations: Vec<super::integrations::IntegrationInfo>,
    },
    IntegrationInstall {
        target: IntegrationTarget,
        details: IntegrationInstallResult,
    },
    PaneLinkResolved {
        regions: Vec<super::panes::PaneLinkRegion>,
    },
    PaneLinkActivated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        handled: bool,
    },
    ConfigReload {
        status: crate::config::ConfigReloadStatus,
        diagnostics: Vec<String>,
    },
    /// Acknowledgement for the client-shell surface interest lease. This method is new on the
    /// endpoint protocol, so its revision-bearing result can establish an activation floor.
    ClientShellSurfaceSet {
        active: bool,
        projection_revision: u64,
    },
    Ok {},
}
