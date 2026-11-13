use super::*;
use crate::ui::widgets::{button_row, paint_panel};

fn contained(parent: Rect, child: Rect) -> bool {
    child.is_empty() || child.intersection(parent) == child
}

fn check<const N: usize>(screen: Rect, size: (u16, u16), widths: [u16; N], body_rows: Option<u16>) {
    let Some(layout) = DialogLayout::centered(screen, size, widths, body_rows) else {
        return;
    };
    assert!(contained(screen, layout.area));
    assert!(contained(layout.area, layout.body));
    assert!(contained(layout.area, layout.footer));
    assert!(layout.body.intersection(layout.footer).is_empty());
    if body_rows.is_none() {
        assert_eq!(layout.footer.y, layout.area.bottom() - 2);
    }
    for (index, button) in layout.buttons.iter().enumerate() {
        assert!(contained(layout.footer, *button));
        for other in &layout.buttons[..index] {
            assert!(button.intersection(*other).is_empty());
        }
    }
    for offset in 0..=layout.body.height {
        let row = layout.body_row(offset);
        assert!(contained(layout.area, row));
        for button in layout.buttons {
            assert!(row.intersection(button).is_empty());
        }
    }
    assert!(layout.body_row(u16::MAX).is_empty());
}

#[test]
fn layouts_keep_body_and_hits_inside_the_same_clipped_geometry() {
    for (x, y) in [(0, 0), (7, 11), (u16::MAX - 40, u16::MAX - 30)] {
        for width in 0..85 {
            for height in 0..30 {
                let screen = Rect::new(x, y, width, height);
                check(screen, (56, 7), [8, 10, 12], Some(3));
                check(screen, (64, 6), [13, 12], Some(3));
                check(screen, (76, 22), [10, 12], None);
                check(screen, (76, 22), [10, 12], Some(0));
                check(screen, (76, 22), [], None);
            }
        }
    }
}

#[test]
fn clipping_never_moves_hidden_actions_into_an_earlier_buttons_place() {
    let area = Rect::new(7, 11, 5, 2);
    assert_eq!(
        button_row(area, [8, 10, 12], 2, 1),
        [Rect::new(7, 12, 5, 1), Rect::default(), Rect::default()]
    );
    assert_eq!(
        button_row(area, [u16::MAX, u16::MAX], u16::MAX, u16::MAX),
        [Rect::new(7, 12, 5, 1), Rect::default()]
    );
    assert_eq!(
        button_row(Rect::default(), [8, 10], 2, 3),
        [Rect::default(); 2]
    );
}

#[test]
fn shared_panel_paints_only_the_buffer_intersection() {
    let mut buffer = Buffer::empty(Rect::new(3, 5, 12, 10));
    for cell in &mut buffer.content {
        cell.set_symbol("x");
    }
    assert_eq!(
        paint_panel(
            &mut buffer,
            Rect::new(0, 0, 8, 10),
            Color::Red,
            Color::Reset
        ),
        Some(Rect::new(4, 6, 3, 3))
    );
    assert_eq!(buffer[(3, 5)].symbol(), "┌");
    assert_eq!(buffer[(7, 9)].symbol(), "┘");
    assert_eq!(buffer[(8, 9)].symbol(), "x");
    let before = buffer.clone();
    assert!(paint_panel(
        &mut buffer,
        Rect::new(40, 40, 2, 2),
        Color::Red,
        Color::Reset
    )
    .is_none());
    assert_eq!(buffer, before);
}
