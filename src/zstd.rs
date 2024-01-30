use std::io::Cursor;

pub use zstd::bulk::{Compressor, Decompressor};
pub use zstd::compression_level_range;
pub use zstd::zstd_safe::compress_bound;

pub fn spare_capacity_buf(buf: &mut Vec<u8>) -> Cursor<&mut Vec<u8>> {
    let pos = buf.len() as u64;
    let mut cursor = Cursor::new(buf);
    cursor.set_position(pos);
    cursor
}
