"""Saving an animation as a video: MP4 holding H.264, or nothing.

The file is read back at the byte level — the sample table says how
many frames went in, the sample entry which codec they went in as —
because the failure this guards is a file that *exists* and is wrong:
on a machine with no H.264 encoder, Qt records MPEG-4 Part 2 into the
same .mp4 without a word (PLAN.md, "Saving an animation").
"""

import numpy as np
import pytest


def _box(kind, body):
    import struct

    return struct.pack('>I4s', 8 + len(body), kind) + body


def _mp4(codec, count, frames=b''):
    """The boxes an MP4's reading comes from: the frames' data, then
    the index Qt 6.12 writes after them."""
    sizes = _box(b'stsz', bytes(8) + count.to_bytes(4, 'big'))
    described = _box(b'stsd', bytes(4) + (1).to_bytes(4, 'big')
                     + _box(codec, bytes(8)))
    index = _box(b'moov', _box(b'trak', _box(b'mdia', _box(
        b'minf', _box(b'stbl', described + sizes)))))
    return _box(b'mdat', frames) + index


def frames(count, height=120, width=160, channels=3):
    yy, xx = np.mgrid[0:height, 0:width]
    for i in range(count):
        image = np.zeros((height, width, channels), np.uint8)
        image[..., 1] = (127 + 127 * np.sin(xx / 9 + i / 5)).astype(np.uint8)
        image[..., 0] = (yy * 2 % 256).astype(np.uint8)
        yield image


@pytest.fixture(scope='module')
def plate_run():
    from conftest import fixture_path

    import hertzforge

    run = hertzforge.random_vibration_run(fixture_path('plate',
                                                           'random.nc4'))
    geometry = hertzforge.import_file(fixture_path('plate',
                                                       'test_geometry.npz'))
    geometry.define_units('m')
    return geometry, run.time_history
