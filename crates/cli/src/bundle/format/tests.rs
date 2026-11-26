//! [`Format::read`] reads a module's format and processor from its header,
//! and rejects any other bytes.

use super::{Arch, Format};

/// The first bytes of a PE module built for `machine`.
fn pe(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0; 0x100];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    bytes[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
    bytes
}

/// The first bytes of a 64-bit little-endian ELF module built for `machine`.
fn elf(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0; 64];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2; // ELFCLASS64
    bytes[5] = 1; // ELFDATA2LSB
    bytes[6] = 1; // EV_CURRENT
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes
}

/// The first bytes of a Mach-O module with the magic number `magic`, built
/// for `cputype`.
fn mach_o(magic: u32, cputype: u32) -> Vec<u8> {
    let mut bytes = vec![0; 32];
    bytes[..4].copy_from_slice(&magic.to_le_bytes());
    bytes[4..8].copy_from_slice(&cputype.to_le_bytes());
    bytes
}

/// The magic number of a 64-bit Mach-O header.
const MH_MAGIC_64: u32 = 0xfeed_facf;
/// The magic number of a 32-bit Mach-O header.
const MH_MAGIC: u32 = 0xfeed_face;

/// Returns the error that `Format::read` gives for `head`.
fn refused(head: &[u8]) -> String {
    Format::read(head).expect_err("refused").to_string()
}

#[test]
fn each_format_s_machine_number_reads_as_its_processor() {
    for (head, read) in [
        (pe(0x8664), (Format::Pe, Arch::X86_64)),
        (pe(0xaa64), (Format::Pe, Arch::Aarch64)),
        (elf(62), (Format::Elf, Arch::X86_64)),
        (elf(183), (Format::Elf, Arch::Aarch64)),
        (
            mach_o(MH_MAGIC_64, 0x0100_0007),
            (Format::MachO, Arch::X86_64),
        ),
        (
            mach_o(MH_MAGIC_64, 0x0100_000c),
            (Format::MachO, Arch::Aarch64),
        ),
    ] {
        assert_eq!(Format::read(&head).unwrap(), read, "{read:?}");
    }
}

#[test]
fn a_module_for_another_processor_is_refused_with_its_machine() {
    for (head, named) in [
        (pe(0x014c), "PE machine 0x014c"),
        (elf(3), "ELF machine 0x0003"),
        (mach_o(MH_MAGIC, 7), "Mach-O machine 0x0007"),
    ] {
        let refused = refused(&head);
        assert!(refused.contains(named), "{refused}");
    }
}

#[test]
fn a_32_bit_or_big_endian_elf_module_is_refused() {
    let mut x32 = elf(62);
    x32[4] = 1; // ELFCLASS32, which x86-64's x32 ABI writes
    let mut big = elf(183);
    big[5] = 2; // ELFDATA2MSB, which aarch64_be writes
    for head in [x32, big] {
        let refused = refused(&head);
        assert!(refused.contains("32-bit or big-endian ELF"), "{refused}");
    }
}

#[test]
fn unknown_or_truncated_headers_are_refused() {
    for head in [&b"not a module"[..], &[]] {
        assert_eq!(
            refused(head),
            "the built module is not a PE, ELF or Mach-O file"
        );
    }
    let mut far = pe(0x8664);
    far[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
    for (head, what) in [
        (&pe(0x8664)[..0x84], "PE"),
        (&far[..], "PE past the end"),
        (&elf(62)[..5], "ELF before its class"),
        (&elf(62)[..18], "ELF before its machine"),
        (&mach_o(MH_MAGIC_64, 0x0100_0007)[..6], "Mach-O"),
    ] {
        let refused = refused(head);
        assert!(refused.contains("cut short"), "{what}: {refused}");
    }
    let mut dos = pe(0x8664);
    dos[0x80..0x84].copy_from_slice(b"NE\0\0");
    let refused = refused(&dos);
    assert!(refused.contains("no PE header"), "{refused}");
}

#[test]
fn each_system_builds_modules_of_its_own_format() {
    assert_eq!(Format::of_system("windows"), Format::Pe);
    assert_eq!(Format::of_system("linux"), Format::Elf);
    assert_eq!(Format::of_system("freebsd"), Format::Elf);
    assert_eq!(Format::of_system("macos"), Format::MachO);
}

#[test]
fn a_module_built_here_reads_as_this_system_s() {
    let exe = std::env::current_exe().expect("this test's own executable");
    let arch = match std::env::consts::ARCH {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        other => panic!("this test has no processor for {other}"),
    };
    let system = Format::of_system(std::env::consts::OS);
    assert_eq!(Format::read_file(&exe).unwrap(), (system, arch));
}
