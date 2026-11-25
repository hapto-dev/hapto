//! The parts of the editor that don't use wry, so that their tests run on
//! every system. They are the page's native side, [`Native`], the hand-back
//! message, [`HAND_BACK`], and [`script`], which wraps a message for the
//! page's transport.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use hapto_bridge::{Dispatcher, Outbox};

/// The page's message that gives the keyboard back to the host: nothing in
/// the page needs keys. It is no protocol message, which is tagged by
/// `type`, so it never reaches the dispatcher.
pub(crate) const HAND_BACK: &str = r#"{"shell":"releaseKeys"}"#;

/// The script that hands `message` to the page's transport, which listens
/// as `window.__hapto`: the message as a JavaScript string, which JSON's
/// string syntax is.
pub(crate) fn script(message: &str) -> String {
    let text = serde_json::Value::String(message.to_owned()).to_string();
    format!("window.__hapto?.receive({text})")
}

/// The page's native side, which the editor and its WebView's IPC handler
/// share: the dispatcher, and what waits while it is in a call.
///
/// A command the page calls may run a message loop of its own, such as a
/// file dialog's, and the page's next messages and the host's close then
/// arrive inside the call. A message is held and handed over once the call
/// returns, before any that comes later, and a close's end of the session
/// is done then.
pub(crate) struct Native<O> {
    dispatcher: RefCell<Dispatcher<O>>,
    /// Messages the page sent while the dispatcher was in a call, in order.
    held: RefCell<VecDeque<String>>,
    /// Whether the page's session ended while the dispatcher was in a call.
    ending: Cell<bool>,
}

impl<O: Outbox> Native<O> {
    /// Creates the native side over `dispatcher`, with nothing held.
    pub(crate) fn new(dispatcher: Dispatcher<O>) -> Self {
        Native {
            dispatcher: RefCell::new(dispatcher),
            held: RefCell::new(VecDeque::new()),
            ending: Cell::new(false),
        }
    }

    /// Hands `message` to the dispatcher after those held before it, or
    /// holds it while the dispatcher is in a call.
    pub(crate) fn deliver(&self, message: String) {
        self.held.borrow_mut().push_back(message);
        if let Ok(mut dispatcher) = self.dispatcher.try_borrow_mut() {
            self.catch_up(&mut dispatcher);
        }
    }

    /// Whether the dispatcher is in a call: a command the page called is
    /// running.
    pub(crate) fn in_call(&self) -> bool {
        self.dispatcher.try_borrow_mut().is_err()
    }

    /// Hands over what was held, then tells the page what the tick changed;
    /// nothing while the dispatcher is in a call.
    pub(crate) fn tick(&self) {
        if let Ok(mut dispatcher) = self.dispatcher.try_borrow_mut() {
            self.catch_up(&mut dispatcher);
            dispatcher.tick();
        }
    }

    /// Ends the page's session and drops what it sent that is still held:
    /// at once, or as the call the dispatcher is in returns.
    pub(crate) fn end_session(&self) {
        self.held.borrow_mut().clear();
        match self.dispatcher.try_borrow_mut() {
            Ok(mut dispatcher) => dispatcher.end_session(),
            Err(_) => self.ending.set(true),
        }
    }

    /// Does what waited for the dispatcher, in order: a session's end, then
    /// each held message, whose call may bring more of either.
    fn catch_up(&self, dispatcher: &mut Dispatcher<O>) {
        loop {
            if self.ending.take() {
                dispatcher.end_session();
            }
            let Some(message) = self.held.borrow_mut().pop_front() else {
                return;
            };
            dispatcher.receive(&message);
        }
    }

    /// The messages that the dispatcher has refused so far.
    #[cfg(all(test, hapto_webview))]
    pub(crate) fn refusals(&self) -> hapto_bridge::Refusals {
        self.dispatcher.borrow().refusals()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use hapto_bridge::{Commands, Dispatcher, Values};
    use hapto_params::{ParamId, UiParams};

    use super::*;
    use crate::fixtures::{DIALOG, READY, dialog, during_dialog, params};

    /// A native side over one parameter, with the `dialog` command, that
    /// posts into a buffer.
    fn native() -> (Rc<Native<Vec<String>>>, UiParams, ParamId) {
        let (_, ui, bypass) = params();
        let commands = Commands::new().command("dialog", dialog);
        let dispatcher = Dispatcher::new(ui.clone(), commands, Values::new(), Vec::new());
        (Rc::new(Native::new(dispatcher)), ui, bypass)
    }

    #[test]
    fn the_hand_back_is_the_fixtures_text() {
        let fixture = include_str!("../tests/fixtures/shell.json");
        assert_eq!(HAND_BACK, fixture.trim_end());
    }

    #[test]
    fn a_message_reaches_the_transport_as_a_javascript_string() {
        assert_eq!(
            script(r#"{"text":"say \"hi\""}"#),
            r#"window.__hapto?.receive("{\"text\":\"say \\\"hi\\\"\"}")"#
        );
    }

    #[test]
    fn it_is_in_a_call_while_a_command_runs_and_not_after() {
        let (native, _ui, _bypass) = native();
        let seen = Rc::new(Cell::new(None));
        let (inside, during) = (Rc::downgrade(&native), Rc::clone(&seen));
        during_dialog(move || during.set(inside.upgrade().map(|native| native.in_call())));
        assert!(!native.in_call());
        native.deliver(READY.to_owned());
        native.deliver(DIALOG.to_owned());
        assert_eq!(seen.get(), Some(true), "in the call");
        assert!(!native.in_call(), "the call has returned");
    }

    #[test]
    fn a_message_held_while_a_command_runs_keeps_its_place() {
        let (native, ui, bypass) = native();
        let inside = Rc::downgrade(&native);
        during_dialog(move || {
            if let Some(native) = inside.upgrade() {
                native.deliver(r#"{"type":"set","param":"bypass","normalized":1}"#.to_owned());
            }
        });
        native.deliver(READY.to_owned());
        native.deliver(DIALOG.to_owned());
        native.deliver(r#"{"type":"set","param":"bypass","normalized":0}"#.to_owned());
        native.tick();
        assert_eq!(ui.value(bypass), 0.0, "the later set lands last");
    }

    #[test]
    fn a_close_while_a_command_runs_ends_the_session_as_it_returns() {
        let (native, ui, bypass) = native();
        let inside = Rc::downgrade(&native);
        during_dialog(move || {
            if let Some(native) = inside.upgrade() {
                native.end_session();
            }
        });
        native.deliver(READY.to_owned());
        native.deliver(r#"{"type":"gestureBegin","param":"bypass"}"#.to_owned());
        assert!(ui.is_open(bypass));
        native.deliver(DIALOG.to_owned());
        assert!(!ui.is_open(bypass), "the page's drag ends for the host");
    }
}
