//! Pure-Rust ISO/IEC 18004 QR Code generator (Byte mode, Versions 1–8,
//! Error Correction Levels L and M) emitting crisp inline SVG markup with
//! zero external dependencies.

use std::fmt::Write as _;

/// Standard ISO/IEC 18004 quiet zone width in modules on every side.
pub const QUIET_ZONE: usize = 4;

/// Supported QR error correction levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcLevel {
    /// ~7% codeword recovery (up to 192 bytes in Version 8).
    Low,
    /// ~15% codeword recovery (up to 152 bytes in Version 8).
    Medium,
}

impl EcLevel {
    /// 2-bit format indicator defined by ISO/IEC 18004 Table 12.
    fn format_bits(self) -> u16 {
        match self {
            EcLevel::Low => 0b01,
            EcLevel::Medium => 0b00,
        }
    }
}

/// One encoded QR Code symbol matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrCode {
    version: u8,
    size: usize,
    ec_level: EcLevel,
    mask: u8,
    modules: Vec<bool>,
}

impl QrCode {
    /// QR version (`1..=8`).
    pub fn version(&self) -> u8 {
        self.version
    }

    /// Matrix width and height in modules (`4 * version + 17`).
    pub fn size(&self) -> usize {
        self.size
    }

    /// Error correction level used to encode this symbol.
    pub fn ec_level(&self) -> EcLevel {
        self.ec_level
    }

    /// Mask pattern (`0..=7`) selected by ISO/IEC 18004 penalty scoring.
    pub fn mask(&self) -> u8 {
        self.mask
    }

    /// Whether the module at `(row, col)` is dark (`true`) or light (`false`).
    pub fn module(&self, row: usize, col: usize) -> bool {
        debug_assert!(row < self.size && col < self.size);
        self.modules[row * self.size + col]
    }

    /// Render the symbol as a crisp, self-contained inline SVG string with a
    /// 4-module white quiet zone and horizontal run-merged integer `<path>`
    /// modules so every module edge lands on exact coordinates.
    pub fn to_svg(&self) -> String {
        let total = self.size + 2 * QUIET_ZONE;
        let mut path = String::with_capacity(self.size * self.size);
        for row in 0..self.size {
            let mut col = 0;
            while col < self.size {
                if !self.module(row, col) {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < self.size && self.module(row, col) {
                    col += 1;
                }
                let run = col - start;
                let x = start + QUIET_ZONE;
                let y = row + QUIET_ZONE;
                let _ = write!(path, "M{x} {y}h{run}v1h-{run}z");
            }
        }
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total} {total}" width="{total}" height="{total}" shape-rendering="crispEdges"><rect width="{total}" height="{total}" fill="#ffffff"/><path d="{path}" fill="#000000"/></svg>"##
        )
    }
}

/// Encode `text` as a crisp inline QR Code SVG string.
///
/// Automatically selects the smallest QR Version 1–8 that fits `text` in Byte
/// mode: uses Error Correction Level M up to 152 bytes, and automatically falls
/// back to Error Correction Level L for 153–192 bytes.
pub fn svg_for_text(text: &str) -> Result<String, String> {
    encode_auto(text.as_bytes()).map(|qr| qr.to_svg())
}

/// Encode arbitrary bytes using Level M when it fits within Versions 1–8,
/// otherwise falling back to Level L (up to 192 bytes).
pub fn encode_auto(data: &[u8]) -> Result<QrCode, String> {
    if data.is_empty() {
        return Err("cannot encode empty text as a QR code".to_string());
    }
    if data.len() <= max_byte_capacity(EcLevel::Medium) {
        encode(data, EcLevel::Medium)
    } else if data.len() <= max_byte_capacity(EcLevel::Low) {
        encode(data, EcLevel::Low)
    } else {
        Err(format!(
            "text length ({} bytes) exceeds QR version 8 capacity ({} bytes)",
            data.len(),
            max_byte_capacity(EcLevel::Low)
        ))
    }
}

/// Encode `data` in ISO/IEC 18004 Byte mode using the smallest Version 1–8
/// for the requested [`EcLevel`].
pub fn encode(data: &[u8], ec_level: EcLevel) -> Result<QrCode, String> {
    if data.is_empty() {
        return Err("cannot encode empty data as a QR code".to_string());
    }
    let spec = select_version(data.len(), ec_level).ok_or_else(|| {
        format!(
            "data length ({} bytes) exceeds QR version 8 capacity ({} bytes for {:?})",
            data.len(),
            max_byte_capacity(ec_level),
            ec_level
        )
    })?;

    let data_codewords = build_byte_codewords(data, spec.total_data_codewords());
    let final_codewords = interleave_with_ecc(&data_codewords, &spec);

    let size = usize::from(spec.version) * 4 + 17;
    let mut base_modules = vec![false; size * size];
    let mut is_function = vec![false; size * size];

    place_function_patterns(spec.version, size, &mut base_modules, &mut is_function);
    place_data_bits(size, &final_codewords, &mut base_modules, &is_function);

    let mut best_mask = 0u8;
    let mut best_penalty = u32::MAX;
    let mut best_modules = Vec::new();

    for mask in 0u8..8 {
        let mut candidate = base_modules.clone();
        apply_mask(size, mask, &mut candidate, &is_function);
        place_format_info(size, ec_level, mask, &mut candidate);
        let penalty = evaluate_penalty(size, &candidate);
        if penalty < best_penalty {
            best_penalty = penalty;
            best_mask = mask;
            best_modules = candidate;
        }
    }

    Ok(QrCode {
        version: spec.version,
        size,
        ec_level,
        mask: best_mask,
        modules: best_modules,
    })
}

// --- Version & RS Block Tables (ISO/IEC 18004 Table 9) -----------------------

#[derive(Debug, Clone, Copy)]
struct VersionSpec {
    version: u8,
    ec_per_block: usize,
    group1_blocks: usize,
    group1_data: usize,
    group2_blocks: usize,
    group2_data: usize,
}

impl VersionSpec {
    const fn total_data_codewords(&self) -> usize {
        self.group1_blocks * self.group1_data + self.group2_blocks * self.group2_data
    }

    /// Maximum payload bytes in Byte mode for Versions 1–8 (4-bit mode + 8-bit count = 12 bits).
    const fn byte_capacity(&self) -> usize {
        (self.total_data_codewords() * 8 - 12) / 8
    }
}

const SPECS_L: [VersionSpec; 8] = [
    VersionSpec {
        version: 1,
        ec_per_block: 7,
        group1_blocks: 1,
        group1_data: 19,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 2,
        ec_per_block: 10,
        group1_blocks: 1,
        group1_data: 34,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 3,
        ec_per_block: 15,
        group1_blocks: 1,
        group1_data: 55,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 4,
        ec_per_block: 20,
        group1_blocks: 1,
        group1_data: 80,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 5,
        ec_per_block: 26,
        group1_blocks: 1,
        group1_data: 108,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 6,
        ec_per_block: 18,
        group1_blocks: 2,
        group1_data: 68,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 7,
        ec_per_block: 20,
        group1_blocks: 2,
        group1_data: 78,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 8,
        ec_per_block: 24,
        group1_blocks: 2,
        group1_data: 97,
        group2_blocks: 0,
        group2_data: 0,
    },
];

const SPECS_M: [VersionSpec; 8] = [
    VersionSpec {
        version: 1,
        ec_per_block: 10,
        group1_blocks: 1,
        group1_data: 16,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 2,
        ec_per_block: 16,
        group1_blocks: 1,
        group1_data: 28,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 3,
        ec_per_block: 26,
        group1_blocks: 1,
        group1_data: 44,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 4,
        ec_per_block: 18,
        group1_blocks: 2,
        group1_data: 32,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 5,
        ec_per_block: 24,
        group1_blocks: 2,
        group1_data: 43,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 6,
        ec_per_block: 16,
        group1_blocks: 4,
        group1_data: 27,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 7,
        ec_per_block: 18,
        group1_blocks: 4,
        group1_data: 31,
        group2_blocks: 0,
        group2_data: 0,
    },
    VersionSpec {
        version: 8,
        ec_per_block: 22,
        group1_blocks: 2,
        group1_data: 38,
        group2_blocks: 2,
        group2_data: 39,
    },
];

fn specs_for(ec_level: EcLevel) -> &'static [VersionSpec; 8] {
    match ec_level {
        EcLevel::Low => &SPECS_L,
        EcLevel::Medium => &SPECS_M,
    }
}

fn max_byte_capacity(ec_level: EcLevel) -> usize {
    specs_for(ec_level)[7].byte_capacity()
}

fn select_version(byte_len: usize, ec_level: EcLevel) -> Option<VersionSpec> {
    specs_for(ec_level)
        .iter()
        .copied()
        .find(|spec| byte_len <= spec.byte_capacity())
}

fn alignment_centers(version: u8) -> &'static [usize] {
    match version {
        1 => &[],
        2 => &[6, 18],
        3 => &[6, 22],
        4 => &[6, 26],
        5 => &[6, 30],
        6 => &[6, 34],
        7 => &[6, 22, 38],
        8 => &[6, 24, 42],
        _ => &[],
    }
}

// --- Data Encoding & GF(256) Reed-Solomon ------------------------------------

fn build_byte_codewords(data: &[u8], total_data_codewords: usize) -> Vec<u8> {
    let mut bits = Vec::with_capacity(total_data_codewords * 8);
    // Byte mode indicator: 0100.
    push_bits(&mut bits, 0b0100, 4);
    // 8-bit character count indicator for Versions 1–8.
    push_bits(&mut bits, data.len() as u32, 8);
    for &byte in data {
        push_bits(&mut bits, u32::from(byte), 8);
    }

    let capacity_bits = total_data_codewords * 8;
    let terminator = (capacity_bits - bits.len()).min(4);
    push_bits(&mut bits, 0, terminator);

    let align = (8 - (bits.len() % 8)) % 8;
    push_bits(&mut bits, 0, align);

    let mut codewords = Vec::with_capacity(total_data_codewords);
    for chunk in bits.as_chunks::<8>().0 {
        let mut byte = 0u8;
        for &bit in chunk {
            byte = (byte << 1) | u8::from(bit);
        }
        codewords.push(byte);
    }

    let pad_bytes = [0xECu8, 0x11u8];
    let mut pad_index = 0usize;
    while codewords.len() < total_data_codewords {
        codewords.push(pad_bytes[pad_index & 1]);
        pad_index += 1;
    }
    codewords
}

fn push_bits(out: &mut Vec<bool>, value: u32, count: usize) {
    for shift in (0..count).rev() {
        out.push(((value >> shift) & 1) != 0);
    }
}

/// Multiply two elements in GF(256) modulo primitive polynomial `x^8 + x^4 + x^3 + x^2 + 1` (`0x11D`).
fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    for _ in 0..8 {
        if (b & 1) != 0 {
            product ^= a;
        }
        let carry = (a & 0x80) != 0;
        a <<= 1;
        if carry {
            a ^= 0x1D;
        }
        b >>= 1;
    }
    product
}

/// Compute Reed-Solomon generator polynomial coefficients of degree `degree`
/// (dropping the implicit leading `1` coefficient for `x^degree`).
fn rs_generator(degree: usize) -> Vec<u8> {
    let mut coeffs = vec![0u8; degree];
    coeffs[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            coeffs[j] = gf_mul(coeffs[j], root);
            if j + 1 < degree {
                coeffs[j] ^= coeffs[j + 1];
            }
        }
        root = gf_mul(root, 2);
    }
    coeffs
}

/// Compute `ec_count` Reed-Solomon error correction codewords for `data`.
fn rs_remainder(data: &[u8], ec_count: usize) -> Vec<u8> {
    let generator = rs_generator(ec_count);
    let mut remainder = vec![0u8; ec_count];
    for &byte in data {
        let factor = byte ^ remainder[0];
        remainder.rotate_left(1);
        remainder[ec_count - 1] = 0;
        for (slot, &coeff) in remainder.iter_mut().zip(generator.iter()) {
            *slot ^= gf_mul(coeff, factor);
        }
    }
    remainder
}

fn interleave_with_ecc(data_codewords: &[u8], spec: &VersionSpec) -> Vec<u8> {
    let total_blocks = spec.group1_blocks + spec.group2_blocks;
    let mut data_blocks: Vec<&[u8]> = Vec::with_capacity(total_blocks);
    let mut ecc_blocks: Vec<Vec<u8>> = Vec::with_capacity(total_blocks);

    let mut offset = 0usize;
    for _ in 0..spec.group1_blocks {
        let slice = &data_codewords[offset..offset + spec.group1_data];
        offset += spec.group1_data;
        ecc_blocks.push(rs_remainder(slice, spec.ec_per_block));
        data_blocks.push(slice);
    }
    for _ in 0..spec.group2_blocks {
        let slice = &data_codewords[offset..offset + spec.group2_data];
        offset += spec.group2_data;
        ecc_blocks.push(rs_remainder(slice, spec.ec_per_block));
        data_blocks.push(slice);
    }
    debug_assert_eq!(offset, data_codewords.len());

    let max_data_len = spec.group1_data.max(spec.group2_data);
    let mut interleaved =
        Vec::with_capacity(data_codewords.len() + total_blocks * spec.ec_per_block);

    for col in 0..max_data_len {
        for block in &data_blocks {
            if col < block.len() {
                interleaved.push(block[col]);
            }
        }
    }
    for col in 0..spec.ec_per_block {
        for ecc in &ecc_blocks {
            interleaved.push(ecc[col]);
        }
    }
    interleaved
}

// --- Matrix Construction & Function Patterns ---------------------------------

fn set_function(
    size: usize,
    row: usize,
    col: usize,
    dark: bool,
    modules: &mut [bool],
    is_function: &mut [bool],
) {
    let idx = row * size + col;
    modules[idx] = dark;
    is_function[idx] = true;
}

fn place_function_patterns(
    version: u8,
    size: usize,
    modules: &mut [bool],
    is_function: &mut [bool],
) {
    // 1. Finder patterns (7x7) and 1-module white separators (8x8 corners).
    place_finder(size, 0, 0, modules, is_function);
    place_finder(size, 0, size - 7, modules, is_function);
    place_finder(size, size - 7, 0, modules, is_function);

    // 2. Timing patterns on row 6 and column 6.
    for i in 8..(size - 8) {
        let dark = (i & 1) == 0;
        set_function(size, 6, i, dark, modules, is_function);
        set_function(size, i, 6, dark, modules, is_function);
    }

    // 3. Dark module at (4 * version + 9, 8) == (size - 8, 8).
    set_function(size, size - 8, 8, true, modules, is_function);

    // 4. Alignment patterns (5x5) for Versions 2–8.
    let centers = alignment_centers(version);
    for (ri, &rc) in centers.iter().enumerate() {
        for (ci, &cc) in centers.iter().enumerate() {
            let last = centers.len() - 1;
            let overlaps_finder =
                (ri == 0 && ci == 0) || (ri == 0 && ci == last) || (ri == last && ci == 0);
            if overlaps_finder {
                continue;
            }
            for dr in -2isize..=2 {
                for dc in -2isize..=2 {
                    let dist = dr.abs().max(dc.abs());
                    let dark = dist != 1;
                    set_function(
                        size,
                        (rc as isize + dr) as usize,
                        (cc as isize + dc) as usize,
                        dark,
                        modules,
                        is_function,
                    );
                }
            }
        }
    }

    // 5. Reserve format information modules (values written per candidate mask).
    for c in 0..=8 {
        if c != 6 {
            set_function(size, 8, c, false, modules, is_function);
        }
    }
    for r in 0..=7 {
        if r != 6 {
            set_function(size, r, 8, false, modules, is_function);
        }
    }
    for r in (size - 7)..size {
        set_function(size, r, 8, false, modules, is_function);
    }
    for c in (size - 8)..size {
        set_function(size, 8, c, false, modules, is_function);
    }

    // 6. Version information (18-bit BCH) for Versions 7 and 8.
    if version >= 7 {
        let bits = version_info_bits(version);
        for i in 0..18usize {
            let dark = ((bits >> i) & 1) != 0;
            let a = i / 3;
            let b = i % 3;
            set_function(size, a, size - 11 + b, dark, modules, is_function);
            set_function(size, size - 11 + b, a, dark, modules, is_function);
        }
    }
}

fn place_finder(
    size: usize,
    top: usize,
    left: usize,
    modules: &mut [bool],
    is_function: &mut [bool],
) {
    for dr in -1isize..=7 {
        for dc in -1isize..=7 {
            let r = top as isize + dr;
            let c = left as isize + dc;
            if r < 0 || c < 0 || r >= size as isize || c >= size as isize {
                continue;
            }
            let dark = if (0..=6).contains(&dr) && (0..=6).contains(&dc) {
                let dist = (dr - 3).abs().max((dc - 3).abs());
                dist != 2
            } else {
                false
            };
            set_function(size, r as usize, c as usize, dark, modules, is_function);
        }
    }
}

fn place_data_bits(size: usize, codewords: &[u8], modules: &mut [bool], is_function: &[bool]) {
    let total_bits = codewords.len() * 8;
    let mut bit_index = 0usize;
    let mut upward = true;
    let mut right = size as isize - 1;

    while right >= 1 {
        if right == 6 {
            right -= 1;
        }
        for step in 0..size {
            let row = if upward { size - 1 - step } else { step };
            for dx in 0..2 {
                let col = (right - dx) as usize;
                let idx = row * size + col;
                if is_function[idx] {
                    continue;
                }
                let dark = if bit_index < total_bits {
                    let byte = codewords[bit_index / 8];
                    let shift = 7 - (bit_index % 8);
                    ((byte >> shift) & 1) != 0
                } else {
                    false
                };
                modules[idx] = dark;
                bit_index += 1;
            }
        }
        upward = !upward;
        right -= 2;
    }
    debug_assert!(bit_index >= total_bits);
}

// --- Masking, Format Info, Version Info, and Penalty Scoring -----------------

fn mask_condition(mask: u8, row: usize, col: usize) -> bool {
    match mask {
        0 => (row + col).is_multiple_of(2),
        1 => row.is_multiple_of(2),
        2 => col.is_multiple_of(3),
        3 => (row + col).is_multiple_of(3),
        4 => ((row / 2) + (col / 3)).is_multiple_of(2),
        5 => ((row * col) % 2) + ((row * col) % 3) == 0,
        6 => (((row * col) % 2) + ((row * col) % 3)).is_multiple_of(2),
        7 => (((row + col) % 2) + ((row * col) % 3)).is_multiple_of(2),
        _ => unreachable!("mask pattern is in 0..8"),
    }
}

fn apply_mask(size: usize, mask: u8, modules: &mut [bool], is_function: &[bool]) {
    for row in 0..size {
        for col in 0..size {
            let idx = row * size + col;
            if !is_function[idx] && mask_condition(mask, row, col) {
                modules[idx] = !modules[idx];
            }
        }
    }
}

/// 15-bit BCH(15, 5) format information with ISO/IEC 18004 mask `0x5412`.
fn format_info_bits(ec_level: EcLevel, mask: u8) -> u16 {
    let data = (ec_level.format_bits() << 3) | u16::from(mask & 0b111);
    let mut rem = data << 10;
    for bit in (10..15).rev() {
        if ((rem >> bit) & 1) != 0 {
            rem ^= 0x537 << (bit - 10);
        }
    }
    ((data << 10) | rem) ^ 0x5412
}

/// 18-bit BCH(18, 6) version information for Versions 7–40 (used for 7 and 8).
fn version_info_bits(version: u8) -> u32 {
    let data = u32::from(version);
    let mut rem = data << 12;
    for bit in (12..18).rev() {
        if ((rem >> bit) & 1) != 0 {
            rem ^= 0x1F25 << (bit - 12);
        }
    }
    (data << 12) | rem
}

fn place_format_info(size: usize, ec_level: EcLevel, mask: u8, modules: &mut [bool]) {
    let bits = format_info_bits(ec_level, mask);
    let bit = |index: u16| ((bits >> index) & 1) != 0;

    // Copy 1: around top-left finder (MSB = bit 14 to LSB = bit 0).
    let copy1: [(usize, usize); 15] = [
        (8, 0),
        (8, 1),
        (8, 2),
        (8, 3),
        (8, 4),
        (8, 5),
        (8, 7),
        (8, 8),
        (7, 8),
        (5, 8),
        (4, 8),
        (3, 8),
        (2, 8),
        (1, 8),
        (0, 8),
    ];
    for (i, &(r, c)) in copy1.iter().enumerate() {
        modules[r * size + c] = bit(14 - i as u16);
    }

    // Copy 2: bottom-left vertical (bits 14..=8) and top-right horizontal (bits 7..=0).
    for i in 0..7 {
        modules[(size - 1 - i) * size + 8] = bit(14 - i as u16);
    }
    for i in 0..8 {
        modules[8 * size + (size - 8 + i)] = bit(7 - i as u16);
    }
}

/// ISO/IEC 18004 Section 7.8.3.1 mask evaluation penalty score.
fn evaluate_penalty(size: usize, modules: &[bool]) -> u32 {
    let mut penalty = 0u32;

    // Rule 1 (adjacent runs >= 5) and Rule 3 (finder-like 1:1:3:1:1 patterns) on rows & cols.
    let mut line = vec![false; size];
    for row in 0..size {
        line.copy_from_slice(&modules[row * size..(row + 1) * size]);
        penalty += line_penalty(&line);
    }
    for col in 0..size {
        for row in 0..size {
            line[row] = modules[row * size + col];
        }
        penalty += line_penalty(&line);
    }

    // Rule 2: 2x2 blocks of identical color (+3 each).
    for row in 0..(size - 1) {
        for col in 0..(size - 1) {
            let color = modules[row * size + col];
            if modules[row * size + col + 1] == color
                && modules[(row + 1) * size + col] == color
                && modules[(row + 1) * size + col + 1] == color
            {
                penalty += 3;
            }
        }
    }

    // Rule 4: dark/light balance deviation from 50% in 5% increments (+10 each).
    let dark = modules.iter().filter(|&&m| m).count();
    let total = size * size;
    let steps = (dark * 20).abs_diff(total * 10) / total;
    penalty += (steps as u32) * 10;

    penalty
}

fn line_penalty(line: &[bool]) -> u32 {
    const PATTERN_A: [bool; 11] = [
        true, false, true, true, true, false, true, false, false, false, false,
    ];
    const PATTERN_B: [bool; 11] = [
        false, false, false, false, true, false, true, true, true, false, true,
    ];

    let mut penalty = 0u32;
    let mut run_color = line[0];
    let mut run_len = 1usize;
    for &cell in &line[1..] {
        if cell == run_color {
            run_len += 1;
        } else {
            if run_len >= 5 {
                penalty += 3 + (run_len - 5) as u32;
            }
            run_color = cell;
            run_len = 1;
        }
    }
    if run_len >= 5 {
        penalty += 3 + (run_len - 5) as u32;
    }

    for window in line.windows(11) {
        if window == PATTERN_A || window == PATTERN_B {
            penalty += 40;
        }
    }
    penalty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reed_solomon_matches_iso_18004_annex_i_vector() {
        // ISO/IEC 18004 Annex I Version 1-M ("01234567" alphanumeric data codewords).
        let data = [
            32, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17,
        ];
        let expected_ecc = [196, 35, 39, 119, 235, 215, 231, 226, 93, 23];
        assert_eq!(rs_remainder(&data, 10), expected_ecc);
    }

    #[test]
    fn format_and_version_bch_bits_match_iso_tables() {
        // ISO/IEC 18004 Annex C Table C.1
        assert_eq!(format_info_bits(EcLevel::Medium, 0), 0x5412);
        assert_eq!(format_info_bits(EcLevel::Low, 0), 0x77C4);
        assert_eq!(format_info_bits(EcLevel::Medium, 2), 0x5E7C);
        assert_eq!(format_info_bits(EcLevel::Low, 7), 0x6976);
        // ISO/IEC 18004 Annex D Table D.1
        assert_eq!(version_info_bits(7), 0x07C94);
        assert_eq!(version_info_bits(8), 0x085BC);
    }

    #[test]
    fn auto_version_selection_scales_from_short_to_long_urls() {
        let short = encode_auto(b"https://a.co").unwrap();
        assert_eq!(short.version(), 1);
        assert_eq!(short.size(), 21);
        assert_eq!(short.ec_level(), EcLevel::Medium);

        let typical = encode_auto(b"https://devbase-app.my-subdomain.workers.dev").unwrap();
        assert_eq!(typical.version(), 4);
        assert_eq!(typical.size(), 33);
        assert_eq!(typical.ec_level(), EcLevel::Medium);

        // 152 bytes is the exact V8-M boundary; 153 bytes switches to V8-L; 192 bytes is V8-L max.
        let max_m = "a".repeat(152);
        let qr_m = encode_auto(max_m.as_bytes()).unwrap();
        assert_eq!(qr_m.version(), 8);
        assert_eq!(qr_m.size(), 49);
        assert_eq!(qr_m.ec_level(), EcLevel::Medium);

        let fallback_l = "b".repeat(160);
        let qr_l = encode_auto(fallback_l.as_bytes()).unwrap();
        assert_eq!(qr_l.version(), 8);
        assert_eq!(qr_l.size(), 49);
        assert_eq!(qr_l.ec_level(), EcLevel::Low);

        let max_l = "c".repeat(192);
        let qr_max = encode_auto(max_l.as_bytes()).unwrap();
        assert_eq!(qr_max.version(), 8);
        assert_eq!(qr_max.ec_level(), EcLevel::Low);
    }

    #[test]
    fn empty_and_overlong_inputs_return_clear_errors() {
        let empty_err = svg_for_text("").unwrap_err();
        assert!(empty_err.contains("empty"));

        let too_long = "x".repeat(193);
        let overlong_err = svg_for_text(&too_long).unwrap_err();
        assert!(overlong_err.contains("exceeds"));

        assert!(encode(b"", EcLevel::Low).is_err());
        assert!(encode(&[b'z'; 153], EcLevel::Medium).is_err());
    }

    #[test]
    fn encoding_is_deterministic_and_preserves_finder_timing_and_dark_modules() {
        for version_sample in [
            "https://a.io",
            "https://app.example.com",
            "https://devbase-api.example-team.workers.dev/health",
            &format!("https://long.example.com/{}", "p".repeat(100)),
            &format!("https://v8.example.com/{}", "q".repeat(130)),
        ] {
            let first = encode_auto(version_sample.as_bytes()).unwrap();
            let second = encode_auto(version_sample.as_bytes()).unwrap();
            assert_eq!(first, second, "QR matrix must be deterministic");

            let size = first.size();
            // Verify all three 7x7 finder patterns and their adjacent white separators.
            for (top, left) in [(0, 0), (0, size - 7), (size - 7, 0)] {
                for dr in 0..7 {
                    for dc in 0..7 {
                        let dist = (dr as isize - 3).abs().max((dc as isize - 3).abs());
                        assert_eq!(
                            first.module(top + dr, left + dc),
                            dist != 2,
                            "finder mismatch at ({}, {}) for v{}",
                            top + dr,
                            left + dc,
                            first.version()
                        );
                    }
                }
            }
            // White separator strips around top-left finder.
            for i in 0..8 {
                assert!(!first.module(7, i));
                assert!(!first.module(i, 7));
            }
            // Alternating timing patterns.
            for i in 8..(size - 8) {
                assert_eq!(first.module(6, i), i % 2 == 0);
                assert_eq!(first.module(i, 6), i % 2 == 0);
            }
            // Fixed dark module.
            assert!(first.module(size - 8, 8));
        }
    }

    #[test]
    fn svg_output_has_quiet_zone_valid_markup_and_exact_module_bounds() {
        let url = "https://www.example.com";
        let qr = encode_auto(url.as_bytes()).unwrap();
        let svg = svg_for_text(url).unwrap();
        let expected_total = qr.size() + 2 * QUIET_ZONE;

        assert!(svg.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg""#));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains(&format!(
            r#"viewBox="0 0 {expected_total} {expected_total}""#
        )));
        assert!(svg.contains(&format!(
            r##"<rect width="{expected_total}" height="{expected_total}" fill="#ffffff"/>"##
        )));
        assert!(svg.contains(r##"<path d=""##));
        assert!(svg.contains(r##"fill="#000000"/>"##));
        // Top-left finder's top bar is 7 dark modules starting at (QUIET_ZONE, QUIET_ZONE).
        assert!(
            svg.contains(&format!("M{QUIET_ZONE} {QUIET_ZONE}h7v1h-7z")),
            "quiet zone offset and top-left finder path must match"
        );
        // Deterministic SVG string.
        assert_eq!(svg, svg_for_text(url).unwrap());
    }

    #[test]
    fn all_versions_1_through_8_have_exact_non_function_bit_counts() {
        let expected_remainder = [0usize, 7, 7, 7, 7, 7, 0, 0];
        for (idx, spec) in SPECS_M.iter().enumerate() {
            let size = usize::from(spec.version) * 4 + 17;
            let mut modules = vec![false; size * size];
            let mut is_function = vec![false; size * size];
            place_function_patterns(spec.version, size, &mut modules, &mut is_function);
            let available = is_function.iter().filter(|&&f| !f).count();
            let total_codewords = spec.total_data_codewords()
                + (spec.group1_blocks + spec.group2_blocks) * spec.ec_per_block;
            assert_eq!(
                available,
                total_codewords * 8 + expected_remainder[idx],
                "non-function module count mismatch for version {}",
                spec.version
            );
        }
    }
}
