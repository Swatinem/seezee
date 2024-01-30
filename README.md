# Seezee

A seekable `zstd` compressed buffer.

This is very similar to the [seekable format] extension, with some small differences:

- The resulting buffer is _not_ a valid `zstd` file, and cannot be handled directly
  by other `zstd` decompression tools.
- All frames have the same (uncompressed) size, so there is no need to store than and binary search.
- Frames are stored without the `zstd` magic, saving a few bytes.

I might add support for an embedded dictionary in the future.

[seekable format]: https://github.com/facebook/zstd/tree/dev/contrib/seekable_format
