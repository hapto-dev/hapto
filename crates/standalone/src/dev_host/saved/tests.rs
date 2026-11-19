//! The run's first load says whether the plugin took the session's block,
//! upgraded it or refused it, and a refused block is the one the session
//! holds on to.

use std::error::Error;

use anyhow::anyhow;
use hapto_host::{
    AuCodes, Block, Category, Layout, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
};
use hapto_params::{Declarer, Params};
use serde_json::{Map, Value as Json};

use super::FirstLoad;
use crate::dev_host::format::Format;
use crate::dev_host::hosted::host;

#[derive(Clone)]
struct NoParams;

impl Params for NoParams {
    fn declare(_: &mut Declarer) -> Self {
        NoParams
    }
}

struct Through;

impl Processor for Through {
    fn process(&mut self, _block: Block<'_, '_>) -> Status {
        Status::Continue
    }
}

/// A plugin at its state's version 1, whose migration refuses a block
/// holding `"broken": true`.
struct Migrating;

impl Plugin for Migrating {
    const INFO: PluginInfo = PluginInfo {
        name: "Migrating",
        vendor: "Test",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.test.migrating",
        vst3_class_id: [1; 16],
        au_codes: AuCodes {
            subtype: *b"migr",
            manufacturer: *b"Test",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[Layout::STEREO],
        midi_input: false,
        midi_output: false,
    };
    const SCHEMA_VERSION: u64 = 1;
    type Params = NoParams;
    type Processor = Through;

    fn new(_params: NoParams, _parts: Parts) -> Self {
        Migrating
    }

    fn migrate(
        _from: u64,
        block: &mut Map<String, Json>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if block.contains_key("broken") {
            return Err("the block is broken".into());
        }
        Ok(())
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Through {
        Through
    }
}

const OLDER: &[u8] = br#"{"seamStateVersion":1,"pluginStateVersion":0,"parameters":{}}"#;
const BROKEN: &[u8] =
    br#"{"seamStateVersion":1,"pluginStateVersion":0,"parameters":{},"broken":true}"#;

#[test]
fn a_session_with_no_block_loads_nothing() {
    let hosted = host::<Migrating>(Format::Cabi);
    let load = FirstLoad::of(Ok(None), &*hosted);
    assert_eq!(load, FirstLoad::Nothing);
    assert!(!load.holds());
}

#[test]
fn a_block_of_the_plugin_s_own_version_loads_as_it_is() {
    for format in Format::ALL {
        let hosted = host::<Migrating>(format);
        let own = hosted.save().expect("saved");
        let load = FirstLoad::of(Ok(Some(own)), &*hosted);
        assert_eq!(load, FirstLoad::Loaded { upgraded: None }, "{format:?}");
        assert_eq!(load.note(), "loaded the saved block");
    }
}

#[test]
fn an_older_block_is_upgraded_and_says_from_which_version() {
    for format in Format::ALL {
        let hosted = host::<Migrating>(format);
        let load = FirstLoad::of(Ok(Some(OLDER.to_vec())), &*hosted);
        let upgraded = FirstLoad::Loaded {
            upgraded: Some((0, 1)),
        };
        assert_eq!(load, upgraded, "{format:?}");
        assert!(!load.holds());
    }
}

#[test]
fn a_refused_block_is_held_as_it_was() {
    for format in Format::ALL {
        let hosted = host::<Migrating>(format);
        let load = FirstLoad::of(Ok(Some(BROKEN.to_vec())), &*hosted);
        let FirstLoad::Refused { reason, block } = &load else {
            panic!("{format:?}: refused, not {load:?}");
        };
        assert!(reason.contains("answered"), "{format:?}: {reason}");
        assert_eq!(block, BROKEN, "{format:?}: unchanged");
        assert!(load.holds(), "{format:?}");
        assert!(load.note().starts_with("the saved block was refused"));
    }
}

#[test]
fn a_block_that_could_not_be_read_is_held() {
    let hosted = host::<Migrating>(Format::Cabi);
    let load = FirstLoad::of(Err(anyhow!("saved.json: denied")), &*hosted);
    let unreadable = FirstLoad::Unreadable {
        reason: "saved.json: denied".into(),
    };
    assert_eq!(load, unreadable);
    assert!(load.holds());
}
