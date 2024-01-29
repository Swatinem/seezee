use std::mem;
use std::ops::Range;

use watto::Pod;

const DEFAULT_FRAME_SIZE: usize = 8 * (1 << 10);

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
        let mut compressor = zstd::bulk::Compressor::new(self.level)?;
        compressor.include_checksum(false)?;
        compressor.include_contentsize(false)?;
        compressor.include_dictid(false)?;
        compressor.include_magicbytes(false)?;

        let table_sizeof = (num_frames + 3) * mem::size_of::<u32>();

        let reserve = table_sizeof + zstd::zstd_safe::compress_bound(self.frame_size * 2);
        let mut buf: Vec<u8> = Vec::with_capacity(reserve);
        buf.resize(table_sizeof, 0);
        set_u32(&mut buf, 0, self.frame_size as u32);
        set_u32(&mut buf, 1, input.len() as u32);

        let mut total_written = 0;

        for i in 0..num_frames {
            let from = i * self.frame_size;
            let to = ((i + 1) * self.frame_size).min(input.len());
            let source = &input[from..to];

            buf.reserve(zstd::zstd_safe::compress_bound(source.len()));
            let destination: &mut [u8] = unsafe { mem::transmute(buf.spare_capacity_mut()) };

            let bytes_written = compressor.compress_to_buffer(source, destination)?;
            unsafe { buf.set_len(buf.len() + bytes_written) };

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
        })
    }

    fn frame_size(&self) -> usize {
        self.header.frame_size as usize
    }

    pub fn read_into(&self, buf: &mut Vec<u8>, range: Range<usize>) -> std::io::Result<()> {
        let frame_size = self.frame_size();
        let start = range.start / frame_size;
        let end = (range.end / frame_size) + 1;
        let frame_offsets = self.frame_offsets.get(start..=end).ok_or_else(eof)?;

        let mut decompressor = zstd::bulk::Decompressor::new()?;
        decompressor.include_magicbytes(false)?;

        buf.clear();

        for (i, win) in frame_offsets.windows(2).enumerate() {
            let [start, end] = win else { return Err(eof()) };
            let source = self
                .zstd_buf
                .get((*start as usize)..(*end as usize))
                .ok_or_else(eof)?;

            buf.reserve(frame_size);
            let destination: &mut [u8] = unsafe { mem::transmute(buf.spare_capacity_mut()) };
            let bytes_written = decompressor.decompress_to_buffer(source, destination)?;
            unsafe { buf.set_len(buf.len() + bytes_written) };

            if i == 0 {
                buf.drain(..range.start % frame_size);
            }
        }

        buf.truncate(range.len());

        Ok(())
    }
}

fn eof() -> std::io::Error {
    std::io::ErrorKind::UnexpectedEof.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_compress() {
        let input: Vec<u8> = (0..255).collect();
        let compressed = Compressor::new().frame_size(128).compress(&input).unwrap();

        let mut output = Vec::new();
        let decompressor = Decompressor::new(&compressed).unwrap();
        decompressor.read_into(&mut output, 0..255).unwrap();

        assert_eq!(&input[0..255], output);
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
            let decompressor = Decompressor::new(&compressed).unwrap();

            for (a,b) in ranges {
                let (a, b) = (a.index(input.len()), b.index(input.len()));
                let range = if a < b { a..b } else { b..a };

                let output = decompressor.read_into(&mut output, range.clone()).ok().map(|_| &output[..]);

                prop_assert_eq!(input.get(range), output);
            }
        }
    }
}
