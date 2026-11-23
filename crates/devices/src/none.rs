//! A system Hapto reads no MIDI devices on yet: the list is empty, nothing
//! changes, and an input never opens.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::id::Device;
use crate::inputs::OpenError;
use crate::ring::Producer;

pub(crate) fn list() -> Vec<Device> {
    Vec::new()
}

/// Watches nothing: an empty list never changes, so it needs no look.
#[derive(Debug)]
pub(crate) struct Watch;

impl Watch {
    pub(crate) fn new(_marked: Arc<AtomicBool>) -> Option<Watch> {
        Some(Watch)
    }
}

/// An input that cannot be: no device opens here.
#[derive(Debug)]
pub(crate) struct Input(Infallible);

impl Input {
    pub(crate) fn open(
        _device: &Device,
        _group: u8,
        producer: Producer,
    ) -> Result<Input, (OpenError, Option<Producer>)> {
        Err((OpenError::NoBackend, Some(producer)))
    }

    pub(crate) fn has_left(&self) -> bool {
        match self.0 {}
    }

    pub(crate) fn close(self) -> Option<Producer> {
        match self.0 {}
    }
}
