//! Tests the output thread's own work without a device, which is packing a
//! message as WinMM takes it. Joining a SysEx is tested in `outputs::sysex`.

use super::*;

#[test]
fn a_message_is_packed_its_status_in_the_low_byte() {
    assert_eq!(pack(&[0x90, 0x3C, 0x7F]), 0x007F_3C90);
    assert_eq!(pack(&[0xC0, 0x05]), 0x0000_05C0);
    assert_eq!(pack(&[0xF8]), 0x0000_00F8);
}
