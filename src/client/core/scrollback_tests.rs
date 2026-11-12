//! Row retention is a client-local parser policy, not a Core geometry setting.
use super::view::View;
use crate::{config::AdvancedConfig, layout::PaneId};

fn populated(config: &AdvancedConfig, cols: u16) -> View {
    let mut view = View::with_scrollback(cols, 54, 17, config).unwrap();
    let id = PaneId::alloc();
    for row in 0..12_000 {
        // Include style and wide text; the byte budget is not a row-count proxy.
        view.process_output(
            id,
            format!(
                "\x1b[32m{row:05} 中文 {}\x1b[0m\r\n",
                "x".repeat(cols as usize - 20)
            )
            .as_bytes(),
        );
    }
    view
}

fn history(view: &View) -> usize {
    view.terminal
        .scroll_metrics()
        .unwrap()
        .max_offset_from_bottom
}

#[test]
fn default_scrollback_retains_about_a_thousand_rows_at_narrow_and_wide_grids() {
    for cols in [80, 195] {
        let view = populated(&AdvancedConfig::default(), cols);
        let kept = history(&view);
        assert!(
            (1000..=2000).contains(&kept),
            "cols={cols}, retained={kept}"
        );
        assert!(view
            .terminal
            .recent_text_snapshot(10)
            .text
            .contains("11999 中文"));
    }
}

#[test]
fn configured_scrollback_retains_more_than_the_old_thousand_row_attach_limit() {
    let config = AdvancedConfig {
        scrollback_limit_lines: 5000,
        ..Default::default()
    };
    let view = populated(&config, 195);
    let kept = history(&view);
    assert!((5000..=6000).contains(&kept), "retained={kept}");
    // Copy/search read this same retained parser without scrolling the program.
    assert!(view
        .terminal
        .recent_text_snapshot(4500)
        .text
        .contains("08000 中文"));
}

#[test]
fn zero_scrollback_rows_or_an_explicit_zero_byte_budget_disables_history() {
    for config in [
        AdvancedConfig {
            scrollback_limit_lines: 0,
            ..Default::default()
        },
        AdvancedConfig {
            scrollback_limit_bytes: Some(0),
            ..Default::default()
        },
    ] {
        let view = populated(&config, 195);
        assert_eq!(history(&view), 0);
        assert!(view.terminal.visible_text().contains("11999 中文"));
    }
}

#[test]
fn an_explicit_byte_budget_remains_an_additional_scrollback_limit() {
    let config = AdvancedConfig {
        scrollback_limit_lines: 5000,
        scrollback_limit_bytes: Some(10_000),
    };
    let view = populated(&config, 195);
    assert!(history(&view) < 1000);
}
