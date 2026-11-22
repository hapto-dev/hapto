//! Walking audio allocates nothing: frames in every layout, channels, and
//! the whole-buffer calls a processor makes on the audio thread.

hapto_testkit::install_alloc_gate!();

use hapto_audio::layout::{AnyLayout, Stereo, Width};
use hapto_audio::{Audio, AudioBuf};
use hapto_testkit::alloc::assert_no_allocations;

#[test]
fn walking_audio_allocates_nothing() {
    let mut stereo = AudioBuf::<Stereo>::new(256);
    let mut wide = AudioBuf::<AnyLayout>::new(Width::new(8, 8), 256);
    let (mut left, mut right) = (vec![0.5_f32; 256], vec![0.5_f32; 256]);
    assert_no_allocations("walking audio", || {
        let mut audio = stereo.audio();
        for [left, right] in audio.frames_mut() {
            *left += 0.1;
            *right -= 0.1;
        }
        for channel in audio.channels_mut() {
            for sample in channel {
                *sample *= 0.5;
            }
        }
        audio.apply_gain(2.0);
        audio.add_to_all(std::iter::repeat(0.25));
        let _ = audio.peak();
        for frame in audio.as_any_layout().frames_mut() {
            frame.add(0.1);
        }
        audio.clear();

        let mut audio = wide.audio();
        for frame in audio.frames_mut() {
            frame.add(0.1);
        }

        let mut channels = [&mut left[..], &mut right[..]];
        let mut lent: Audio<'_, Stereo> = Audio::new(&mut channels, 128);
        lent.apply_gain(0.5);
    });
}
