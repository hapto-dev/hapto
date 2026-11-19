//! The mappings in a saved block reach the parameters they name, by the
//! host id each key hashes to, as words.

use super::learned;
use crate::dev_host::hosted::Param;

fn param(key: &str) -> Param {
    Param {
        id: 0,
        host_id: hapto_params::host_id(key),
        handle: key.to_owned(),
        title: key.to_owned(),
        units: String::new(),
        steps: 0,
        kind: "continuous".to_owned(),
    }
}

#[test]
fn each_mapping_reaches_the_parameter_its_key_names() {
    let block = br#"{"seamStateVersion": 1, "parameters": {}, "extra": {"midiLearn": {
        "schema": 1,
        "mappings": [
            {"target": "gain", "kind": "cc", "group": 0, "channel": 0, "number": 74},
            {"target": "gain", "kind": "note", "group": -1, "channel": 9, "number": 60},
            {"target": "mode", "kind": "nrpn", "group": 2, "channel": -1, "number": 300},
            {"target": "gone", "kind": "cc", "group": 0, "channel": 0, "number": 1},
            {"target": "mode", "kind": "pitch", "group": 0, "channel": 0, "number": 1}
        ]
    }}}"#;
    let params = [param("gain"), param("mode"), param("bypass")];
    assert_eq!(
        learned(block, &params),
        [
            "group 1 · ch 1 · CC 74, any group · ch 10 · note 60",
            "group 3 · any channel · NRPN 300",
            "",
        ]
    );
}

#[test]
fn a_mapping_with_a_group_past_any_number_is_passed_over() {
    let block = br#"{"extra": {"midiLearn": {"mappings": [
        {"target": "gain", "kind": "cc", "group": 9223372036854775807, "channel": 0, "number": 1}
    ]}}}"#;
    assert_eq!(learned(block, &[param("gain")]), [""]);
}

#[test]
fn a_block_with_no_mappings_learned_nothing() {
    let params = [param("gain")];
    assert_eq!(learned(b"not json", &params), [""]);
    assert_eq!(learned(br#"{"parameters": {}}"#, &params), [""]);
}
