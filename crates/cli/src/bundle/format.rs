//! The file format of a built module and the processor that it is built for,
//! read from the module's header.
//!
//! Open: a universal Mach-O file, which holds a module for each of several
//! processors, is not read yet. `cargo build` writes a module for one
//! processor, so only a macOS bundle that joins two would make one.

#[cfg(test)]
mod tests;

use std::fmt;
use std::fs;
use std::io::Read as _;
use std::path::Path;

use anyhow::{Context as _, Result, ensure};

/// A module's file format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
    /// Windows' Portable Executable.
    Pe,
    /// The Executable and Linkable Format of Linux and most other systems.
    Elf,
    /// Apple's Mach-O.
    MachO,
}

/// A processor that a module is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Arch {
    /// 64-bit x86.
    X86_64,
    /// 64-bit Arm.
    Aarch64,
}

// The headers' signatures, fields and machine numbers, with the names that
// `winnt.h`, `elf.h`, `mach-o/loader.h` and `mach/machine.h` give them.
const IMAGE_DOS_SIGNATURE: &[u8] = b"MZ";
const IMAGE_NT_SIGNATURE: [u8; 4] = *b"PE\0\0";
const IMAGE_FILE_MACHINE_AMD64: u32 = 0x8664;
const IMAGE_FILE_MACHINE_ARM64: u32 = 0xaa64;
const ELFMAG: &[u8] = b"\x7fELF";
const EI_CLASS: usize = 4;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EM_X86_64: u32 = 62;
const EM_AARCH64: u32 = 183;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_MAGIC: u32 = 0xfeed_face;
const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;

impl Format {
    /// Returns the format of the modules that Cargo builds on `os`, a value of
    /// `std::env::consts::OS`.
    ///
    /// Any system but Windows and macOS is taken to use ELF, as Linux and the
    /// BSDs do.
    pub(super) fn of_system(os: &str) -> Format {
        match os {
            "windows" => Format::Pe,
            "macos" => Format::MachO,
            _ => Format::Elf,
        }
    }

    /// Returns the format of the module at `path` and the processor that it
    /// is built for, from the first 64 KiB of the file, which hold its
    /// headers.
    ///
    /// # Errors
    ///
    /// Returns why the file could not be read, or an error as
    /// [`read`](Self::read) does.
    pub(super) fn read_file(path: &Path) -> Result<(Format, Arch)> {
        let mut head = Vec::new();
        fs::File::open(path)
            .and_then(|file| file.take(64 * 1024).read_to_end(&mut head))
            .with_context(|| path.display().to_string())?;
        Format::read(&head)
    }

    /// Returns the format of the module whose file starts with `head`, and
    /// the processor that it is built for.
    ///
    /// # Errors
    ///
    /// Returns an error if `head` is not a whole PE, ELF or Mach-O header, or
    /// is the header of a module for a processor other than x86-64 and arm64.
    fn read(head: &[u8]) -> Result<(Format, Arch)> {
        let format =
            Format::of_magic(head).context("the built module is not a PE, ELF or Mach-O file")?;
        let machine = match format {
            Format::Pe => pe(head),
            Format::Elf => elf(head),
            Format::MachO => mach_o(head),
        }?;
        let arch = format.arch(machine).with_context(|| {
            format!(
                "the built module is for the {format} machine {machine:#06x}, but cargo hapto \
                 bundles only x86-64 and arm64 modules"
            )
        })?;
        Ok((format, arch))
    }

    /// Returns the format whose magic number `head` starts with.
    fn of_magic(head: &[u8]) -> Option<Format> {
        let mach_o = [MH_MAGIC_64, MH_MAGIC].map(u32::to_le_bytes);
        if head.starts_with(IMAGE_DOS_SIGNATURE) {
            Some(Format::Pe)
        } else if head.starts_with(ELFMAG) {
            Some(Format::Elf)
        } else if mach_o.iter().any(|magic| head.starts_with(magic)) {
            Some(Format::MachO)
        } else {
            None
        }
    }

    /// Returns the processor for `machine`, a machine number from this
    /// format's header.
    fn arch(self, machine: u32) -> Option<Arch> {
        match (self, machine) {
            (Format::Pe, IMAGE_FILE_MACHINE_AMD64)
            | (Format::Elf, EM_X86_64)
            | (Format::MachO, CPU_TYPE_X86_64) => Some(Arch::X86_64),
            (Format::Pe, IMAGE_FILE_MACHINE_ARM64)
            | (Format::Elf, EM_AARCH64)
            | (Format::MachO, CPU_TYPE_ARM64) => Some(Arch::Aarch64),
            _ => None,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Pe => "PE",
            Format::Elf => "ELF",
            Format::MachO => "Mach-O",
        })
    }
}

/// Returns a PE header's machine number, which follows the PE signature that
/// the DOS header's `e_lfanew`, at offset 0x3c, points to.
fn pe(head: &[u8]) -> Result<u32> {
    let at = usize::try_from(u32::from_le_bytes(bytes(head, 0x3c)?))?;
    ensure!(
        bytes(head, at)? == IMAGE_NT_SIGNATURE,
        "the built module has a DOS header but no PE header"
    );
    Ok(u16::from_le_bytes(bytes(head, at.saturating_add(4))?).into())
}

/// Returns the `e_machine`, at offset 18, of an ELF header whose class byte at
/// `EI_CLASS`, and the data byte after it, give 64-bit and little-endian.
fn elf(head: &[u8]) -> Result<u32> {
    let [class, data] = bytes(head, EI_CLASS)?;
    ensure!(
        class == ELFCLASS64 && data == ELFDATA2LSB,
        "the built module is a 32-bit or big-endian ELF file, but cargo hapto bundles only \
         64-bit little-endian modules"
    );
    Ok(u16::from_le_bytes(bytes(head, 18)?).into())
}

/// Returns a Mach-O header's `cputype`, which follows the magic number, read
/// little-endian as x86-64 and arm64 write it.
fn mach_o(head: &[u8]) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(head, 4)?))
}

/// Returns the `N` bytes of `head` at `at`, or an error if `head` ends first.
fn bytes<const N: usize>(head: &[u8], at: usize) -> Result<[u8; N]> {
    at.checked_add(N)
        .and_then(|end| head.get(at..end))
        .and_then(|bytes| bytes.try_into().ok())
        .context("the built module's header is cut short")
}
