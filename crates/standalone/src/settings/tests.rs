//! The dev host offers only the standard rates the device takes, keeps its
//! settings between runs, and names each block plan.

use super::{Blocks, MAX_BLOCKS, Settings, offered};

#[test]
fn only_the_standard_rates_the_device_takes_are_offered() {
    assert_eq!(
        offered([(8_000, 384_000)], 48_000),
        [44_100, 48_000, 88_200, 96_000, 176_400, 192_000]
    );
    assert_eq!(offered([(48_000, 48_000)], 48_000), [48_000]);
    assert_eq!(
        offered([(44_100, 48_000), (96_000, 96_000)], 48_000),
        [44_100, 48_000, 96_000]
    );
}

#[test]
fn the_devices_own_rate_is_offered_even_when_it_is_not_standard() {
    assert_eq!(offered([(22_050, 22_050)], 22_050), [22_050]);
    assert_eq!(offered([], 48_000), [48_000]);
}

#[test]
fn kept_settings_come_back_only_when_the_device_still_takes_them() {
    let settings = Settings {
        rate: 96_000,
        max: 256,
        blocks: Blocks::Random,
    };
    let kept = settings.write();
    assert_eq!(kept, "96000 256 random");
    assert_eq!(Settings::read(&kept, &[48_000, 96_000]), Some(settings));
    assert_eq!(Settings::read(&kept, &[48_000]), None, "a rate gone");
    assert_eq!(Settings::read("96000 300 random", &[96_000]), None);
    assert_eq!(Settings::read("96000 256 jumbled", &[96_000]), None);
    assert_eq!(Settings::read("", &[96_000]), None);
}

#[test]
fn a_run_starts_at_the_devices_rate_as_the_device_asks() {
    let start = Settings::starting(44_100);
    assert_eq!((start.rate, start.blocks), (44_100, Blocks::Device));
    assert!(MAX_BLOCKS.contains(&start.max));
}

#[test]
fn each_plan_is_found_by_its_name_and_its_number() {
    for blocks in Blocks::ALL {
        assert_eq!(Blocks::named(blocks.name()), Some(blocks));
        assert_eq!(Blocks::from_index(blocks.index()), blocks);
        assert!(!blocks.label().is_empty());
    }
    assert_eq!(Blocks::from_index(200), Blocks::Device);
}
