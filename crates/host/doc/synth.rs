# use hapto_host::layout::StereoOut;
# use hapto_host::midi::{Expression, NoteOff, NoteOn, Poly, Voice, VoiceAudio};
# use hapto_host::{AuCodes, Block, Category, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status};
#
# /// A sine that sounds while its key is held, and stops at its release.
# struct Sine {
#     step: f32,
#     phase: f32,
#     level: f32,
#     sample_rate: f32,
# }
#
# impl Voice for Sine {
#     fn start(&mut self, note: &NoteOn) {
#         (self.phase, self.level) = (0.0, note.velocity);
#     }
#
#     fn release(&mut self, _: &NoteOff) {
#         self.level = 0.0;
#     }
#
#     fn render(&mut self, expression: &Expression<'_>, out: &mut VoiceAudio<'_>) {
#         self.step = expression.pitch.hz() / self.sample_rate;
#         for frame in out.frames_mut() {
#             frame.add(self.level * (self.phase * std::f32::consts::TAU).sin());
#             self.phase = (self.phase + self.step).fract();
#         }
#     }
#
#     fn is_done(&self) -> bool {
#         self.level == 0.0
#     }
# }
#
# /// Eight sines.
# struct Voices(Poly<Sine>);
#
# impl Processor for Voices {
#     type Layout = StereoOut;
#
#     fn process(&mut self, block: Block<'_, StereoOut>) -> Status {
#         self.0.play(block)
#     }
# }
#
# /// An instrument of sines.
# struct Synth;
#
# impl Plugin for Synth {
#     const INFO: PluginInfo = PluginInfo {
#         name: "Synth",
#         vendor: "Example",
#         url: "",
#         email: "",
#         description: "",
#         version: "0.1.0",
#         id: "dev.example.synth",
#         vst3_class_id: [0; 16],
#         au_codes: AuCodes { subtype: *b"synt", manufacturer: *b"Exmp" },
#         category: Category::Instrument,
#         features: &[],
#         layouts: &[],
#         midi_input: true,
#         midi_output: false,
#     };
#     type Params = ();
#     type Processor = Voices;
#
#     fn new(_: (), _: Parts) -> Self {
#         Synth
#     }
#
#     fn activate(&mut self, setup: &ProcessSetup) -> Voices {
#         let sample_rate = setup.sample_rate as f32;
#         Voices(Poly::new(8, || Sine { step: 0.0, phase: 0.0, level: 0.0, sample_rate }))
#     }
# }
