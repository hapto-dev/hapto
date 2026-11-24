# use hapto_params::{Kind, ParamDecl, ParamSetBuilder};
# let mut builder = ParamSetBuilder::new();
# builder.add(ParamDecl::new("gain", "Gain", Kind::continuous(0.0, 1.0, 0.0, None), 0.0))?;
# let set = builder.build();
# let gain = set.id("gain").ok_or("declared above")?;
# use hapto_ump::{EventList, Midi1Message, Midi1Voice, Upgrade};
# // One block of MIDI 1.0 control changes on group 0 and channel 0, as
# // (controller, value) pairs, upgraded as the `Learn` example does it.
# let mut upgrade = Upgrade::new();
# let mut block = |changes: &[(u8, u8)]| {
#     let mut sent = EventList::with_capacity(16);
#     for &(controller, value) in changes {
#         let message = Midi1Message::ControlChange { controller, value };
#         sent.push(Midi1Voice { group: 0, channel: 0, message }.event(0));
#     }
#     let mut events = EventList::with_capacity(16);
#     upgrade.block(sent.as_slice(), &mut events);
#     events
# };
# // A target that does nothing, for the examples that only register one:
# // the `Target` example shows what a target does.
# struct Fader;
# impl hapto_learn::Target for Fader {
#     fn begin(&mut self) {}
#     fn apply(&mut self, _normalized: f64) {}
#     fn end(&mut self) {}
# }
