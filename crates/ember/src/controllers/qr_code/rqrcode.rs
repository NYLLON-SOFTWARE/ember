//! A port of rqrcode_core 2.1.0 and rqrcode 3.2.0's `as_svg` (Rect output), for
//! `QrCodeController#show` (reference/app/controllers/qr_code_controller.rb):
//! `RQRCode::QRCode.new(url).as_svg(viewbox: true, fill: :white, color: :black)`.
//!
//! The output has to be byte-identical to the gem's, so this follows its algorithms rather than
//! the QR spec's optimal choices: a single segment whose mode is numeric, alphanumeric or 8-bit
//! byte (in that order of preference), error correction level H, the smallest version whose
//! capacity is *strictly* greater than the segment's bits, and the mask with the fewest "lost
//! points" as the gem scores them (including its floating-point dark-ratio term).
//! Golden vectors: `reference-tools/campfire/rqrcode.rb`.

/// `RQRCode::QRCode.new(data).as_svg(viewbox: true, fill: :white, color: :black)`, for the binary
/// string `Base64.urlsafe_decode64` returns; `None` when it doesn't fit a version 40 code.
pub fn svg_bytes(data: &[u8]) -> Option<String> {
    let modules = QrCode::new(data)?.modules;
    let module_size = 11;
    let dimension = modules.len() * module_size;
    let mut out = String::with_capacity(256 + modules.len() * modules.len() * 30);
    out.push_str(r#"<?xml version="1.0" standalone="yes"?>"#);
    out.push_str(&format!(
        r#"<svg version="1.1" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:ev="http://www.w3.org/2001/xml-events" viewBox="0 0 {dimension} {dimension}" shape-rendering="crispEdges">"#
    ));
    out.push_str(&format!(r#"<rect width="{dimension}" height="{dimension}" x="0" y="0" fill="white"/>"#));
    for (row, cells) in modules.iter().enumerate() {
        for (col, &dark) in cells.iter().enumerate() {
            if dark {
                let (x, y) = (col * module_size, row * module_size);
                out.push_str(&format!(r#"<rect width="{module_size}" height="{module_size}" x="{x}" y="{y}" fill="black"/>"#));
            }
        }
    }
    out.push_str("</svg>");
    Some(out)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Number = 1,
    AlphaNumeric = 2,
    Byte = 4,
}

const ALPHANUMERIC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

/// `QRERRORCORRECTLEVEL[:h]`
const LEVEL_H: u32 = 2;

/// `QRMAXBITS[:h]`
const MAX_BITS_H: [usize; 40] = [
    72, 128, 208, 288, 368, 480, 528, 688, 800, 976, 1120, 1264, 1440, 1576, 1784, 2024, 2264, 2504, 2728, 3080, 3248, 3536, 3712, 4112,
    4304, 4768, 5024, 5288, 5608, 5960, 6344, 6760, 7208, 7688, 7888, 8432, 8768, 9136, 9776, 10208,
];

/// The H rows of `QRRSBlock::RS_BLOCK_TABLE`: (count, total, data) groups.
const RS_BLOCKS_H: [&[usize]; 40] = [
    &[1, 26, 9],
    &[1, 44, 16],
    &[2, 35, 13],
    &[4, 25, 9],
    &[2, 33, 11, 2, 34, 12],
    &[4, 43, 15],
    &[4, 39, 13, 1, 40, 14],
    &[4, 40, 14, 2, 41, 15],
    &[4, 36, 12, 4, 37, 13],
    &[6, 43, 15, 2, 44, 16],
    &[3, 36, 12, 8, 37, 13],
    &[7, 42, 14, 4, 43, 15],
    &[12, 33, 11, 4, 34, 12],
    &[11, 36, 12, 5, 37, 13],
    &[11, 36, 12, 7, 37, 13],
    &[3, 45, 15, 13, 46, 16],
    &[2, 42, 14, 17, 43, 15],
    &[2, 42, 14, 19, 43, 15],
    &[9, 39, 13, 16, 40, 14],
    &[15, 43, 15, 10, 44, 16],
    &[19, 46, 16, 6, 47, 17],
    &[34, 37, 13],
    &[16, 45, 15, 14, 46, 16],
    &[30, 46, 16, 2, 47, 17],
    &[22, 45, 15, 13, 46, 16],
    &[33, 46, 16, 4, 47, 17],
    &[12, 45, 15, 28, 46, 16],
    &[11, 45, 15, 31, 46, 16],
    &[19, 45, 15, 26, 46, 16],
    &[23, 45, 15, 25, 46, 16],
    &[23, 45, 15, 28, 46, 16],
    &[19, 45, 15, 35, 46, 16],
    &[11, 45, 15, 46, 46, 16],
    &[59, 46, 16, 1, 47, 17],
    &[22, 45, 15, 41, 46, 16],
    &[2, 45, 15, 64, 46, 16],
    &[24, 45, 15, 46, 46, 16],
    &[42, 45, 15, 32, 46, 16],
    &[10, 45, 15, 67, 46, 16],
    &[20, 45, 15, 61, 46, 16],
];

/// `QRUtil::PATTERN_POSITION_TABLE`
const PATTERN_POSITIONS: [&[usize]; 40] = [
    &[],
    &[6, 18],
    &[6, 22],
    &[6, 26],
    &[6, 30],
    &[6, 34],
    &[6, 22, 38],
    &[6, 24, 42],
    &[6, 26, 46],
    &[6, 28, 50],
    &[6, 30, 54],
    &[6, 32, 58],
    &[6, 34, 62],
    &[6, 26, 46, 66],
    &[6, 26, 48, 70],
    &[6, 26, 50, 74],
    &[6, 30, 54, 78],
    &[6, 30, 56, 82],
    &[6, 30, 58, 86],
    &[6, 34, 62, 90],
    &[6, 28, 50, 72, 94],
    &[6, 26, 50, 74, 98],
    &[6, 30, 54, 78, 102],
    &[6, 28, 54, 80, 106],
    &[6, 32, 58, 84, 110],
    &[6, 30, 58, 86, 114],
    &[6, 34, 62, 90, 118],
    &[6, 26, 50, 74, 98, 122],
    &[6, 30, 54, 78, 102, 126],
    &[6, 26, 52, 78, 104, 130],
    &[6, 30, 56, 82, 108, 134],
    &[6, 34, 60, 86, 112, 138],
    &[6, 30, 58, 86, 114, 142],
    &[6, 34, 62, 90, 118, 146],
    &[6, 30, 54, 78, 102, 126, 150],
    &[6, 24, 50, 76, 102, 128, 154],
    &[6, 28, 54, 80, 106, 132, 158],
    &[6, 32, 58, 84, 110, 136, 162],
    &[6, 26, 54, 82, 110, 138, 166],
    &[6, 30, 58, 86, 114, 142, 170],
];

const G15: u32 = (1 << 10) | (1 << 8) | (1 << 5) | (1 << 4) | (1 << 2) | (1 << 1) | 1;
const G18: u32 = (1 << 12) | (1 << 11) | (1 << 10) | (1 << 9) | (1 << 8) | (1 << 5) | (1 << 2) | 1;
const G15_MASK: u32 = (1 << 14) | (1 << 12) | (1 << 10) | (1 << 4) | (1 << 1);

/// `QRSegment`: the whole input in one mode.
struct Segment<'a> {
    data: &'a [u8],
    mode: Mode,
}

impl<'a> Segment<'a> {
    /// `QRSegment.new(data:)` without a mode: numeric, then alphanumeric, then 8-bit byte. The
    /// gem checks `data.chars`; every valid character is ASCII, so checking bytes is the same.
    fn new(data: &'a [u8]) -> Self {
        let mode = if data.iter().all(u8::is_ascii_digit) {
            Mode::Number
        } else if data.iter().all(|b| ALPHANUMERIC.contains(b)) {
            Mode::AlphaNumeric
        } else {
            Mode::Byte
        };
        Self { data, mode }
    }

    /// `QRSegment#size(version)`: bits needed, including the mode indicator and length.
    fn size(&self, version: usize) -> usize {
        4 + length_in_bits(self.mode, version) + self.content_size()
    }

    fn content_size(&self) -> usize {
        let length = self.data.len();
        let (chunk, bits, extra) = match self.mode {
            Mode::Number => (3, 10, [0, 4, 7][length % 3]),
            Mode::AlphaNumeric => (2, 11, 6),
            Mode::Byte => (1, 8, 0),
        };
        (length / chunk) * bits + if length.is_multiple_of(chunk) { 0 } else { extra }
    }

    fn write(&self, buffer: &mut BitBuffer) {
        buffer.put(self.mode as u32, 4);
        buffer.put(self.data.len() as u32, length_in_bits(self.mode, buffer.version));
        match self.mode {
            Mode::Number => {
                for chunk in self.data.chunks(3) {
                    let code = chunk.iter().fold(0u32, |acc, b| acc * 10 + u32::from(b - b'0'));
                    buffer.put(code, [0, 4, 7, 10][chunk.len()]);
                }
            }
            Mode::AlphaNumeric => {
                let index = |b: &u8| ALPHANUMERIC.iter().position(|a| a == b).unwrap() as u32;
                for pair in self.data.chunks(2) {
                    match pair {
                        [a, b] => buffer.put(index(a) * 45 + index(b), 11),
                        [a] => buffer.put(index(a), 6),
                        _ => unreachable!(),
                    }
                }
            }
            Mode::Byte => {
                for &b in self.data {
                    buffer.put(u32::from(b), 8);
                }
            }
        }
    }
}

/// `QRUtil.get_length_in_bits`
fn length_in_bits(mode: Mode, version: usize) -> usize {
    let macro_version = match version {
        1..=9 => 0,
        10..=26 => 1,
        _ => 2,
    };
    match mode {
        Mode::Number => [10, 12, 14][macro_version],
        Mode::AlphaNumeric => [9, 11, 13][macro_version],
        Mode::Byte => [8, 16, 16][macro_version],
    }
}

/// `QRBitBuffer`
struct BitBuffer {
    version: usize,
    buffer: Vec<u8>,
    length: usize,
}

impl BitBuffer {
    fn new(version: usize) -> Self {
        Self { version, buffer: Vec::new(), length: 0 }
    }

    fn put(&mut self, num: u32, length: usize) {
        for i in 0..length {
            self.put_bit((num >> (length - i - 1)) & 1 == 1);
        }
    }

    fn put_bit(&mut self, bit: bool) {
        let index = self.length / 8;
        if self.buffer.len() <= index {
            self.buffer.push(0);
        }
        if bit {
            self.buffer[index] |= 0x80 >> (self.length % 8);
        }
        self.length += 1;
    }

    fn end_of_message(&mut self, max_data_bits: usize) {
        if self.length + 4 <= max_data_bits {
            self.put(0, 4);
        }
    }

    fn pad_until(&mut self, preferred_size: usize) {
        while !self.length.is_multiple_of(8) {
            self.put_bit(false);
        }
        while self.length < preferred_size {
            self.put(0xEC, 8);
            if self.length < preferred_size {
                self.put(0x11, 8);
            }
        }
    }
}

/// `QRMath`: GF(256) exp/log tables.
struct Galois {
    exp: [u32; 256],
    log: [u32; 256],
}

impl Galois {
    fn new() -> Self {
        let mut exp = [0u32; 256];
        let mut log = [0u32; 256];
        for (i, value) in exp.iter_mut().enumerate().take(8) {
            *value = 1 << i;
        }
        for i in 8..256 {
            exp[i] = exp[i - 4] ^ exp[i - 5] ^ exp[i - 6] ^ exp[i - 8];
        }
        for i in 0..255 {
            log[exp[i] as usize] = i as u32;
        }
        Self { exp, log }
    }

    fn glog(&self, n: u32) -> i64 {
        assert!(n >= 1, "glog({n})");
        i64::from(self.log[n as usize])
    }

    fn gexp(&self, mut n: i64) -> u32 {
        while n < 0 {
            n += 255;
        }
        while n >= 256 {
            n -= 255;
        }
        self.exp[n as usize]
    }
}

/// `QRPolynomial`. The gem pads with `nil`s where this pads with zeros; they only differ in
/// inputs where the gem raises.
struct Polynomial(Vec<u32>);

impl Polynomial {
    fn new(num: &[u32], shift: usize) -> Self {
        let offset = num.iter().take_while(|&&n| n == 0).count();
        let mut values = num[offset..].to_vec();
        values.resize(num.len() - offset + shift, 0);
        Self(values)
    }

    fn multiply(&self, other: &Polynomial, gf: &Galois) -> Polynomial {
        let mut num = vec![0u32; self.0.len() + other.0.len() - 1];
        for (i, &a) in self.0.iter().enumerate() {
            for (j, &b) in other.0.iter().enumerate() {
                num[i + j] ^= gf.gexp(gf.glog(a) + gf.glog(b));
            }
        }
        Polynomial::new(&num, 0)
    }

    fn modulo(self, other: &Polynomial, gf: &Galois) -> Polynomial {
        let mut current = self;
        loop {
            if current.0.len() < other.0.len() {
                return current;
            }
            let ratio = gf.glog(current.0[0]) - gf.glog(other.0[0]);
            let mut num = current.0.clone();
            for (i, &value) in other.0.iter().enumerate() {
                num[i] ^= gf.gexp(gf.glog(value) + ratio);
            }
            current = Polynomial::new(&num, 0);
        }
    }
}

/// `QRUtil.get_error_correct_polynomial`
fn error_correct_polynomial(length: usize, gf: &Galois) -> Polynomial {
    let mut a = Polynomial::new(&[1], 0);
    for i in 0..length {
        a = a.multiply(&Polynomial::new(&[1, gf.gexp(i as i64)], 0), gf);
    }
    a
}

/// `QRCode.create_data`: the data and error correction codewords, interleaved.
fn create_data(version: usize, segment: &Segment) -> Vec<u8> {
    let blocks: Vec<(usize, usize)> =
        RS_BLOCKS_H[version - 1].chunks(3).flat_map(|group| std::iter::repeat_n((group[1], group[2]), group[0])).collect();
    let max_data_bits = blocks.iter().map(|(_, data)| data).sum::<usize>() * 8;

    let mut buffer = BitBuffer::new(version);
    segment.write(&mut buffer);
    buffer.end_of_message(max_data_bits);
    assert!(buffer.length <= max_data_bits, "code length overflow");
    buffer.pad_until(max_data_bits);

    let gf = Galois::new();
    let mut offset = 0;
    let mut dc_data: Vec<Vec<u32>> = Vec::new();
    let mut ec_data: Vec<Vec<u32>> = Vec::new();
    for &(total, data_count) in &blocks {
        let ec_count = total - data_count;
        let dc: Vec<u32> = buffer.buffer[offset..offset + data_count].iter().map(|&b| u32::from(b)).collect();
        offset += data_count;
        let rs_poly = error_correct_polynomial(ec_count, &gf);
        let ec_length = rs_poly.0.len() - 1;
        let mod_poly = Polynomial::new(&dc, ec_length).modulo(&rs_poly, &gf);
        let ec: Vec<u32> = (0..ec_length)
            .map(|i| {
                let index = i as i64 + mod_poly.0.len() as i64 - ec_length as i64;
                if index >= 0 { mod_poly.0[index as usize] } else { 0 }
            })
            .collect();
        dc_data.push(dc);
        ec_data.push(ec);
    }

    let mut data = Vec::new();
    for codewords in [&dc_data, &ec_data] {
        let longest = codewords.iter().map(Vec::len).max().unwrap_or(0);
        for i in 0..longest {
            for block in codewords.iter() {
                if let Some(&value) = block.get(i) {
                    data.push(value as u8);
                }
            }
        }
    }
    data
}

/// `QRUtil.get_bch_digit`
fn bch_digit(mut data: u32) -> i32 {
    let mut digit = 0;
    while data != 0 {
        digit += 1;
        data >>= 1;
    }
    digit
}

fn bch_format_info(data: u32) -> u32 {
    let mut d = data << 10;
    while bch_digit(d) - bch_digit(G15) >= 0 {
        d ^= G15 << (bch_digit(d) - bch_digit(G15));
    }
    ((data << 10) | d) ^ G15_MASK
}

fn bch_version(data: u32) -> u32 {
    let mut d = data << 12;
    while bch_digit(d) - bch_digit(G18) >= 0 {
        d ^= G18 << (bch_digit(d) - bch_digit(G18));
    }
    (data << 12) | d
}

/// `QRMASKCOMPUTATIONS`
fn mask(pattern: u32, i: usize, j: usize) -> bool {
    match pattern {
        0 => (i + j).is_multiple_of(2),
        1 => i.is_multiple_of(2),
        2 => j.is_multiple_of(3),
        3 => (i + j).is_multiple_of(3),
        4 => (i / 2 + j / 3).is_multiple_of(2),
        5 => ((i * j) % 2 + (i * j) % 3) == 0,
        6 => ((i * j) % 2 + (i * j) % 3).is_multiple_of(2),
        7 => ((i * j) % 3 + (i + j) % 2).is_multiple_of(2),
        _ => unreachable!(),
    }
}

/// `RQRCodeCore::QRCode`
struct QrCode {
    modules: Vec<Vec<bool>>,
}

type Grid = Vec<Vec<Option<bool>>>;

impl QrCode {
    fn new(data: &[u8]) -> Option<Self> {
        let segment = Segment::new(data);
        let version = minimum_version(&segment)?;
        let count = version * 4 + 17;

        let mut common: Grid = vec![vec![None; count]; count];
        place_position_probe_pattern(&mut common, 0, 0);
        place_position_probe_pattern(&mut common, count - 7, 0);
        place_position_probe_pattern(&mut common, 0, count - 7);
        place_position_adjust_pattern(&mut common, version);
        place_timing_pattern(&mut common);

        let data = create_data(version, &segment);
        let make = |test: bool, pattern: u32| -> Vec<Vec<bool>> {
            let mut grid = common.clone();
            place_format_info(&mut grid, test, pattern);
            if version >= 7 {
                place_version_info(&mut grid, version, test);
            }
            map_data(&mut grid, &data, pattern);
            grid.into_iter().map(|row| row.into_iter().map(|m| m.unwrap_or(false)).collect()).collect()
        };

        // `get_best_mask_pattern`: the first pattern with the fewest lost points.
        let mut best = (0, f64::MAX);
        for pattern in 0..8 {
            let points = lost_points(&make(true, pattern));
            if pattern == 0 || best.1 > points {
                best = (pattern, points);
            }
        }
        Some(Self { modules: make(false, best.0) })
    }
}

/// `minimum_version`: the first version whose capacity is strictly greater than the bits needed,
/// or `None` where rqrcode raises "Data length exceed maximum capacity of version 40".
fn minimum_version(segment: &Segment) -> Option<usize> {
    (1..=40).find(|&version| segment.size(version) < MAX_BITS_H[version - 1])
}

fn place_position_probe_pattern(grid: &mut Grid, row: usize, col: usize) {
    let count = grid.len() as i64;
    for r in -1i64..=7 {
        let y = row as i64 + r;
        if !(0..count).contains(&y) {
            continue;
        }
        for c in -1i64..=7 {
            let x = col as i64 + c;
            if !(0..count).contains(&x) {
                continue;
            }
            let vertical = (0..=6).contains(&r) && (c == 0 || c == 6);
            let horizontal = (0..=6).contains(&c) && (r == 0 || r == 6);
            let square = (2..=4).contains(&r) && (2..=4).contains(&c);
            grid[y as usize][x as usize] = Some(vertical || horizontal || square);
        }
    }
}

fn place_position_adjust_pattern(grid: &mut Grid, version: usize) {
    let positions = PATTERN_POSITIONS[version - 1];
    for &row in positions {
        for &col in positions {
            if grid[row][col].is_some() {
                continue;
            }
            for r in -2i64..=2 {
                for c in -2i64..=2 {
                    let part = r.abs() == 2 || c.abs() == 2 || (r == 0 && c == 0);
                    grid[(row as i64 + r) as usize][(col as i64 + c) as usize] = Some(part);
                }
            }
        }
    }
}

fn place_timing_pattern(grid: &mut Grid) {
    let count = grid.len();
    for (i, row) in grid.iter_mut().enumerate().take(count - 8).skip(8) {
        row[6] = Some(i.is_multiple_of(2));
    }
    for (i, cell) in grid[6].iter_mut().enumerate().take(count - 8).skip(8) {
        *cell = Some(i.is_multiple_of(2));
    }
}

fn place_version_info(grid: &mut Grid, version: usize, test: bool) {
    let count = grid.len();
    let bits = bch_version(version as u32);
    for i in 0..18 {
        let dark = !test && (bits >> i) & 1 == 1;
        grid[i / 3][i % 3 + count - 8 - 3] = Some(dark);
        grid[i % 3 + count - 8 - 3][i / 3] = Some(dark);
    }
}

fn place_format_info(grid: &mut Grid, test: bool, pattern: u32) {
    let count = grid.len();
    let bits = bch_format_info((LEVEL_H << 3) | pattern);
    for i in 0..15 {
        let dark = !test && (bits >> i) & 1 == 1;
        let row = if i < 6 {
            i
        } else if i < 8 {
            i + 1
        } else {
            count - 15 + i
        };
        grid[row][8] = Some(dark);
        let col = if i < 8 {
            count - i - 1
        } else if i < 9 {
            15 - i
        } else {
            15 - i - 1
        };
        grid[8][col] = Some(dark);
    }
    grid[count - 8][8] = Some(!test);
}

fn map_data(grid: &mut Grid, data: &[u8], pattern: u32) {
    let count = grid.len() as i64;
    let mut inc: i64 = -1;
    let mut row: i64 = count - 1;
    let mut bit_index: i32 = 7;
    let mut byte_index = 0;

    let mut col = count - 1;
    while col >= 1 {
        let c0 = if col <= 6 { col - 1 } else { col };
        loop {
            for c in 0..2 {
                let x = (c0 - c) as usize;
                let y = row as usize;
                if grid[y][x].is_none() {
                    let mut dark = byte_index < data.len() && (data[byte_index] >> bit_index) & 1 == 1;
                    if mask(pattern, y, x) {
                        dark = !dark;
                    }
                    grid[y][x] = Some(dark);
                    bit_index -= 1;
                    if bit_index == -1 {
                        byte_index += 1;
                        bit_index = 7;
                    }
                }
            }
            row += inc;
            if row < 0 || count <= row {
                row -= inc;
                inc = -inc;
                break;
            }
        }
        col -= 2;
    }
}

/// `QRUtil.get_lost_points`. The dark-ratio term is a Float in Ruby, so the total is too.
fn lost_points(modules: &[Vec<bool>]) -> f64 {
    let count = modules.len();
    let max = count - 1;
    let mut points: i64 = 0;

    // Same-color neighbours.
    for row in 0..count {
        for col in 0..count {
            let dark = modules[row][col];
            let mut same = 0;
            if row > 0 {
                let above = &modules[row - 1];
                same += (col > 0 && dark == above[col - 1]) as i64;
                same += (dark == above[col]) as i64;
                same += (col < max && dark == above[col + 1]) as i64;
            }
            same += (col > 0 && dark == modules[row][col - 1]) as i64;
            same += (col < max && dark == modules[row][col + 1]) as i64;
            if row < max {
                let below = &modules[row + 1];
                same += (col > 0 && dark == below[col - 1]) as i64;
                same += (dark == below[col]) as i64;
                same += (col < max && dark == below[col + 1]) as i64;
            }
            if same > 5 {
                points += 3 + same - 5;
            }
        }
    }

    // 2x2 blocks.
    for row in 0..max {
        for col in 0..max {
            let value = modules[row][col];
            if value == modules[row + 1][col] && value == modules[row][col + 1] && value == modules[row + 1][col + 1] {
                points += 3;
            }
        }
    }

    // 1:1:3:1:1 patterns, in rows then columns.
    let finder = |cell: &dyn Fn(usize) -> bool| cell(0) && !cell(1) && cell(2) && cell(3) && cell(4) && !cell(5) && cell(6);
    for start in 0..count.saturating_sub(6) {
        for (line, row) in modules.iter().enumerate().take(count) {
            if finder(&|k| row[start + k]) {
                points += 40;
            }
            if finder(&|k| modules[start + k][line]) {
                points += 40;
            }
        }
    }

    // Dark ratio.
    let dark = modules.iter().map(|row| row.iter().filter(|&&m| m).count()).sum::<usize>();
    let ratio = dark as f64 / (count * count) as f64;
    let delta = (100.0 * ratio - 50.0).abs() / 5.0;
    points as f64 + delta * 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Vector {
        input_base64: String,
        version: usize,
        modules: String,
        svg: Option<String>,
    }

    #[test]
    fn matches_rqrcode() {
        let vectors: Vec<Vector> = serde_json::from_str(include_str!("testdata/rqrcode.json")).unwrap();
        assert!(vectors.len() >= 30);
        for vector in vectors {
            let input = rails_compat::encoding::strict_decode(&vector.input_base64).unwrap();
            let segment = Segment::new(&input);
            assert_eq!(minimum_version(&segment), Some(vector.version), "version for {:?}", vector.input_base64);
            let modules: Vec<String> =
                QrCode::new(&input).unwrap().modules.iter().map(|row| row.iter().map(|&m| if m { '1' } else { '0' }).collect()).collect();
            assert_eq!(modules.join("\n"), vector.modules, "modules for {:?}", vector.input_base64);
            if let Some(svg) = vector.svg {
                assert_eq!(svg_bytes(&input).as_deref(), Some(svg.as_str()), "svg for {:?}", vector.input_base64);
            }
        }
    }

    #[test]
    fn data_too_long_for_version_40_is_none() {
        assert!(svg_bytes(&vec![b'a'; 3_000]).is_none());
        assert!(svg_bytes(b"http://campfire.test").is_some());
    }
}
