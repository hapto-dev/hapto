//! The example synth as an app of its own: it plays every MIDI input the
//! computer has.

fn main() -> std::process::ExitCode {
    hapto_standalone::App::<hapto_example_synth::Synth>::new().run()
}
