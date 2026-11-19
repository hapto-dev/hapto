//! Where the bar and the plugin's view go in the dev host's window, in
//! logical pixels: the bar across the top, the view below it, so no part of
//! the window is left uncovered. The settings have a window of their own.

#[cfg(test)]
mod tests;

/// The bar's height.
pub(crate) const BAR: u32 = 36;

/// The window's size for a format with no editor: the bar, and a line
/// saying where the parameters are.
pub(crate) const NO_EDITOR: (u32, u32) = (480, 120);

/// The settings window's size as it first opens.
pub(crate) const SETTINGS: (u32, u32) = (1100, 760);

/// A rectangle of a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
    /// From the left edge.
    pub(crate) x: u32,
    /// From the top edge.
    pub(crate) y: u32,
    /// Its width.
    pub(crate) width: u32,
    /// Its height.
    pub(crate) height: u32,
}

impl Rect {
    /// The whole of a window `width` by `height`.
    pub(crate) fn whole(width: u32, height: u32) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    /// Returns it in the screen's pixels at `scale`, as Win32 takes it.
    pub(crate) fn pixels(self, scale: f64) -> (i32, i32, i32, i32) {
        let to = |value: u32| (f64::from(value) * scale).round() as i32;
        (to(self.x), to(self.y), to(self.width), to(self.height))
    }
}

/// The parts of the dev host's window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    /// The bar: across the top, or the whole window with no view.
    pub(crate) bar: Rect,
    /// The view, below the bar; `None` with no view.
    pub(crate) view: Option<Rect>,
}

/// Lays out a window `width` by `height`: the bar [`BAR`] tall across the
/// top and the view below it, or with no view, the bar all of it.
pub(crate) fn lay_out(width: u32, height: u32, has_view: bool) -> Layout {
    if !has_view {
        return Layout {
            bar: Rect::whole(width, height),
            view: None,
        };
    }
    let bar = BAR.min(height);
    Layout {
        bar: Rect::whole(width, bar),
        view: Some(Rect {
            x: 0,
            y: bar,
            width,
            height: height - bar,
        }),
    }
}

/// Returns the window's size for a view of `width` by `height`: the view
/// with the bar above it.
pub(crate) fn window_for(width: u32, height: u32) -> (u32, u32) {
    (width, height + BAR)
}
