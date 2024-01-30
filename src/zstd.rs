pub use zstd::bulk::{Compressor, Decompressor};
pub use zstd::compression_level_range;
pub use zstd::zstd_safe::compress_bound;

pub struct SpareCapacityWriteBuf<'b> {
    buf: &'b mut Vec<u8>,
    start: usize,
}

impl<'b> SpareCapacityWriteBuf<'b> {
    pub fn new(buf: &'b mut Vec<u8>) -> Self {
        let start = buf.len();
        Self { buf, start }
    }
}

unsafe impl<'b> zstd::zstd_safe::WriteBuf for SpareCapacityWriteBuf<'b> {
    fn as_slice(&self) -> &[u8] {
        &self.buf[self.start..]
    }

    fn capacity(&self) -> usize {
        self.buf.capacity() - self.start
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        unsafe { self.buf.as_mut_ptr().byte_add(self.start) }
    }

    unsafe fn filled_until(&mut self, n: usize) {
        self.buf.set_len(n + self.start)
    }
}
