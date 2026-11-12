//! Copy-mode reads the client's retained presentation terminal, never a PTY,
//! process tree, or the user's currently visible viewport.
use std::io;

use super::{model::invalid, view::View};
use crate::{
    api::schema as api,
    layout::PaneId,
    pane::{TerminalSearchDirection, TerminalTextPoint, TerminalWordMotion},
};

const MAX_QUERY_BYTES: usize = 4096;
const MAX_RETURNED_MATCHES: usize = 1024;

fn current(view: &View, revision: Option<u64>) -> io::Result<()> {
    if revision.is_some_and(|revision| revision != view.revision) {
        return Err(io::Error::other("stale_content"));
    }
    Ok(())
}

pub(super) fn motion(
    view: &View,
    id: PaneId,
    params: api::PaneCopyMotionParams,
) -> io::Result<api::ResponseResult> {
    use api::PaneCopyMotion as M;
    current(view, params.content_revision)?;
    let (width, rows) = view
        .terminal
        .text_dimensions()
        .ok_or_else(|| invalid("copy_motion_unavailable"))?;
    if usize::try_from(params.cursor.row).map_or(true, |row| row >= rows)
        || params.cursor.col >= width
    {
        return Err(invalid("copy_motion_unavailable"));
    }
    let origin = TerminalTextPoint {
        row: params.cursor.row,
        col: params.cursor.col,
    };
    let target = match params.motion {
        M::LineEnd | M::FirstNonBlank => {
            let selection = crate::selection::Selection::absolute_range(
                id,
                (origin.row, 0),
                (origin.row, width.saturating_sub(1)),
            );
            let text = view
                .terminal
                .extract_selection(&selection)
                .ok_or_else(|| invalid("copy_motion_unavailable"))?;
            let col = if params.motion == M::LineEnd {
                crate::copy_mode::last_character_col(&text)
            } else {
                crate::copy_mode::first_non_blank_col(&text)
            }
            .unwrap_or(0)
            .min(width.saturating_sub(1));
            TerminalTextPoint {
                row: origin.row,
                col,
            }
        }
        M::PreviousParagraph | M::NextParagraph => view
            .terminal
            .paragraph_motion_target(
                origin.row,
                if params.motion == M::PreviousParagraph {
                    -1
                } else {
                    1
                },
            )
            .map(|point| TerminalTextPoint {
                row: point.row,
                col: origin.col,
            })
            .unwrap_or(origin),
        motion => {
            let motion = match motion {
                M::NextWordStart => TerminalWordMotion::NextStart,
                M::PreviousWordStart => TerminalWordMotion::PreviousStart,
                M::NextWordEnd => TerminalWordMotion::NextEnd,
                M::NextBigWordStart => TerminalWordMotion::NextBigStart,
                M::PreviousBigWordStart => TerminalWordMotion::PreviousBigStart,
                M::NextBigWordEnd => TerminalWordMotion::NextBigEnd,
                _ => return Err(invalid("copy_motion_unavailable")),
            };
            view.terminal
                .word_motion_target(origin.row, origin.col, motion)
                .unwrap_or(origin)
        }
    };
    // Session owns this View on its one worker thread: output/resize cannot
    // interleave this read, unlike the removed async PTY runtime.
    Ok(api::ResponseResult::PaneCopyMotion {
        pane_id: params.pane_id,
        cursor: api::PaneTextPoint {
            row: target.row,
            col: target.col,
        },
        content_revision: view.revision,
    })
}

pub(super) fn search(
    view: &View,
    params: api::PaneCopySearchParams,
) -> io::Result<api::ResponseResult> {
    if params.query.len() > MAX_QUERY_BYTES {
        return Err(invalid("query_too_large"));
    }
    current(view, Some(params.content_revision))?;
    let point = |point: api::PaneTextPoint| TerminalTextPoint {
        row: point.row,
        col: point.col,
    };
    let result = view.terminal.search_text_window(
        &params.query,
        params.query.chars().any(char::is_uppercase),
        match params.direction {
            api::PaneCopySearchDirection::Forward => TerminalSearchDirection::Forward,
            api::PaneCopySearchDirection::Backward => TerminalSearchDirection::Backward,
        },
        point(params.cursor),
        params
            .previous
            .map(|range| (point(range.start), point(range.end))),
        MAX_RETURNED_MATCHES,
    );
    let point = |point: TerminalTextPoint| api::PaneTextPoint {
        row: point.row,
        col: point.col,
    };
    Ok(api::ResponseResult::PaneCopySearch {
        pane_id: params.pane_id,
        content_revision: view.revision,
        matches: result
            .matches
            .into_iter()
            .map(|found| api::PaneTextRange {
                start: point(found.start),
                end: point(found.end),
            })
            .collect(),
        total: u64::try_from(result.total).unwrap_or(u64::MAX),
        current: result.current.and_then(|index| u32::try_from(index).ok()),
        current_global: result
            .current_global
            .and_then(|index| u64::try_from(index).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::{PaneCopyMotion as M, PaneCopySearchDirection as D, PaneTextPoint as P};

    fn view(cols: u16, rows: u16, text: &str) -> View {
        let mut view = View::new(cols, rows, 17).unwrap();
        view.process_output(PaneId::from_raw(1), text.as_bytes());
        view
    }

    fn move_to(view: &View, row: u32, col: u16, motion_kind: M) -> P {
        let result = motion(
            view,
            PaneId::from_raw(1),
            api::PaneCopyMotionParams {
                pane_id: "opaque/terminal".into(),
                cursor: P { row, col },
                motion: motion_kind,
                content_revision: Some(view.revision),
            },
        )
        .unwrap();
        let api::ResponseResult::PaneCopyMotion {
            pane_id,
            cursor,
            content_revision,
        } = result
        else {
            panic!("wrong motion response")
        };
        assert_eq!(pane_id, "opaque/terminal");
        assert_eq!(content_revision, view.revision);
        cursor
    }

    fn query(
        view: &View,
        text: &str,
        direction: D,
        previous: Option<api::PaneTextRange>,
    ) -> api::ResponseResult {
        search(
            view,
            api::PaneCopySearchParams {
                pane_id: "opaque/terminal".into(),
                query: text.into(),
                direction,
                cursor: P { row: 0, col: 0 },
                content_revision: view.revision,
                previous,
            },
        )
        .unwrap()
    }

    fn selected(result: api::ResponseResult) -> api::PaneTextRange {
        let api::ResponseResult::PaneCopySearch {
            matches, current, ..
        } = result
        else {
            panic!("wrong search response")
        };
        matches[current.unwrap() as usize]
    }

    #[test]
    fn all_existing_copy_motions_use_terminal_coordinates() {
        let view = view(40, 6, "  alpha.beta gamma\r\n\r\nzebra end\r\n");
        for (row, col, motion, expected_row, expected_col) in [
            (0, 0, M::FirstNonBlank, 0, 2),
            (0, 0, M::LineEnd, 0, 17),
            (0, 2, M::NextWordStart, 0, 7),
            (0, 8, M::PreviousWordStart, 0, 7),
            (0, 2, M::NextWordEnd, 0, 6),
            (0, 2, M::NextBigWordStart, 0, 13),
            (0, 13, M::PreviousBigWordStart, 0, 2),
            (0, 2, M::NextBigWordEnd, 0, 11),
            (0, 8, M::NextParagraph, 1, 8),
            (2, 2, M::PreviousParagraph, 1, 2),
        ] {
            assert_eq!(
                move_to(&view, row, col, motion),
                P {
                    row: expected_row,
                    col: expected_col
                },
                "{motion:?}"
            );
        }
    }

    #[test]
    fn wide_combining_line_edges_and_soft_wrap_search_are_cell_based() {
        let view = view(8, 5, "  中e\u{301}x\r\nabcdef中Z");
        assert_eq!(move_to(&view, 0, 0, M::FirstNonBlank), P { row: 0, col: 2 });
        assert_eq!(move_to(&view, 0, 0, M::LineEnd), P { row: 0, col: 5 });
        let found = selected(query(&view, "中Z", D::Forward, None));
        assert_eq!(found.start, P { row: 1, col: 6 });
        assert_eq!(found.end, P { row: 2, col: 0 });
    }

    #[test]
    fn search_preserves_smart_case_previous_reverse_and_wrap() {
        let view = view(32, 4, "Needle first\r\nneedle middle\r\nneedle last");
        let second = selected(query(&view, "needle", D::Forward, None));
        assert_eq!(second.start.row, 1);
        let third = selected(query(&view, "needle", D::Forward, Some(second)));
        assert_eq!(third.start.row, 2);
        let first = selected(query(&view, "needle", D::Forward, Some(third)));
        assert_eq!(first.start.row, 0);
        assert_eq!(
            selected(query(&view, "needle", D::Backward, Some(first))),
            third
        );
        let api::ResponseResult::PaneCopySearch { total, .. } =
            query(&view, "Needle", D::Forward, None)
        else {
            panic!("wrong response")
        };
        assert_eq!(total, 1);
        let api::ResponseResult::PaneCopySearch { total, .. } =
            query(&view, "n.edle", D::Forward, None)
        else {
            panic!("wrong response")
        };
        assert_eq!(total, 0, "search is literal, not a regular expression");
    }

    #[test]
    fn search_response_is_bounded_but_total_is_exact_in_scrollback() {
        let config = crate::config::AdvancedConfig {
            scrollback_limit_lines: 3000,
            ..Default::default()
        };
        let mut view = View::with_scrollback(20, 4, 17, &config).unwrap();
        view.process_output(PaneId::from_raw(1), "needle\r\n".repeat(2500).as_bytes());
        let before = query(&view, "needle", D::Backward, None);
        view.terminal.scroll_up(100);
        let after = query(&view, "needle", D::Backward, None);
        assert_eq!(
            serde_json::to_value(&before).unwrap(),
            serde_json::to_value(&after).unwrap()
        );
        let api::ResponseResult::PaneCopySearch {
            matches,
            total,
            current,
            current_global,
            ..
        } = after
        else {
            panic!("wrong response")
        };
        assert_eq!(total, 2500);
        assert_eq!(matches.len(), MAX_RETURNED_MATCHES);
        assert_eq!(current, Some((MAX_RETURNED_MATCHES - 1) as u32));
        assert_eq!(current_global, Some(2499));
        assert_eq!(matches.last().unwrap().start.row, 2499);
    }

    #[test]
    fn changed_output_rejects_motion_and_search_without_mutating_view() {
        let mut view = view(20, 4, "first needle");
        let old_revision = view.revision;
        view.process_output(PaneId::from_raw(1), b"\r\nnew output");
        let revision = view.revision;
        let request = api::PaneCopyMotionParams {
            pane_id: "opaque/terminal".into(),
            cursor: P { row: 0, col: 0 },
            motion: M::LineEnd,
            content_revision: Some(old_revision),
        };
        assert_eq!(
            motion(&view, PaneId::from_raw(1), request)
                .unwrap_err()
                .to_string(),
            "stale_content"
        );
        let request = api::PaneCopySearchParams {
            pane_id: "opaque/terminal".into(),
            query: "needle".into(),
            direction: D::Forward,
            cursor: P { row: 0, col: 0 },
            content_revision: old_revision,
            previous: None,
        };
        assert_eq!(
            search(&view, request).unwrap_err().to_string(),
            "stale_content"
        );
        assert_eq!(view.revision, revision);
    }

    #[test]
    fn invalid_coordinates_and_oversized_queries_are_rejected() {
        let view = view(20, 4, "needle");
        for cursor in [
            P {
                row: u32::MAX,
                col: 0,
            },
            P { row: 0, col: 20 },
        ] {
            assert_eq!(
                motion(
                    &view,
                    PaneId::from_raw(1),
                    api::PaneCopyMotionParams {
                        pane_id: "opaque/terminal".into(),
                        cursor,
                        motion: M::LineEnd,
                        content_revision: Some(view.revision),
                    }
                )
                .unwrap_err()
                .to_string(),
                "copy_motion_unavailable"
            );
        }
        assert_eq!(
            search(
                &view,
                api::PaneCopySearchParams {
                    pane_id: "opaque/terminal".into(),
                    query: "x".repeat(MAX_QUERY_BYTES + 1),
                    direction: D::Forward,
                    cursor: P { row: 0, col: 0 },
                    content_revision: view.revision,
                    previous: None,
                }
            )
            .unwrap_err()
            .to_string(),
            "query_too_large"
        );
        for method in ["pane.copy_motion", "pane.copy_search"] {
            assert!(super::super::methods::METHODS.contains(&method));
        }
    }
}
