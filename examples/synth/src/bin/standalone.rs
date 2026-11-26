//! The example synth as an app of its own, which plays every MIDI input on
//! the computer.

fn main() -> std::process::ExitCode {
    hapto_standalone::App::<hapto_example_synth::Synth>::new().run()
}
