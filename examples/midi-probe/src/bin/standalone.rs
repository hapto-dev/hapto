//! The MIDI probe as an app of its own: it plays every MIDI input the
//! computer has, and sends what it echoes to the system's synth, so its MIDI
//! out is heard beside its own sines.

fn main() -> std::process::ExitCode {
    hapto_standalone::App::<hapto_example_midi_probe::MidiProbe>::new()
        .with_midi_outputs(|device| device.name() == "Microsoft GS Wavetable Synth")
        .run()
}
