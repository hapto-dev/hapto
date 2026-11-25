//! The backend for systems where Hapto reaches no MIDI devices yet, so the
//! list is empty and never changes, and no input or output opens.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::error::OpenError;
use crate::id::Device;
use crate::ring::{Consumer, Producer};

pub(crate) fn list() -> Vec<Device> {
    Vec::new()
}

/// A watch that never starts, because an empty list never changes.
#[derive(Debug)]
pub(crate) struct Watch;

impl Watch {
    pub(crate) fn new(_marked: Arc<AtomicBool>) -> Option<Watch> {
        None
    }
}

/// An input that can't exist, because no device opens here.
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

/// An output thread's wake signal, which does nothing because no output
/// opens here.
#[derive(Debug)]
pub(crate) struct Signal;

impl Signal {
    pub(crate) fn new() -> Signal {
        Signal
    }

    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn set(&self) {}
}

/// An output that can't exist, because no device opens here.
#[derive(Debug)]
pub(crate) struct Output(Infallible);

impl Output {
    pub(crate) fn open(
        _device: &Device,
        consumer: Consumer,
        _signal: Arc<Signal>,
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
