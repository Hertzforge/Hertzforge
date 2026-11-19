"""Shared helpers for the test-data generators."""

import numpy as np


def recompress(*paths):
    """Rewrite .npz files with compression.

    Test data lives in the repository, and these arrays are smooth enough
    to shrink by nearly half. sdynpy writes them uncompressed; numpy reads
    either kind without being told which.
    """
    for path in paths:
        with np.load(path, allow_pickle=True) as handle:
            arrays = dict(handle)
        np.savez_compressed(path, **arrays)


def gray_png(width=320, height=240, rgb=(0x6b, 0x6e, 0x74)):
    """A plain PNG of one color, written with the standard library: the
    stand-in photograph the demonstration projects carry."""
    import struct
    import zlib

    def chunk(kind, data):
        return (struct.pack('>I', len(data)) + kind + data
                + struct.pack('>I', zlib.crc32(kind + data) & 0xffffffff))

    row = b'\x00' + bytes(rgb) * width
    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(row * height))
            + chunk(b'IEND', b''))
