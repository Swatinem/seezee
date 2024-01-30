use std::mem;
use std::ops::{Range, RangeBounds};

use watto::Pod;

mod zstd;

const DEFAULT_FRAME_SIZE: usize = 32 * (1 << 10);

pub struct Compressor {
    level: i32,
    frame_size: usize,
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            level: 0,
            frame_size: DEFAULT_FRAME_SIZE,
        }
    }

    pub fn level(mut self, level: i32) -> Self {
        assert!(zstd::compression_level_range().contains(&level));
        self.level = level;
        self
    }

    pub fn frame_size(mut self, frame_size: usize) -> Self {
        assert!(frame_size >= 1);
        assert!(frame_size < u32::MAX as usize);
        self.frame_size = frame_size;
        self
    }

    pub fn compress(self, input: &[u8]) -> std::io::Result<Vec<u8>> {
        assert!(input.len() < u32::MAX as usize);

        let num_frames = input.len().div_ceil(self.frame_size);
        let mut compressor = zstd::Compressor::new(self.level)?;
        compressor.include_checksum(false)?;
        compressor.include_contentsize(false)?;
        compressor.include_dictid(false)?;
        compressor.include_magicbytes(false)?;

        let table_sizeof = (num_frames + 3) * mem::size_of::<u32>();

        let reserve = table_sizeof + zstd::compress_bound(self.frame_size * 2);
        let mut buf: Vec<u8> = Vec::with_capacity(reserve);
        buf.resize(table_sizeof, 0);
        set_u32(&mut buf, 0, self.frame_size as u32);
        set_u32(&mut buf, 1, input.len() as u32);

        let mut total_written = 0;

        for i in 0..num_frames {
            let from = i * self.frame_size;
            let to = ((i + 1) * self.frame_size).min(input.len());
            let source = &input[from..to];

            buf.reserve(zstd::compress_bound(source.len()));
            let mut destination = zstd::spare_capacity_buf(&mut buf);

            let bytes_written = compressor.compress_to_buffer(source, &mut destination)?;

            total_written += bytes_written;
            set_u32(&mut buf, i + 3, total_written as u32);
        }

        Ok(buf)
    }
}

fn set_u32(buf: &mut [u8], i: usize, val: u32) {
    let from = i * mem::size_of::<u32>();
    let to = from + mem::size_of::<u32>();
    buf[from..to].copy_from_slice(&val.to_ne_bytes())
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct Decompressor<'b> {
    header: &'b Header,
    frame_offsets: &'b [u32],
    zstd_buf: &'b [u8],
    read_buf: Vec<u8>,
}

#[repr(C)]
#[derive(Debug)]
struct Header {
    frame_size: u32,
    input_len: u32,
}

unsafe impl watto::Pod for Header {}

impl<'b> Decompressor<'b> {
    pub fn new(bytes: &'b [u8]) -> Option<Self> {
        let (header, bytes) = Header::ref_from_prefix(bytes)?;
        let num_frames = header.input_len.div_ceil(header.frame_size) + 1;
        let (frame_offsets, zstd_buf) = u32::slice_from_prefix(bytes, num_frames as usize)?;

        Some(Self {
            header,
            frame_offsets,
            zstd_buf,
            read_buf: Vec::new(),
        })
    }

    fn frame_size(&self) -> usize {
        self.header.frame_size as usize
    }

    pub fn get<R>(&mut self, range: R) -> std::io::Result<Vec<u8>>
    where
        R: RangeBounds<usize>,
    {
        let mut buf = Vec::new();
        self.get_into(&mut buf, range)?;
        Ok(buf)
    }

    pub fn get_into<'o, R>(&mut self, buf: &'o mut Vec<u8>, range: R) -> std::io::Result<&'o [u8]>
    where
        R: RangeBounds<usize>,
    {
        let range = make_range(range, self.header.input_len as usize);
        self.read_into(buf, range)
    }

    fn read_into<'o>(
        &mut self,
        buf: &'o mut Vec<u8>,
        range: Range<usize>,
    ) -> std::io::Result<&'o [u8]> {
        if range.start > range.end {
            return Err(eof());
        }
        let frame_size = self.frame_size();
        let start = range.start / frame_size;
        let end = range.end.div_ceil(frame_size);
        let frame_offsets = self.frame_offsets.get(start..=end).ok_or_else(eof)?;

        let mut decompressor = zstd::Decompressor::new()?;
        decompressor.include_magicbytes(false)?;

        buf.clear();
        buf.reserve(range.len());

        // FIXME: a stable `array_windows` would be nice
        for (i, win) in frame_offsets.windows(2).enumerate() {
            let &[start, end] = win else {
                return Err(eof());
            };
            let source = &self
                .zstd_buf
                .get((start as usize)..(end as usize))
                .ok_or_else(eof)?;

            let is_end = i == frame_offsets.len() - 2;
            if i == 0 || is_end {
                self.read_buf.clear();
                self.read_buf.reserve(frame_size);
                let mut destination = zstd::spare_capacity_buf(&mut self.read_buf);
                decompressor.decompress_to_buffer(source, &mut destination)?;

                let start = if i == 0 { range.start % frame_size } else { 0 };
                let end = (start + (range.len() - buf.len())).min(self.read_buf.len());
                buf.extend_from_slice(&self.read_buf[start..end]);
            } else {
                let mut destination = zstd::spare_capacity_buf(buf);
                let _bytes_written = decompressor.decompress_to_buffer(source, &mut destination)?;
            }
        }

        Ok(buf.as_slice())
    }
}

fn eof() -> std::io::Error {
    std::io::ErrorKind::UnexpectedEof.into()
}

fn make_range<R>(range: R, len: usize) -> Range<usize>
where
    R: RangeBounds<usize>,
{
    use std::ops::Bound::*;

    let start = match range.start_bound() {
        Included(b) => *b,
        Excluded(b) => *b,
        Unbounded => 0,
    };
    let end = match range.end_bound() {
        Included(b) => *b + 1,
        Excluded(b) => *b,
        Unbounded => len,
    };

    start..end
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_compress() {
        let input: Vec<u8> = (0..32).collect();
        let compressed = Compressor::new().frame_size(16).compress(&input).unwrap();

        let mut o = Vec::new();
        let mut d = Decompressor::new(&compressed).unwrap();

        #[allow(clippy::reversed_empty_ranges)]
        {
            assert_eq!(d.get_into(&mut o, 3..1).ok(), input.get(3..1));
        }

        assert_eq!(d.get_into(&mut o, ..0).ok(), input.get(..0));
        assert_eq!(d.get_into(&mut o, ..).ok(), input.get(..));
        assert_eq!(d.get_into(&mut o, 0..32).ok(), input.get(0..32));

        assert_eq!(d.get_into(&mut o, ..31).ok(), input.get(..31));
        assert_eq!(d.get_into(&mut o, 1..).ok(), input.get(1..));
        assert_eq!(d.get_into(&mut o, 1..31).ok(), input.get(1..31));

        assert_eq!(d.get_into(&mut o, 5..10).ok(), input.get(5..10));
        assert_eq!(d.get_into(&mut o, 10..20).ok(), input.get(10..20));
    }

    proptest! {
        #[test]
        fn test_slice(
            input in prop::collection::vec(any::<u8>(), 8..1024),
            frame_size in 8..256usize,
            ranges in prop::collection::vec((any::<prop::sample::Index>(), any::<prop::sample::Index>()), 100)
        ) {
            let compressed = Compressor::new().frame_size(frame_size).compress(&input).unwrap();

            let mut output = Vec::new();
            let mut decompressor = Decompressor::new(&compressed).unwrap();

            for (a,b) in ranges {
                let (a, b) = (a.index(input.len()), b.index(input.len()));
                let range = if a < b { a..b } else { b..a };

                let output = decompressor.get_into(&mut output, range.clone()).ok();

                prop_assert_eq!(input.get(range), output);
            }
        }
    }
}
