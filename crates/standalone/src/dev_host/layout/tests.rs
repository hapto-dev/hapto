//! The bar stays across the top as the window resizes, the view takes the
//! rest, and nothing is left uncovered.

use super::{BAR, Layout, Rect, lay_out, window_for};

#[test]
fn the_view_fills_the_window_below_the_bar() {
    for (width, height) in [(420, 256), (800, 600), (2_000, 1_200)] {
        let layout = lay_out(width, height, true);
        assert_eq!(layout.bar, Rect::whole(width, BAR));
        let view = layout.view.expect("a view");
        assert_eq!((view.x, view.y, view.width), (0, BAR, width));
        assert_eq!(view.y + view.height, height, "to the bottom edge");
    }
}

#[test]
fn a_window_shorter_than_the_bar_is_all_bar() {
    let layout = lay_out(300, 20, true);
    assert_eq!(layout.bar, Rect::whole(300, 20));
    assert_eq!(layout.view.map(|v| v.height), Some(0));
}

#[test]
fn with_no_view_the_bar_fills_the_window() {
    assert_eq!(
        lay_out(420, 120, false),
        Layout {
            bar: Rect::whole(420, 120),
            view: None,
        }
    );
}

#[test]
fn the_window_fits_the_view_with_the_bar_above() {
    assert_eq!(window_for(420, 220), (420, 220 + BAR));
}

#[test]
fn a_rectangle_in_the_screens_pixels_scales_each_side() {
    let rect = Rect {
        x: 0,
        y: BAR,
        width: 420,
        height: 220,
    };
    assert_eq!(rect.pixels(1.0), (0, 36, 420, 220));
    assert_eq!(rect.pixels(1.25), (0, 45, 525, 275));
}
