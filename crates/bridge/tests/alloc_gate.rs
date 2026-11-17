//! The native side's tick allocates nothing while nothing moved: it runs on
//! every main-thread tick of a plugin with an editor open, however many
//! parameters and values the page shows.

hapto_testkit::install_alloc_gate!();

use hapto_bridge::{Commands, Dispatcher, Values};
use hapto_params::{Kind, ParamDecl, ParamSetBuilder, Value};
use hapto_testkit::alloc::assert_no_allocations;

const KEYS: [&str; 16] = [
    "p00", "p01", "p02", "p03", "p04", "p05", "p06", "p07", "p08", "p09", "p10", "p11", "p12",
    "p13", "p14", "p15",
];

#[test]
fn a_tick_with_nothing_moved_does_not_allocate() {
    let mut builder = ParamSetBuilder::new();
    for key in KEYS {
        builder
            .add(ParamDecl::new(
                key,
                key,
                Kind::continuous(0.0, 1.0, 0.0, None),
                0.5,
            ))
            .expect("valid");
    }
    let set = builder.build();
    let ui = set.take_ui().expect("the first UI handle");
    let label = Value::new(String::from("Untitled"), &set);
    let path = Value::new(vec![String::from("kick.wav")], &set);
    let values = Values::new().value("label", &label).value("path", &path);
    let mut native = Dispatcher::new(ui, Commands::new(), values, Vec::<String>::new());
    native.receive(r#"{"type":"ready","protocol":3}"#);
    // A drag the page holds open on one of them changes nothing here, and
    // nor does a value the page wrote and was answered.
    native.receive(r#"{"type":"gestureBegin","param":"p07"}"#);
    native.receive(r#"{"type":"setValue","key":"label","value":"Lead"}"#);
    native.outbox_mut().clear();
    assert_no_allocations("a tick with nothing moved", || native.tick());
    assert!(native.outbox_mut().is_empty());
}
