//! The app's audio thread never allocates, from the first device buffer
//! after activation on: the shipped app's callback, through the standalone
//! exporter, and the dev host's, through the VST3 exporter and the C
//! interface, for every way of cutting blocks, every test signal, and
//! devices of fewer and more channels than the plugin's.
//!
//! The app runs on Windows alone, so the gate does too.

hapto_testkit::install_alloc_gate!();

#[cfg(windows)]
mod on_windows {
    use hapto_host::standalone::Standalone;
    use hapto_host::{
        AuCodes, Block, Category, Layout, Parts, Plugin, PluginInfo, ProcessSetup, Processor,
        Status,
    };
    use hapto_params::{Declarer, Float, FloatDecl, Params};
    use hapto_standalone::gate::{Blocks, Callback, Signal};
    use hapto_testkit::alloc::assert_no_allocations;

    const PLANS: [Blocks; 4] = [
        Blocks::Device,
        Blocks::Fixed,
        Blocks::Random,
        Blocks::Awkward,
    ];
    const SIGNALS: [Signal; 3] = [Signal::Sine, Signal::Noise, Signal::Silence];
    /// Device buffers' lengths in frames: a common one, a frame, more than
    /// a part, one short of a block, and one that ends a part.
    const BUFFERS: [usize; 5] = [480, 1, 4096, 1023, 512];

    #[derive(Clone)]
    struct Level {
        gain: Float,
    }

    impl Params for Level {
        fn declare(p: &mut Declarer) -> Self {
            Level {
                gain: p.add(FloatDecl::new("gain", "Gain", 0.0..=1.0, 0.5)),
            }
        }
    }

    /// Scales every channel by its gain.
    struct Scale(Level);

    impl Processor for Scale {
        fn process(&mut self, block: Block<'_, '_>) -> Status {
            let gain = self.0.gain.get();
            for channel in block.channels.iter_mut() {
                for sample in channel.iter_mut() {
                    *sample *= gain;
                }
            }
            Status::Continue
        }
    }

    struct Amp {
        params: Level,
    }

    impl Plugin for Amp {
        const INFO: PluginInfo = PluginInfo {
            name: "Amp",
            vendor: "Hapto",
            url: "",
            email: "",
            description: "",
            version: "0.1.0",
            id: "dev.hapto.amp",
            vst3_class_id: [7; 16],
            au_codes: AuCodes {
                subtype: *b"amp ",
                manufacturer: *b"Hapt",
            },
            category: Category::Effect,
            features: &[],
            layouts: &[Layout::STEREO, Layout::MONO],
            midi_input: false,
            midi_output: false,
        };
        type Params = Level;
        type Processor = Scale;

        fn new(params: Level, _: Parts) -> Self {
            Amp { params }
        }

        fn activate(&mut self, _: &ProcessSetup) -> Scale {
            Scale(self.params.clone())
        }
    }

    #[test]
    fn the_apps_callback_does_not_allocate() {
        let mut buffer = vec![0.0f32; 4096 * 6];
        for layout in [Layout::STEREO, Layout::MONO] {
            for blocks in PLANS {
                for signal in SIGNALS {
                    for channels in [1, 2, 6] {
                        let plugin = Standalone::<Amp>::new();
                        // The app's own maximum.
                        assert_eq!(plugin.activate(48_000.0, 1024, layout), Ok(()));
                        let mut callback = Callback::app(
                            &plugin, layout, channels, 48_000.0, 1024, blocks, signal,
                        );
                        let what = format!(
                            "the app's callback: {layout:?}, {blocks:?}, {signal:?}, \
                             {channels} channels"
                        );
                        assert_no_allocations(&what, || {
                            for frames in BUFFERS {
                                callback.play(&mut buffer[..frames * channels]);
                            }
                        });
                        drop(callback);
                        assert_eq!(plugin.faults(), 0, "{what}");
                    }
                }
            }
        }
    }

    #[cfg(feature = "dev-host")]
    #[test]
    fn the_dev_hosts_callback_does_not_allocate() {
        use hapto_standalone::gate::dev::{DevHost, Format};

        let mut buffer = vec![0.0f32; 4096 * 2];
        for format in [Format::Vst3, Format::Cabi] {
            for max in [32, 1024, 4096] {
                let mut host = DevHost::start::<Amp>(format, 2, 48_000.0, max).expect("started");
                for blocks in PLANS {
                    for signal in SIGNALS {
                        // As the settings window does while blocks play.
                        host.cut(blocks);
                        host.play_into(signal);
                        host.set(0, 0.25);
                        let what = format!(
                            "the dev host's callback: {format:?} at most {max}, {blocks:?}, \
                             {signal:?}"
                        );
                        assert_no_allocations(&what, || {
                            for frames in BUFFERS {
                                host.play(&mut buffer[..frames * 2]);
                            }
                        });
                    }
                }
                assert_eq!(host.faults(), 0, "{format:?}");
            }
        }
    }
}
