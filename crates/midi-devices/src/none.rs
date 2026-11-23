//! A system Hapto reaches no MIDI devices on yet: the list is empty,
//! nothing changes, and no input or output opens.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::id::Device;
use crate::inputs::OpenError;
use crate::ring::{Consumer, Producer};

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

/// An output that cannot be: no device opens here.
#[derive(Debug)]
pub(crate) struct Output(Infallible);

impl Output {
    pub(crate) fn open(
        _device: &Device,
        consumer: Consumer,
    ) -> Result<Output, (OpenError, Option<Consumer>)> {
        Err((OpenError::NoBackend, Some(consumer)))
    }

    pub(crate) fn has_left(&self) -> bool {
        match self.0 {}
    }

    pub(crate) fn close(self) -> Option<Consumer> {
        match self.0 {}
    }
}
