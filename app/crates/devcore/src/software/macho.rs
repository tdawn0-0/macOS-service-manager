//! Executable architecture verification: Mach-O arm64 slice checks on macOS
//! (ported from the Go `architecture.go:11-47`) and ELF64 `e_machine` checks on
//! Linux (`x86_64` / `aarch64`).
//!
//! On macOS every Mach-O executable must contain an arm64 slice; scripts with
//! an interpreter are accepted while Intel-only Mach-O binaries are rejected.
//! On Linux every managed executable must be either a `#!` interpreter script
//! or a 64-bit ELF binary whose `e_machine` matches the host architecture.

use std::io::Read;
use std::path::Path;

use super::SoftwareError;
use crate::platform::{HostArch, HostTarget};

const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;

const MH_MAGIC: u32 = 0xfeed_face;
const MH_CIGAM: u32 = 0xcefa_edfe;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_CIGAM_64: u32 = 0xcffa_edfe;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_CIGAM: u32 = 0xbeba_feca;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const FAT_CIGAM_64: u32 = 0xbfba_feca;

const ELF_MAGIC: [u8; 4] = *b"\x7fELF";
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ELFDATA2MSB: u8 = 2;
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;

/// Verify that `path` is a valid executable for the compiled host target.
pub fn require_host_executable(path: &Path) -> Result<(), SoftwareError> {
    let target = HostTarget::current();
    if target.is_macos() {
        require_apple_silicon(path)
    } else {
        require_elf_arch(path, target.arch)
    }
}

/// Reject a managed executable that has no Apple Silicon slice.
///
/// The Go implementation first inspects the four-byte magic, ignores anything
/// that is not Mach-O, then opens the file thin and finally fat. This port
/// parses just the headers it needs, so it never follows an executable's load
/// commands.
pub fn require_apple_silicon(path: &Path) -> Result<(), SoftwareError> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 4 {
        return Ok(());
    }
    let magic = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    match magic {
        MH_MAGIC | MH_CIGAM | MH_MAGIC_64 | MH_CIGAM_64 => {
            let big_endian = matches!(magic, MH_MAGIC | MH_MAGIC_64);
            let Some(cpu) = read_u32(&bytes, 4, big_endian) else {
                return Err(cannot_verify(path, "truncated Mach-O header"));
            };
            if cpu == CPU_TYPE_ARM64 {
                return Ok(());
            }
            if cpu == CPU_TYPE_X86_64 {
                return Err(SoftwareError::invalid(format!(
                    "{} is Intel-only or another unsupported architecture; an Apple Silicon executable is required",
                    path.display()
                )));
            }
            Err(cannot_verify(path, "unsupported Mach-O CPU type"))
        }
        FAT_MAGIC | FAT_CIGAM | FAT_MAGIC_64 | FAT_CIGAM_64 => {
            let big_endian = matches!(magic, FAT_MAGIC | FAT_MAGIC_64);
            let is_64 = matches!(magic, FAT_MAGIC_64 | FAT_CIGAM_64);
            let Some(count) = read_u32(&bytes, 4, big_endian) else {
                return Err(cannot_verify(path, "truncated fat header"));
            };
            let entry = if is_64 { 32 } else { 20 };
            for index in 0..count as usize {
                let offset = 8 + index * entry;
                let Some(cpu) = read_u32(&bytes, offset, big_endian) else {
                    return Err(cannot_verify(path, "truncated fat architecture table"));
                };
                if cpu == CPU_TYPE_ARM64 {
                    return Ok(());
                }
            }
            Err(SoftwareError::invalid(format!(
                "{} has no Apple Silicon (arm64) executable",
                path.display()
            )))
        }
        _ => Ok(()),
    }
}

/// Reject a Linux managed executable unless it is a `#!` script or a 64-bit ELF
/// binary matching `expected_arch`.
pub fn require_elf_arch(path: &Path, expected_arch: HostArch) -> Result<(), SoftwareError> {
    let mut file = std::fs::File::open(path)?;
    let mut header = [0u8; 64];
    let mut read = 0usize;
    while read < header.len() {
        let n = file.read(&mut header[read..])?;
        if n == 0 {
            break;
        }
        read += n;
    }
    check_elf_bytes(path, &header[..read], expected_arch)
}

/// Pure byte-level ELF64 architecture check (callable on any host OS for tests).
pub fn check_elf_bytes(
    path: &Path,
    bytes: &[u8],
    expected_arch: HostArch,
) -> Result<(), SoftwareError> {
    if bytes.starts_with(b"#!") {
        return Ok(());
    }
    if bytes.len() < 4 || bytes[0..4] != ELF_MAGIC {
        return Err(SoftwareError::invalid(format!(
            "{} is not a Linux ELF executable or interpreter script",
            path.display()
        )));
    }
    if bytes.len() < 20 {
        return Err(cannot_verify_elf(path, "truncated ELF header"));
    }
    let ei_class = bytes[4];
    if ei_class != ELFCLASS64 {
        return Err(SoftwareError::invalid(format!(
            "{} is not a 64-bit ELF executable (EI_CLASS={ei_class})",
            path.display()
        )));
    }
    let ei_data = bytes[5];
    let machine = match ei_data {
        ELFDATA2LSB => u16::from_le_bytes([bytes[18], bytes[19]]),
        ELFDATA2MSB => u16::from_be_bytes([bytes[18], bytes[19]]),
        _ => return Err(cannot_verify_elf(path, "invalid ELF byte order")),
    };
    let (want_machine, want_name) = match expected_arch {
        HostArch::Amd64 => (EM_X86_64, "x86_64"),
        HostArch::Arm64 => (EM_AARCH64, "aarch64"),
    };
    if machine == want_machine {
        return Ok(());
    }
    let found_name = match machine {
        EM_X86_64 => "x86_64",
        EM_AARCH64 => "aarch64",
        _ => "unsupported",
    };
    Err(SoftwareError::invalid(format!(
        "{} is a Linux {found_name} ELF executable (e_machine={machine}); a {want_name} executable is required",
        path.display()
    )))
}

fn cannot_verify(path: &Path, detail: &str) -> SoftwareError {
    SoftwareError::invalid(format!(
        "cannot verify Apple Silicon architecture in {}: {detail}",
        path.display()
    ))
}

fn cannot_verify_elf(path: &Path, detail: &str) -> SoftwareError {
    SoftwareError::invalid(format!(
        "cannot verify Linux ELF architecture in {}: {detail}",
        path.display()
    ))
}

fn read_u32(bytes: &[u8], offset: usize, big_endian: bool) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    let array = [slice[0], slice[1], slice[2], slice[3]];
    Some(if big_endian {
        u32::from_be_bytes(array)
    } else {
        u32::from_le_bytes(array)
    })
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn elf64_header(ei_class: u8, ei_data: u8, machine: u16) -> Vec<u8> {
        let mut buf = vec![0u8; 64];
        buf[0..4].copy_from_slice(&ELF_MAGIC);
        buf[4] = ei_class;
        buf[5] = ei_data;
        buf[6] = 1; // EV_CURRENT
        let machine_bytes = if ei_data == ELFDATA2MSB {
            machine.to_be_bytes()
        } else {
            machine.to_le_bytes()
        };
        buf[18..20].copy_from_slice(&machine_bytes);
        buf
    }

    #[test]
    fn elf_accepts_matching_arch_and_scripts_and_rejects_wrong_or_non_elf() {
        let dir = TempDir::new().unwrap();

        let x86_path = dir.path().join("bin-x86_64");
        std::fs::write(&x86_path, elf64_header(ELFCLASS64, ELFDATA2LSB, EM_X86_64)).unwrap();
        assert!(require_elf_arch(&x86_path, HostArch::Amd64).is_ok());
        let err = require_elf_arch(&x86_path, HostArch::Arm64).unwrap_err();
        assert!(err.to_string().contains("aarch64"), "{err}");

        let arm_path = dir.path().join("bin-aarch64");
        std::fs::write(&arm_path, elf64_header(ELFCLASS64, ELFDATA2LSB, EM_AARCH64)).unwrap();
        assert!(require_elf_arch(&arm_path, HostArch::Arm64).is_ok());
        let err = require_elf_arch(&arm_path, HostArch::Amd64).unwrap_err();
        assert!(err.to_string().contains("x86_64"), "{err}");

        // 32-bit ELF rejected
        let elf32_path = dir.path().join("bin-elf32");
        std::fs::write(&elf32_path, elf64_header(1, ELFDATA2LSB, EM_X86_64)).unwrap();
        assert!(require_elf_arch(&elf32_path, HostArch::Amd64).is_err());

        // Truncated ELF rejected
        let trunc_path = dir.path().join("bin-trunc");
        std::fs::write(&trunc_path, b"\x7fELF\x02\x01").unwrap();
        assert!(require_elf_arch(&trunc_path, HostArch::Amd64).is_err());

        // Non-ELF (Mach-O binary & random bytes) rejected
        let macho_path = dir.path().join("bin-macho");
        let mut macho = Vec::new();
        macho.extend(MH_MAGIC_64.to_be_bytes());
        macho.extend(CPU_TYPE_ARM64.to_be_bytes());
        std::fs::write(&macho_path, macho).unwrap();
        assert!(require_elf_arch(&macho_path, HostArch::Arm64).is_err());

        let garbage_path = dir.path().join("bin-garbage");
        std::fs::write(&garbage_path, b"not-an-elf-binary").unwrap();
        assert!(require_elf_arch(&garbage_path, HostArch::Amd64).is_err());

        // Interpreter scripts (#!) accepted on both arches
        let script_path = dir.path().join("npm");
        std::fs::write(&script_path, b"#!/usr/bin/env node\nconsole.log('ok')\n").unwrap();
        assert!(require_elf_arch(&script_path, HostArch::Amd64).is_ok());
        assert!(require_elf_arch(&script_path, HostArch::Arm64).is_ok());
    }
}
