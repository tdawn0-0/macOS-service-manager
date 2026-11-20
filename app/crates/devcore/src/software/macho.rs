//! Mach-O arm64 slice verification, ported from the Go `architecture.go:11-47`.
//!
//! Every managed executable must contain an arm64 slice. Non-Mach-O files
//! (scripts with an interpreter) are accepted; thin Intel-only binaries and
//! universal binaries without arm64 are rejected before publication.

use std::path::Path;

use super::SoftwareError;

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

fn cannot_verify(path: &Path, detail: &str) -> SoftwareError {
    SoftwareError::invalid(format!(
        "cannot verify Apple Silicon architecture in {}: {detail}",
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
