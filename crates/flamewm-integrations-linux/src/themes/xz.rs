//! Bounded pure-Rust `.xz` reader: one stream, LZMA2 blocks, CRC32/CRC64/no check.
//! Header-derived sizes are capped before allocation; every malformed input
//! yields `io::Error` and never panics. Output size is bounded by the caller
//! (`take`), and the dictionary is allocated lazily as data arrives.

use std::io::{self, Read};

const MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];
const FOOTER_MAGIC: [u8; 2] = *b"YZ";
const LZMA2_ID: u64 = 0x21;
const MAX_DICT: u64 = 64 * 1024 * 1024;
const MAX_BLOCKS: usize = 1 << 16;
const PROB_INIT: u16 = 1 << 10;

const fn make_crc32() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

const fn make_crc64() -> [u64; 256] {
    let mut table = [0u64; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u64;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xC96C_5795_D787_0F42 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC32_TABLE: [u32; 256] = make_crc32();
static CRC64_TABLE: [u64; 256] = make_crc64();

/// Streaming CRC32 (IEEE, reflected). Start from `0`.
fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = CRC32_TABLE[usize::from((c as u8) ^ b)] ^ (c >> 8);
    }
    !c
}

/// Streaming CRC64 (ECMA-182, reflected, as used by xz). Start from `0`.
fn crc64_update(crc: u64, data: &[u8]) -> u64 {
    let mut c = !crc;
    for &b in data {
        c = CRC64_TABLE[usize::from((c as u8) ^ b)] ^ (c >> 8);
    }
    !c
}

fn corrupt(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("xz: {msg}"))
}

fn unsupported(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, format!("xz: {msg}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Check {
    None,
    Crc32,
    Crc64,
}

impl Check {
    fn size(self) -> usize {
        match self {
            Self::None => 0,
            Self::Crc32 => 4,
            Self::Crc64 => 8,
        }
    }
}

#[derive(Clone, Copy)]
struct Digest {
    check: Check,
    crc32: u32,
    crc64: u64,
}

impl Digest {
    fn update(&mut self, data: &[u8]) {
        match self.check {
            Check::None => {}
            Check::Crc32 => self.crc32 = crc32_update(self.crc32, data),
            Check::Crc64 => self.crc64 = crc64_update(self.crc64, data),
        }
    }

    fn matches(&self, stored: &[u8]) -> bool {
        match self.check {
            Check::None => true,
            Check::Crc32 => stored == &self.crc32.to_le_bytes()[..],
            Check::Crc64 => stored == &self.crc64.to_le_bytes()[..],
        }
    }
}

/// Input wrapper tracking consumed bytes and a running CRC32 for index validation.
struct Input<R> {
    inner: R,
    pos: u64,
    crc: u32,
}

impl<R: Read> Input<R> {
    fn fill(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.inner.read_exact(buf)?;
        self.pos += buf.len() as u64;
        self.crc = crc32_update(self.crc, buf);
        Ok(())
    }

    fn byte(&mut self) -> io::Result<u8> {
        let mut b = [0u8; 1];
        self.fill(&mut b)?;
        Ok(b[0])
    }

    fn be16(&mut self) -> io::Result<u16> {
        let mut b = [0u8; 2];
        self.fill(&mut b)?;
        Ok(u16::from_be_bytes(b))
    }
}

/// xz multibyte integer: 7 bits per byte, at most 9 bytes, value below 2^63.
fn decode_varint(mut next: impl FnMut() -> io::Result<u8>) -> io::Result<u64> {
    let mut value = 0u64;
    for i in 0..9u32 {
        let b = next()?;
        if i == 8 && b > 0x01 {
            return Err(corrupt("varint overflow"));
        }
        value |= u64::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(corrupt("varint too long"))
}

/// Dictionary size from the LZMA2 property byte (xz spec), capped by `MAX_DICT`.
fn dict_size(prop: u8) -> io::Result<usize> {
    if prop > 40 {
        return Err(corrupt("invalid LZMA2 dictionary property"));
    }
    let size = if prop == 40 {
        u64::from(u32::MAX)
    } else {
        (2 | u64::from(prop & 1)) << (prop / 2 + 11)
    };
    if size > MAX_DICT {
        return Err(unsupported("dictionary size exceeds cap"));
    }
    usize::try_from(size).map_err(|_| unsupported("dictionary size exceeds cap"))
}

/// Ring buffer of the last `cap` output bytes; grows only as data arrives.
struct Dict {
    buf: Vec<u8>,
    cap: usize,
    pos: usize,
    avail: usize,
    total: u64,
}

impl Dict {
    fn new(cap: usize) -> Self {
        Self { buf: Vec::new(), cap, pos: 0, avail: 0, total: 0 }
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.pos = 0;
        self.avail = 0;
        self.total = 0;
    }

    fn put(&mut self, b: u8) {
        if self.buf.len() < self.cap {
            self.buf.push(b);
        } else {
            self.buf[self.pos] = b;
        }
        self.pos = if self.pos + 1 == self.cap { 0 } else { self.pos + 1 };
        self.avail = (self.avail + 1).min(self.cap);
        self.total = self.total.wrapping_add(1);
    }

    /// Byte `dist + 1` positions back (`dist` is zero-based), if in history.
    fn back(&self, dist: usize) -> Option<u8> {
        if dist >= self.avail {
            return None;
        }
        Some(self.buf[(self.pos + self.cap - 1 - dist) % self.cap])
    }
}

/// LZMA range decoder over one in-memory LZMA2 chunk.
struct Rc<'a> {
    data: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}

impl<'a> Rc<'a> {
    fn new(data: &'a [u8]) -> io::Result<Self> {
        if data.len() < 5 || data[0] != 0 {
            return Err(corrupt("bad LZMA range coder init"));
        }
        let code = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        if code == u32::MAX {
            return Err(corrupt("bad LZMA range coder init"));
        }
        Ok(Self { data, pos: 5, range: u32::MAX, code })
    }

    fn normalize(&mut self) -> io::Result<()> {
        if self.range < (1 << 24) {
            let b = *self.data.get(self.pos).ok_or_else(|| corrupt("truncated LZMA chunk"))?;
            self.pos += 1;
            self.range <<= 8;
            self.code = (self.code << 8) | u32::from(b);
        }
        Ok(())
    }

    fn bit(&mut self, prob: &mut u16) -> io::Result<u32> {
        self.normalize()?;
        let bound = (self.range >> 11) * u32::from(*prob);
        if self.code < bound {
            self.range = bound;
            *prob += (2048 - *prob) >> 5;
            Ok(0)
        } else {
            self.range -= bound;
            self.code -= bound;
            *prob -= *prob >> 5;
            Ok(1)
        }
    }

    fn direct(&mut self, count: u32) -> io::Result<u32> {
        let mut res = 0u32;
        for _ in 0..count {
            self.normalize()?;
            self.range >>= 1;
            let bit = if self.code >= self.range {
                self.code -= self.range;
                1
            } else {
                0
            };
            res = (res << 1) | bit;
        }
        Ok(res)
    }

    fn bittree(&mut self, probs: &mut [u16], bits: u32) -> io::Result<u32> {
        let mut m = 1usize;
        for _ in 0..bits {
            m = (m << 1) | self.bit(&mut probs[m])? as usize;
        }
        Ok(m as u32 - (1 << bits))
    }

    fn reverse_bittree(&mut self, probs: &mut [u16], base: usize, bits: u32) -> io::Result<u32> {
        let mut m = 1usize;
        let mut sym = 0u32;
        for i in 0..bits {
            let bit = self.bit(&mut probs[base + m])?;
            m = (m << 1) | bit as usize;
            sym |= bit << i;
        }
        Ok(sym)
    }

    /// The chunk must end exactly on a fully consumed range coder.
    fn finish(&mut self) -> io::Result<()> {
        self.normalize()?;
        if self.code != 0 || self.pos != self.data.len() {
            return Err(corrupt("LZMA chunk size mismatch"));
        }
        Ok(())
    }
}

struct LenCoder {
    choice: u16,
    choice2: u16,
    low: [[u16; 8]; 16],
    mid: [[u16; 8]; 16],
    high: [u16; 256],
}

impl LenCoder {
    fn new() -> Self {
        Self {
            choice: PROB_INIT,
            choice2: PROB_INIT,
            low: [[PROB_INIT; 8]; 16],
            mid: [[PROB_INIT; 8]; 16],
            high: [PROB_INIT; 256],
        }
    }

    fn decode(&mut self, rc: &mut Rc<'_>, pos_state: usize) -> io::Result<u32> {
        if rc.bit(&mut self.choice)? == 0 {
            return rc.bittree(&mut self.low[pos_state], 3);
        }
        if rc.bit(&mut self.choice2)? == 0 {
            return Ok(8 + rc.bittree(&mut self.mid[pos_state], 3)?);
        }
        Ok(16 + rc.bittree(&mut self.high, 8)?)
    }
}

/// LZMA state machine. `Lzma::new` gives the reset state for the given properties.
struct Lzma {
    lc: u32,
    lp: u32,
    pb: u32,
    literal: Vec<u16>,
    is_match: [u16; 192],
    is_rep: [u16; 12],
    is_rep_g0: [u16; 12],
    is_rep_g1: [u16; 12],
    is_rep_g2: [u16; 12],
    is_rep0_long: [u16; 192],
    pos_slot: [[u16; 64]; 4],
    pos_special: [u16; 115],
    align: [u16; 16],
    len: LenCoder,
    rep_len: LenCoder,
    state: usize,
    reps: [u32; 4],
}

impl Lzma {
    fn new(lc: u32, lp: u32, pb: u32) -> Self {
        Self {
            lc,
            lp,
            pb,
            literal: vec![PROB_INIT; 0x300 << (lc + lp)],
            is_match: [PROB_INIT; 192],
            is_rep: [PROB_INIT; 12],
            is_rep_g0: [PROB_INIT; 12],
            is_rep_g1: [PROB_INIT; 12],
            is_rep_g2: [PROB_INIT; 12],
            is_rep0_long: [PROB_INIT; 192],
            pos_slot: [[PROB_INIT; 64]; 4],
            pos_special: [PROB_INIT; 115],
            align: [PROB_INIT; 16],
            len: LenCoder::new(),
            rep_len: LenCoder::new(),
            state: 0,
            reps: [0; 4],
        }
    }

    fn distance(&mut self, rc: &mut Rc<'_>, len: u32) -> io::Result<u32> {
        let len_state = len.min(3) as usize;
        let slot = rc.bittree(&mut self.pos_slot[len_state], 6)?;
        if slot < 4 {
            return Ok(slot);
        }
        let direct = (slot >> 1) - 1;
        let mut dist = (2 | (slot & 1)) << direct;
        if slot < 14 {
            dist += rc.reverse_bittree(&mut self.pos_special, (dist - slot) as usize, direct)?;
        } else {
            dist = dist.checked_add(rc.direct(direct - 4)? << 4).ok_or_else(|| corrupt("distance overflow"))?;
            dist += rc.reverse_bittree(&mut self.align, 0, 4)?;
        }
        Ok(dist)
    }

    /// Decode symbols until exactly `want` bytes have been appended to `out`.
    fn decode(&mut self, rc: &mut Rc<'_>, dict: &mut Dict, out: &mut Vec<u8>, want: usize) -> io::Result<()> {
        let start = out.len();
        let end = start + want;
        let pb_mask = (1usize << self.pb) - 1;
        let lp_mask = (1usize << self.lp) - 1;
        while out.len() < end {
            let pos_state = (dict.total as usize) & pb_mask;
            let st = self.state;
            if rc.bit(&mut self.is_match[(st << 4) + pos_state])? == 0 {
                let prev = dict.back(0).unwrap_or(0);
                let lit_state = (((dict.total as usize) & lp_mask) << self.lc) + (usize::from(prev) >> (8 - self.lc));
                let base = 0x300 * lit_state;
                let probs = &mut self.literal[base..base + 0x300];
                let mut sym = 1usize;
                if st >= 7 {
                    let mut match_byte = usize::from(dict.back(self.reps[0] as usize).ok_or_else(|| corrupt("bad literal match"))?);
                    while sym < 0x100 {
                        let match_bit = (match_byte >> 7) & 1;
                        match_byte <<= 1;
                        let bit = rc.bit(&mut probs[((1 + match_bit) << 8) + sym])? as usize;
                        sym = (sym << 1) | bit;
                        if match_bit != bit {
                            break;
                        }
                    }
                }
                while sym < 0x100 {
                    sym = (sym << 1) | rc.bit(&mut probs[sym])? as usize;
                }
                let byte = (sym - 0x100) as u8;
                out.push(byte);
                dict.put(byte);
                self.state = if st < 4 {
                    0
                } else if st < 10 {
                    st - 3
                } else {
                    st - 6
                };
                continue;
            }
            let len;
            if rc.bit(&mut self.is_rep[st])? == 0 {
                len = self.len.decode(rc, pos_state)?;
                self.state = if st < 7 { 7 } else { 10 };
                let dist = self.distance(rc, len)?;
                self.reps = [dist, self.reps[0], self.reps[1], self.reps[2]];
            } else {
                if rc.bit(&mut self.is_rep_g0[st])? == 0 {
                    if rc.bit(&mut self.is_rep0_long[(st << 4) + pos_state])? == 0 {
                        self.state = if st < 7 { 9 } else { 11 };
                        let byte = dict.back(self.reps[0] as usize).ok_or_else(|| corrupt("bad short rep"))?;
                        out.push(byte);
                        dict.put(byte);
                        continue;
                    }
                } else {
                    let dist;
                    if rc.bit(&mut self.is_rep_g1[st])? == 0 {
                        dist = self.reps[1];
                    } else {
                        if rc.bit(&mut self.is_rep_g2[st])? == 0 {
                            dist = self.reps[2];
                        } else {
                            dist = self.reps[3];
                            self.reps[3] = self.reps[2];
                        }
                        self.reps[2] = self.reps[1];
                    }
                    self.reps[1] = self.reps[0];
                    self.reps[0] = dist;
                }
                len = self.rep_len.decode(rc, pos_state)?;
                self.state = if st < 7 { 8 } else { 11 };
            }
            let n = len as usize + 2;
            if out.len() + n > end {
                return Err(corrupt("LZMA match crosses chunk boundary"));
            }
            let dist = self.reps[0] as usize;
            for _ in 0..n {
                let byte = dict.back(dist).ok_or_else(|| corrupt("LZMA distance out of range"))?;
                out.push(byte);
                dict.put(byte);
            }
        }
        Ok(())
    }
}

struct Block {
    header_size: u64,
    data_start: u64,
    declared_compressed: Option<u64>,
    declared_uncompressed: Option<u64>,
    digest: Digest,
    out_len: u64,
    dict: Dict,
    lzma: Option<Lzma>,
    need_dict_reset: bool,
    need_props: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    StreamHeader,
    BlockOrIndex,
    Chunks,
    Done,
}

/// Single-stream `.xz` decoder implementing `Read`.
pub(crate) struct XzReader<R: Read> {
    input: Input<R>,
    phase: Phase,
    stream_flags: [u8; 2],
    check: Check,
    records: Vec<(u64, u64)>,
    block: Option<Block>,
    pending: Vec<u8>,
    pending_pos: usize,
}

impl<R: Read> XzReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            input: Input { inner, pos: 0, crc: 0 },
            phase: Phase::StreamHeader,
            stream_flags: [0; 2],
            check: Check::None,
            records: Vec::new(),
            block: None,
            pending: Vec::new(),
            pending_pos: 0,
        }
    }

    fn step(&mut self) -> io::Result<()> {
        match self.phase {
            Phase::StreamHeader => self.read_stream_header(),
            Phase::BlockOrIndex => {
                self.input.crc = 0;
                let index_start = self.input.pos;
                match self.input.byte()? {
                    0 => self.read_index_and_footer(index_start),
                    size => self.start_block(size),
                }
            }
            Phase::Chunks => match self.input.byte()? {
                0x00 => self.finish_block(),
                control @ (0x01 | 0x02) => self.uncompressed_chunk(control),
                control @ 0x80..=0xFF => self.lzma_chunk(control),
                _ => Err(corrupt("invalid LZMA2 control byte")),
            },
            Phase::Done => Ok(()),
        }
    }

    fn read_stream_header(&mut self) -> io::Result<()> {
        let mut h = [0u8; 12];
        self.input.fill(&mut h)?;
        if !h.starts_with(&MAGIC) {
            return Err(corrupt("not an xz stream"));
        }
        let flags = [h[6], h[7]];
        if crc32_update(0, &flags) != u32::from_le_bytes([h[8], h[9], h[10], h[11]]) {
            return Err(corrupt("stream header CRC mismatch"));
        }
        if flags[0] != 0 || flags[1] & 0xF0 != 0 {
            return Err(corrupt("invalid stream flags"));
        }
        self.check = match flags[1] & 0x0F {
            0x00 => Check::None,
            0x01 => Check::Crc32,
            0x04 => Check::Crc64,
            0x0A => return Err(unsupported("SHA-256 check")),
            _ => return Err(unsupported("reserved check type")),
        };
        self.stream_flags = flags;
        self.phase = Phase::BlockOrIndex;
        Ok(())
    }

    fn start_block(&mut self, size_byte: u8) -> io::Result<()> {
        let header_size = (usize::from(size_byte) + 1) * 4;
        let mut h = vec![0u8; header_size];
        h[0] = size_byte;
        self.input.fill(&mut h[1..])?;
        let body = &h[..header_size - 4];
        let stored = u32::from_le_bytes([h[header_size - 4], h[header_size - 3], h[header_size - 2], h[header_size - 1]]);
        if crc32_update(0, body) != stored {
            return Err(corrupt("block header CRC mismatch"));
        }
        let mut at = 1;
        let mut next = || -> io::Result<u8> {
            let b = *body.get(at).ok_or_else(|| corrupt("truncated block header"))?;
            at += 1;
            Ok(b)
        };
        let flags = next()?;
        if flags & 0x3C != 0 {
            return Err(corrupt("reserved block header flags"));
        }
        if flags & 0x03 != 0 {
            return Err(unsupported("multi-filter chains"));
        }
        let declared_compressed = if flags & 0x40 != 0 { Some(decode_varint(&mut next)?) } else { None };
        let declared_uncompressed = if flags & 0x80 != 0 { Some(decode_varint(&mut next)?) } else { None };
        if decode_varint(&mut next)? != LZMA2_ID {
            return Err(unsupported("non-LZMA2 filter"));
        }
        if decode_varint(&mut next)? != 1 {
            return Err(corrupt("invalid LZMA2 filter properties"));
        }
        let dict = dict_size(next()?)?;
        if body[at..].iter().any(|&b| b != 0) {
            return Err(corrupt("nonzero block header padding"));
        }
        self.block = Some(Block {
            header_size: header_size as u64,
            data_start: self.input.pos,
            declared_compressed,
            declared_uncompressed,
            digest: Digest { check: self.check, crc32: 0, crc64: 0 },
            out_len: 0,
            dict: Dict::new(dict),
            lzma: None,
            need_dict_reset: true,
            need_props: true,
        });
        self.phase = Phase::Chunks;
        Ok(())
    }

    fn uncompressed_chunk(&mut self, control: u8) -> io::Result<()> {
        let size = usize::from(self.input.be16()?) + 1;
        let mut data = vec![0u8; size];
        self.input.fill(&mut data)?;
        let blk = self.block.as_mut().ok_or_else(|| corrupt("chunk outside block"))?;
        if control == 0x01 {
            blk.dict.reset();
            blk.need_dict_reset = false;
        } else if blk.need_dict_reset {
            return Err(corrupt("missing dictionary reset"));
        }
        for &b in &data {
            blk.dict.put(b);
        }
        self.pending.extend_from_slice(&data);
        Ok(())
    }

    fn lzma_chunk(&mut self, control: u8) -> io::Result<()> {
        let reset = (control >> 5) & 0x03;
        let unpacked = (usize::from(control & 0x1F) << 16) + usize::from(self.input.be16()?) + 1;
        let packed = usize::from(self.input.be16()?) + 1;
        let props = if reset >= 2 { Some(self.input.byte()?) } else { None };
        let mut data = vec![0u8; packed];
        self.input.fill(&mut data)?;

        let blk = self.block.as_mut().ok_or_else(|| corrupt("chunk outside block"))?;
        if reset == 3 {
            blk.dict.reset();
            blk.need_dict_reset = false;
        } else if blk.need_dict_reset {
            return Err(corrupt("missing dictionary reset"));
        }
        if let Some(prop) = props {
            if prop >= 9 * 5 * 5 {
                return Err(corrupt("invalid LZMA properties"));
            }
            let (lc, lp, pb) = (u32::from(prop) % 9, (u32::from(prop) / 9) % 5, u32::from(prop) / 45);
            if lc + lp > 4 || pb > 4 {
                return Err(corrupt("unsupported LZMA properties"));
            }
            blk.lzma = Some(Lzma::new(lc, lp, pb));
            blk.need_props = false;
        } else if blk.need_props {
            return Err(corrupt("missing LZMA properties"));
        } else if reset == 1 {
            if let Some(lzma) = blk.lzma.as_mut() {
                *lzma = Lzma::new(lzma.lc, lzma.lp, lzma.pb);
            }
        }
        let lzma = blk.lzma.as_mut().ok_or_else(|| corrupt("missing LZMA properties"))?;
        let mut rc = Rc::new(&data)?;
        lzma.decode(&mut rc, &mut blk.dict, &mut self.pending, unpacked)?;
        rc.finish()
    }

    fn finish_block(&mut self) -> io::Result<()> {
        let blk = self.block.take().ok_or_else(|| corrupt("block end without block"))?;
        let compressed = self.input.pos - blk.data_start;
        let pad = ((4 - compressed % 4) % 4) as usize;
        let mut zeros = [0u8; 3];
        self.input.fill(&mut zeros[..pad])?;
        if zeros[..pad].iter().any(|&b| b != 0) {
            return Err(corrupt("nonzero block padding"));
        }
        let mut stored = [0u8; 8];
        let n = blk.digest.check.size();
        self.input.fill(&mut stored[..n])?;
        if !blk.digest.matches(&stored[..n]) {
            return Err(corrupt("block check mismatch"));
        }
        if blk.declared_compressed.is_some_and(|c| c != compressed) || blk.declared_uncompressed.is_some_and(|u| u != blk.out_len) {
            return Err(corrupt("block size mismatch"));
        }
        if self.records.len() >= MAX_BLOCKS {
            return Err(unsupported("too many blocks"));
        }
        self.records.push((blk.header_size + compressed + n as u64, blk.out_len));
        self.phase = Phase::BlockOrIndex;
        Ok(())
    }

    fn read_index_and_footer(&mut self, index_start: u64) -> io::Result<()> {
        let count = decode_varint(|| self.input.byte())?;
        if count != self.records.len() as u64 {
            return Err(corrupt("index record count mismatch"));
        }
        let records = std::mem::take(&mut self.records);
        for (unpadded, uncompressed) in records {
            let got_unpadded = decode_varint(|| self.input.byte())?;
            let got_uncompressed = decode_varint(|| self.input.byte())?;
            if (got_unpadded, got_uncompressed) != (unpadded, uncompressed) {
                return Err(corrupt("index record mismatch"));
            }
        }
        let pad = ((4 - (self.input.pos - index_start) % 4) % 4) as usize;
        let mut zeros = [0u8; 3];
        self.input.fill(&mut zeros[..pad])?;
        if zeros[..pad].iter().any(|&b| b != 0) {
            return Err(corrupt("nonzero index padding"));
        }
        let computed = self.input.crc;
        let mut stored = [0u8; 4];
        self.input.fill(&mut stored)?;
        if computed != u32::from_le_bytes(stored) {
            return Err(corrupt("index CRC mismatch"));
        }
        let index_size = self.input.pos - index_start;

        let mut footer = [0u8; 12];
        self.input.fill(&mut footer)?;
        if crc32_update(0, &footer[4..10]) != u32::from_le_bytes([footer[0], footer[1], footer[2], footer[3]]) {
            return Err(corrupt("footer CRC mismatch"));
        }
        let backward = (u64::from(u32::from_le_bytes([footer[4], footer[5], footer[6], footer[7]])) + 1) * 4;
        if backward != index_size {
            return Err(corrupt("backward size mismatch"));
        }
        if footer[8..10] != self.stream_flags || footer[10..12] != FOOTER_MAGIC {
            return Err(corrupt("bad stream footer"));
        }
        let mut extra = [0u8; 1];
        if self.input.inner.read(&mut extra)? != 0 {
            return Err(corrupt("trailing data after stream (multi-stream and padding are rejected)"));
        }
        self.phase = Phase::Done;
        Ok(())
    }
}

impl<R: Read> Read for XzReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.pending_pos < self.pending.len() {
                let n = buf.len().min(self.pending.len() - self.pending_pos);
                buf[..n].copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + n]);
                self.pending_pos += n;
                return Ok(n);
            }
            if self.phase == Phase::Done {
                return Ok(0);
            }
            self.pending.clear();
            self.pending_pos = 0;
            self.step()?;
            if let Some(blk) = self.block.as_mut() {
                blk.digest.update(&self.pending);
                blk.out_len += self.pending.len() as u64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    use super::XzReader;

    /// Encode with the host `xz` CLI via stdin/stdout. `None` when xz is unavailable.
    fn xz_encode(input: &[u8], args: &[&str]) -> Option<Vec<u8>> {
        let mut child = Command::new("xz")
            .args(["-c", "-q"])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdin = child.stdin.take()?;
        let data = input.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&data));
        let out = child.wait_with_output().ok()?;
        writer.join().ok()?.ok()?;
        out.status.success().then_some(out.stdout)
    }

    fn encode_or_skip(input: &[u8], args: &[&str]) -> Option<Vec<u8>> {
        let out = xz_encode(input, args);
        if out.is_none() {
            eprintln!("SKIP: xz CLI unavailable");
        }
        out
    }

    fn decode(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        XzReader::new(bytes).read_to_end(&mut out)?;
        Ok(out)
    }

    fn xorshift_bytes(len: usize, mut seed: u64) -> Vec<u8> {
        (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 24) as u8
            })
            .collect()
    }

    fn text(lines: usize) -> Vec<u8> {
        (0..lines).map(|i| format!("line {i} flamewm theme data\n")).collect::<String>().into_bytes()
    }

    fn mixed() -> Vec<u8> {
        let mut data = text(4000);
        data.extend(xorshift_bytes(300_000, 0x9E37_79B9_7F4A_7C15));
        data.extend(text(2000));
        data
    }

    /// Recompute the block header CRC after patching a header byte.
    fn patch_block_header(enc: &mut [u8], offset: usize, value: u8) {
        let hs = (usize::from(enc[12]) + 1) * 4;
        enc[offset] = value;
        let crc = super::crc32_update(0, &enc[12..12 + hs - 4]);
        enc[12 + hs - 4..12 + hs].copy_from_slice(&crc.to_le_bytes());
    }

    /// Offset of the LZMA2 dictionary property byte in the first block header.
    fn dict_prop_offset(enc: &[u8]) -> usize {
        let flags = enc[13];
        let mut at = 14;
        for present in [flags & 0x40 != 0, flags & 0x80 != 0] {
            if present {
                while enc[at] & 0x80 != 0 {
                    at += 1;
                }
                at += 1;
            }
        }
        assert_eq!((enc[at], enc[at + 1]), (0x21, 1), "unexpected block header layout");
        at + 2
    }

    #[test]
    fn roundtrip_matches_xz_cli_across_options() {
        let input = mixed();
        let variants: &[&[&str]] = &[
            &[],
            &["--check=crc32"],
            &["--check=none"],
            &["--check=crc64"],
            &["-0"],
            &["-9e"],
            &["--lzma2=lc=0,lp=2,pb=0"],
            &["--lzma2=lc=4,lp=0,pb=4"],
            &["--block-size=65536"],
        ];
        for args in variants {
            let Some(enc) = encode_or_skip(&input, args) else { return };
            let got = decode(&enc).unwrap_or_else(|e| panic!("{args:?}: {e}"));
            assert!(got == input, "{args:?}: output mismatch");
        }
    }

    #[test]
    fn empty_stream_decodes_to_nothing() {
        let Some(enc) = encode_or_skip(b"", &[]) else { return };
        assert!(decode(&enc).unwrap().is_empty());
    }

    #[test]
    fn every_truncated_prefix_is_an_error() {
        let Some(enc) = encode_or_skip(&text(60), &[]) else { return };
        for n in 0..enc.len() {
            assert!(decode(&enc[..n]).is_err(), "prefix {n} of {} decoded", enc.len());
        }
    }

    #[test]
    fn every_single_bit_flip_is_an_error_not_a_panic() {
        let Some(enc) = encode_or_skip(&text(20), &[]) else { return };
        for i in 0..enc.len() {
            for bit in 0..8 {
                let mut bad = enc.clone();
                bad[i] ^= 1 << bit;
                assert!(decode(&bad).is_err(), "flip byte {i} bit {bit} decoded");
            }
        }
    }

    #[test]
    fn garbage_and_bad_magic_are_errors() {
        assert!(decode(b"").is_err());
        assert!(decode(b"not an xz stream at all").is_err());
        for seed in 1..200u64 {
            let mut junk = xorshift_bytes(64 + (seed as usize % 300), seed);
            junk[..6].copy_from_slice(&[0xFD, b'7', b'z', b'X', b'Z', 0]);
            let _ = decode(&junk);
        }
    }

    #[test]
    fn oversized_dictionary_is_rejected_and_cap_boundary_is_accepted() {
        let input = text(40);
        let Some(enc) = encode_or_skip(&input, &[]) else { return };
        let at = dict_prop_offset(&enc);

        let mut far_over = enc.clone();
        patch_block_header(&mut far_over, at, 40);
        assert_eq!(decode(&far_over).unwrap_err().kind(), std::io::ErrorKind::Unsupported);

        let mut over_cap = enc.clone();
        patch_block_header(&mut over_cap, at, 29);
        assert_eq!(decode(&over_cap).unwrap_err().kind(), std::io::ErrorKind::Unsupported);

        let mut at_cap = enc.clone();
        patch_block_header(&mut at_cap, at, 28);
        assert_eq!(decode(&at_cap).unwrap(), input);
    }

    #[test]
    fn sha256_check_is_unsupported() {
        let Some(enc) = encode_or_skip(&text(10), &["--check=sha256"]) else { return };
        assert_eq!(decode(&enc).unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn multi_stream_and_stream_padding_are_rejected() {
        let Some(enc) = encode_or_skip(&text(10), &[]) else { return };
        let mut two = enc.clone();
        two.extend_from_slice(&enc);
        assert!(decode(&two).is_err());
        let mut padded = enc.clone();
        padded.extend_from_slice(&[0u8; 8]);
        assert!(decode(&padded).is_err());
    }

    #[test]
    fn bomb_output_is_bounded_by_caller_take() {
        const LIMIT: u64 = 1024 * 1024;
        let zeros = vec![0u8; 32 * 1024 * 1024];
        let Some(enc) = encode_or_skip(&zeros, &[]) else { return };
        assert!(enc.len() < 64 * 1024, "fixture should be a high-ratio bomb, got {}", enc.len());
        let mut out = Vec::new();
        XzReader::new(&enc[..]).take(LIMIT + 1).read_to_end(&mut out).unwrap();
        assert_eq!(out.len() as u64, LIMIT + 1);
    }
}
