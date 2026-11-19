//! Each format has a name for the session and a label for a person, and a
//! host call carries its parameter, its value and its flags as fields.

use super::{Call, Format};

#[test]
fn each_format_is_found_by_its_name() {
    for format in Format::ALL {
        assert_eq!(Format::named(format.name()), Some(format));
    }
    assert_eq!(Format::named("clap"), None);
    assert_eq!(Format::Vst3.label(), "VST3");
    assert_eq!(Format::Cabi.label(), "C interface");
}

#[test]
fn an_edit_carries_its_parameter_and_its_value() {
    let call = Call::edit("performEdit", Some("Gain"), Some(0.625));
    assert_eq!(call.name, "performEdit");
    assert_eq!(
        call.fields,
        [("Param", "Gain".to_owned()), ("Value", "0.6250".to_owned())]
    );
    assert_eq!(
        Call::edit("beginEdit", Some("Gain"), None).fields,
        [("Param", "Gain".to_owned())]
    );
}

#[test]
fn a_restart_names_the_flags_it_knows_and_shows_the_rest_in_hex() {
    let known = [(1, "values"), (4, "latency")];
    let flags = |flags| Call::restart("restart", flags, &known).fields;
    assert_eq!(flags(1), [("Flags", "values".to_owned())]);
    assert_eq!(flags(5), [("Flags", "values latency".to_owned())]);
    assert_eq!(flags(0x41), [("Flags", "values 0x40".to_owned())]);
    assert_eq!(flags(0), [("Flags", "none".to_owned())]);
}
