//! Typed UI actions for the sole Core runtime, not a public JSON API.
//!
//! Names identify capability gates and diagnostics. The Core wire protocol is
//! implemented independently in client/core/transport.rs.

mod common;
mod panes;
mod result;
mod tabs;
mod workspaces;

pub(crate) use common::*;
pub(crate) use panes::*;
pub(crate) use result::*;
pub(crate) use tabs::*;
pub(crate) use workspaces::*;

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ActionRequest {
    pub id: String,
    pub action: Action,
}

macro_rules! actions {
    ($( $variant:ident($params:ty) => $name:literal; )*) => {
        // Direct variants avoid extra boxes on every UI action.
        #[derive(Debug, Clone, PartialEq)]
        #[allow(clippy::large_enum_variant)]
        pub(crate) enum Action {
            $($variant($params),)*
        }

        impl Action {
            pub(crate) fn name(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $name,)*
                }
            }

            #[cfg(test)]
            pub(crate) const NAMES: &'static [&'static str] = &[$($name,)*];
        }
    };
}

// Some original UI affordances still name actions Core does not implement.
// Constructing an action is not admission: the existing capability gate must
// reject those actions, and only client/core/methods.rs advertises implementations.
actions! {
    WorkspaceCreate(WorkspaceCreateParams) => "workspace.create";
    WorkspaceFocus(WorkspaceTarget) => "workspace.focus";
    WorkspaceRename(WorkspaceRenameParams) => "workspace.rename";
    WorkspaceMove(WorkspaceMoveParams) => "workspace.move";
    WorkspaceMoveBlock(WorkspaceMoveBlockParams) => "workspace.move_block";
    WorkspaceClose(WorkspaceCloseParams) => "workspace.close";
    TabCreate(TabCreateParams) => "tab.create";
    TabFocus(TabTarget) => "tab.focus";
    TabRename(TabRenameParams) => "tab.rename";
    TabMove(TabMoveParams) => "tab.move";
    TabClose(TabTarget) => "tab.close";
    PaneSplit(PaneSplitParams) => "pane.split";
    PaneSwap(PaneSwapParams) => "pane.swap";
    PaneZoom(PaneZoomParams) => "pane.zoom";
    LayoutSetSplitRatio(LayoutSetSplitRatioParams) => "layout.set_split_ratio";
    PaneFocusDirection(PaneFocusDirectionParams) => "pane.focus_direction";
    PaneResize(PaneResizeParams) => "pane.resize";
    PaneScroll(PaneScrollParams) => "pane.scroll";
    PaneClear(PaneTarget) => "pane.clear";
    PaneEditScrollback(PaneTarget) => "pane.edit_scrollback";
    PaneSelectionRead(PaneSelectionReadParams) => "pane.selection.read";
    PaneCopyMotion(PaneCopyMotionParams) => "pane.copy_motion";
    PaneCopySearch(PaneCopySearchParams) => "pane.copy_search";
    PaneFocus(PaneTarget) => "pane.focus";
    PaneInputSet(PaneInputSetParams) => "pane.input.set";
    PaneLinkActivate(PaneLinkActivateParams) => "pane.link.activate";
    PaneLinkResolve(PaneLinkActivateParams) => "pane.link.resolve";
    PaneRename(PaneRenameParams) => "pane.rename";
    PaneClose(PaneTarget) => "pane.close";
}

#[cfg(test)]
mod tests;
