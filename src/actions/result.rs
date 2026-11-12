use super::panes::{
    LayoutDescription, PaneFocusDirectionResult, PaneInfo, PaneResizeResult, PaneSwapResult,
    PaneTextPoint, PaneTextRange, PaneZoomResult,
};
use super::tabs::TabInfo;
use super::workspaces::WorkspaceInfo;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ActionResult {
    WorkspaceInfo {
        workspace: WorkspaceInfo,
    },
    WorkspaceCreated {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
    },
    TabInfo {
        tab: TabInfo,
    },
    TabCreated {
        tab: TabInfo,
        root_pane: PaneInfo,
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
        current: Option<u32>,
        current_global: Option<u64>,
    },
    // The original UI consumes these results; Core does not advertise link actions.
    #[allow(dead_code)]
    PaneLinkResolved {
        regions: Vec<super::panes::PaneLinkRegion>,
    },
    // Kept for the capability-gated original UI fallback, never advertised by Core.
    #[allow(dead_code)]
    PaneLinkActivated {
        url: Option<String>,
        handled: bool,
    },
    Ok {},
}
