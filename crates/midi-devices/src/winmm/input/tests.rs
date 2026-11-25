//! Tests the callback's reading with the messages that WinMM sends, so no
//! device is needed.

use windows::Win32::Media::{MMSYSERR_ALLOCATED, MMSYSERR_BADDEVICEID, MMSYSERR_NOMEM};

use super::super::refusal;
use super::*;
use crate::ring::{Consumer, ring};

fn shared() -> (Shared, Consumer) {
    let (producer, consumer) = ring(16);
    let wake = EventHandle::new().expect("an event");
    let shared = Shared::new(producer, wake);
    // The device started at one second.
    shared.started.store(1_000_000_000, Ordering::Release);
    (shared, consumer)
}

/// Returns each packet's time in the ring, in milliseconds, and its first
/// two words.
fn taken(consumer: &mut Consumer) -> Vec<(u64, [u32; 2])> {
    let mut taken = Vec::new();
    while let Some((at, [first, second, ..])) = consumer.pop() {
        taken.push((at.nanos() / 1_000_000, [first, second]));
    }
    taken
}

/// Hands back a SysEx buffer as the driver does, with `bytes` recorded, at
/// `index` among the input's buffers.
fn long_data(shared: &Shared, bytes: &mut [u8], index: usize, millis: usize) {
    let header = MIDIHDR {
        lpData: PSTR(bytes.as_mut_ptr()),
        dwBufferLength: bytes.len() as u32,
        dwBytesRecorded: bytes.len() as u32,
        dwUser: index,
        ..MIDIHDR::default()
    };
    shared.message(MM_MIM_LONGDATA, (&raw const header) as usize, millis);
}

#[test]
fn a_short_message_is_read_into_a_packet_stamped_from_the_start() {
    let (shared, mut consumer) = shared();
    shared.message(MM_MIM_DATA, 0x007F_3C90, 5);
    // A clock is one byte and a program change two, so the rest of the word
    // isn't read.
    shared.message(MM_MIM_DATA, 0x00AA_BBF8, 6);
    shared.message(MM_MIM_DATA, 0x00AA_05C1, 7);
    assert_eq!(
        taken(&mut consumer),
        [
            (1005, [0x2090_3C7F, 0]),
            (1006, [0x10F8_0000, 0]),
            (1007, [0x20C1_0500, 0])
        ]
    );
}

#[test]
fn a_sysex_buffer_is_read_and_marked_to_be_handed_back() {
    let (shared, mut consumer) = shared();
    let mut identity = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];
    long_data(&shared, &mut identity, 3, 9);
    assert_eq!(taken(&mut consumer), [(1009, [0x3004_7E7F, 0x0601_0000])]);
    let marked: Vec<bool> = shared
        .returned
        .iter()
        .map(|flag| flag.load(Ordering::Acquire))
        .collect();
    assert!(marked[3] && marked.iter().filter(|&&marked| marked).count() == 1);
}

#[test]
fn a_sysex_across_buffers_reads_as_one() {
    let (shared, mut consumer) = shared();
    let mut first = [0xF0, 1, 2, 3, 4];
    let mut second = [5, 6, 7, 0xF7];
    long_data(&shared, &mut first, 0, 1);
    long_data(&shared, &mut second, 1, 2);
    // A Start of six bytes, once the seventh shows that more follows, then
    // the End.
    assert_eq!(
        taken(&mut consumer),
        [(1002, [0x3016_0102, 0x0304_0506]), (1002, [0x3031_0700, 0])]
    );
}

#[test]
fn an_error_drops_what_came_of_a_sysex() {
    let (shared, mut consumer) = shared();
    let mut cut = [0xF0, 1, 2];
    long_data(&shared, &mut cut, 0, 1);
    shared.message(MM_MIM_ERROR, 0, 2);
    let mut rest = [3, 0xF7];
    long_data(&shared, &mut rest, 1, 3);
    assert_eq!(taken(&mut consumer), []);
}

#[test]
fn while_closing_nothing_is_read_or_handed_back() {
    let (shared, mut consumer) = shared();
    shared.closing.store(true, Ordering::Release);
    shared.message(MM_MIM_DATA, 0x007F_3C90, 1);
    let mut identity = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];
    long_data(&shared, &mut identity, 0, 2);
    assert_eq!(taken(&mut consumer), []);
    assert!(!shared.returned[0].load(Ordering::Acquire));
    shared.message(MM_MIM_CLOSE, 0, 0);
    assert!(!shared.left.load(Ordering::Acquire), "the input closed it");
}

#[test]
fn a_close_the_input_did_not_ask_for_means_the_device_left() {
    let (shared, _consumer) = shared();
    shared.message(MM_MIM_CLOSE, 0, 0);
    assert!(shared.left.load(Ordering::Acquire));
}

#[test]
fn a_refusal_names_what_the_system_said() {
    assert_eq!(refusal(MMSYSERR_ALLOCATED), OpenError::InUse);
    assert_eq!(refusal(MMSYSERR_BADDEVICEID), OpenError::Gone);
    assert_eq!(refusal(MMSYSERR_NOMEM), OpenError::NoMemory);
    assert_eq!(refusal(11), OpenError::System(11));
}
