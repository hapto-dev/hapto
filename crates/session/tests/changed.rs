//! What a session reports as changed: a capture a participant asked for
//! that was kept and differs from the section it replaced, and never a
//! registration's first capture, a restore or the capture after its apply.

use std::cell::RefCell;
use std::rc::Rc;

use hapto_params::ParamSetBuilder;
use hapto_session::{
    ApplyError, MAX_SECTION_DEPTH, Participant, Registration, Section, Session, SessionUi,
};
use serde_json::json;

/// A participant whose state is one section; `null` saves nothing.
struct Held {
    section: Section,
    /// Its own registration, for a change it reports from inside a method.
    registration: Option<Registration>,
    /// Runs inside the capture.
    during_capture: Option<Box<dyn Fn()>>,
}

impl Held {
    /// Sets the section and reports it, under the owner's borrow.
    fn set(&mut self, section: Section) {
        self.section = section;
        if let Some(registration) = &self.registration {
            registration.changed();
        }
    }
}

impl Participant for Held {
    fn capture(&self) -> Section {
        if let Some(during) = &self.during_capture {
            during();
        }
        self.section.clone()
    }

    fn apply(&mut self, section: Option<&Section>) -> Result<(), ApplyError> {
        self.section = section.cloned().unwrap_or(json!("default"));
        Ok(())
    }
}

fn held(section: Section) -> Rc<RefCell<Held>> {
    Rc::new(RefCell::new(Held {
        section,
        registration: None,
        during_capture: None,
    }))
}

fn rig() -> (Session, SessionUi) {
    let session = Session::new(&ParamSetBuilder::new().build());
    let ui = session.take_ui().expect("the first UI handle");
    (session, ui)
}

#[test]
fn a_reported_capture_that_differs_is_a_change_taken_once() {
    let (session, ui) = rig();
    let a = held(json!("one"));
    let registration = ui.register("a", &a).expect("free");
    assert!(!session.take_changed(), "a first capture is no change");

    a.borrow_mut().section = json!("two");
    assert!(!session.take_changed(), "not reported yet");
    registration.changed();
    assert!(session.take_changed());
    assert!(!session.take_changed(), "taking it cleared the mark");

    registration.changed();
    assert!(!session.take_changed(), "the same section again");

    // To nothing to save, and back.
    a.borrow_mut().section = Section::Null;
    registration.changed();
    assert!(session.take_changed());
    registration.changed();
    assert!(!session.take_changed(), "still nothing");
    a.borrow_mut().section = json!({"deep": [1, 2.5, "x"]});
    registration.changed();
    assert!(session.take_changed());
    a.borrow_mut().section = json!({"deep": [1, 2.5, "y"]});
    registration.changed();
    assert!(session.take_changed(), "the same length, another section");
}

#[test]
fn a_capture_a_tick_takes_for_a_held_up_participant_is_a_change() {
    let (session, ui) = rig();
    let a = held(json!(1));
    let registration = ui.register("a", &a).expect("free");
    a.borrow_mut().registration = Some(registration);

    a.borrow_mut().set(json!(2));
    assert!(!session.take_changed(), "held up: nothing captured yet");
    ui.tick();
    assert!(session.take_changed());
    ui.tick();
    assert!(!session.take_changed(), "captured once");
}

#[test]
fn a_restore_is_no_change_and_drops_the_ones_before_it() {
    let (session, ui) = rig();
    let a = held(json!("one"));
    let registration = ui.register("a", &a).expect("free");

    a.borrow_mut().section = json!("two");
    registration.changed();
    session.restore(Some(&json!({"a": "three", "late": "four"})));
    assert!(!session.take_changed(), "the restore replaced the change");

    // A report while the restored section waits captures nothing.
    registration.changed();
    ui.tick();
    assert_eq!(a.borrow().section, json!("three"), "applied");
    assert!(
        !session.take_changed(),
        "nor is the capture after the apply"
    );

    // A participant that registers for a section the restore left waiting.
    let late = held(json!("mine"));
    let _late = ui.register("late", &late).expect("free");
    assert_eq!(
        late.borrow().section,
        json!("four"),
        "applied as it registered"
    );
    assert!(!session.take_changed());

    // A change after the restore counts again.
    a.borrow_mut().section = json!("five");
    registration.changed();
    assert!(session.take_changed());
}

#[test]
fn a_restore_landing_during_a_capture_leaves_no_change() {
    let (session, ui) = rig();
    let a = held(json!("one"));
    let registration = ui.register("a", &a).expect("free");
    let host = session.clone();
    a.borrow_mut().section = json!("two");
    a.borrow_mut().during_capture = Some(Box::new(move || {
        host.restore(Some(&json!({"a": "restored"})));
    }));
    registration.changed();
    assert!(!session.take_changed(), "the capture was not kept");
    assert_eq!(session.save(), Some(json!({"a": "restored"})));
}

#[test]
fn a_refused_capture_is_no_change() {
    let (session, ui) = rig();
    let a = held(json!("one"));
    let registration = ui.register("a", &a).expect("free");

    // Too deep to load again.
    a.borrow_mut().section = (0..=MAX_SECTION_DEPTH).fold(json!(1), |inner, _| json!([inner]));
    registration.changed();
    assert!(registration.refused());
    assert!(!session.take_changed());

    // No room in the budget: `{"a":"one"}` and not a byte more.
    session.set_budget(r#"{"a":"one"}"#.len());
    a.borrow_mut().section = json!("longer");
    registration.changed();
    assert!(registration.refused());
    assert!(!session.take_changed());
    assert_eq!(session.save(), Some(json!({"a": "one"})));

    // One that fits is kept, and is a change.
    a.borrow_mut().section = json!("two");
    registration.changed();
    assert!(!registration.refused());
    assert!(session.take_changed());
}
