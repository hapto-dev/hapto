//! The example gain as an app of its own, which `cargo hapto dev` runs: its
//! page in a window, and a test signal through it to the audio output.

fn main() -> std::process::ExitCode {
    hapto_standalone::main::<hapto_example_gain::Gain>()
}
