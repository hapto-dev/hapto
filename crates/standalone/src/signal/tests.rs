//! The test signals stay at their level and carry on across blocks.

use super::{Generator, LEVEL, Signal};

#[test]
fn each_signal_is_named_on_the_command_line() {
    assert_eq!(Signal::named("sine"), Some(Signal::Sine));
    assert_eq!(Signal::named("noise"), Some(Signal::Noise));
    assert_eq!(Signal::named("silence"), Some(Signal::Silence));
    assert_eq!(Signal::named("Sine"), None);
}

#[test]
fn each_signal_keeps_its_name_and_its_number() {
    for signal in Signal::ALL {
        assert_eq!(Signal::named(signal.name()), Some(signal));
        assert_eq!(Signal::from_index(signal.index()), signal);
    }
    assert_eq!(Signal::from_index(9), Signal::Silence);
}

#[test]
fn the_level_is_minus_24_dbfs() {
    let db = 20.0 * f64::from(LEVEL).log10();
    assert!((db + 24.0).abs() < 1e-4, "{db}");
}

#[test]
fn the_sine_carries_on_across_blocks_and_peaks_at_the_level() {
    let mut whole = Generator::new(Signal::Sine, 48_000.0);
    let mut split = Generator::new(Signal::Sine, 48_000.0);
    let mut one = [0.0_f32; 480];
    whole.fill(&mut one);
    let (mut first, mut second) = ([0.0_f32; 100], [0.0_f32; 380]);
    split.fill(&mut first);
    split.fill(&mut second);
    assert_eq!(one[..100], first);
    assert_eq!(one[100..], second);
    let peak = one.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
    assert!((peak - LEVEL).abs() < 1e-4, "{peak}");
}

#[test]
fn the_noise_stays_within_the_level_and_is_not_silence() {
    let mut noise = Generator::new(Signal::Noise, 48_000.0);
    let mut block = [0.0_f32; 4_096];
    noise.fill(&mut block);
    assert!(block.iter().all(|s| s.abs() <= LEVEL));
    assert!(block.iter().any(|s| s.abs() > LEVEL / 2.0));
    let mean = block.iter().sum::<f32>() / block.len() as f32;
    assert!(mean.abs() < LEVEL / 10.0, "centred: {mean}");
}

#[test]
fn silence_is_zeros() {
    let mut block = [1.0_f32; 64];
    Generator::new(Signal::Silence, 44_100.0).fill(&mut block);
    assert_eq!(block, [0.0; 64]);
}
