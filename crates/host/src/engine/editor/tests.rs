//! The editor's slot keeps what each exporter gives it, and refuses a scale
//! that no editor could be told and be done with.

use std::error::Error;

use super::EditorSlot;
use crate::{Editor, ParentWindow, Size};

/// An editor that does nothing, at 100 by 100.
struct Still;

impl Editor for Still {
    fn size(&self) -> Size {
        Size {
            width: 100,
            height: 100,
        }
    }

    fn open(&mut self, _parent: &ParentWindow) -> Result<(), Box<dyn Error>> {
        Ok(())
    }

    fn close(&mut self) {}

    fn set_size(&mut self, _size: Size) {}

    fn set_scale(&mut self, _scale: f64) {}

    fn tick(&mut self) {}
}

#[test]
fn a_scale_that_is_no_positive_number_is_refused_by_the_slot_itself() {
    let slot = EditorSlot::new(Box::new(Still));
    // A NaN never equals the scale told last, so a slot that took one would
    // tell the editor again and again.
    for scale in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
        assert!(!slot.set_scale(scale), "{scale}");
        assert_eq!(slot.scale(), 1.0, "{scale} is not kept");
    }
    assert!(slot.set_scale(2.0));
    assert_eq!(slot.scale(), 2.0);
}
