"""Photos out as the pictures they are.

The bytes are stored exactly as they arrived — that is why a `Photos`
object keeps them encoded rather than decoded — so writing them out is a
copy, and a JPEG that makes the trip loses no generation.

What a folder cannot hold is the *order*, which is the one thing this
export loses and the report cares about. Said here so it is a known
cost rather than a discovery.
"""

from __future__ import annotations

import pathlib

import pytest

from hertzforge.core.photos import Photos
from hertzforge.io import photo_files


def test_a_name_that_is_not_a_filename(tmp_path):
    """'Fixture 3/4 view' is a good caption and a bad path."""
    album = Photos(names=['Fixture 3/4 view'], formats=['png'],
                   images=[b'\x89PNG\r\n\x1a\n'])
    written = photo_files.save(album, str(tmp_path / 'out'))
    name = pathlib.Path(written[0]).name
    assert not set(name) & set('/\\:*?"<>|'), name
    assert name.endswith('.png')


def test_two_photos_of_the_same_name_do_not_become_one(tmp_path):
    """Nothing stops an object holding two photos called the same thing;
    a folder cannot, so the second is numbered rather than lost."""
    album = Photos(names=['Setup', 'Setup'], formats=['png', 'png'],
                   images=[b'first', b'second'])
    written = photo_files.save(album, str(tmp_path / 'out'))
    assert len(written) == 2
    assert len({pathlib.Path(p).name for p in written}) == 2
    assert {pathlib.Path(p).read_bytes() for p in written} == {b'first',
                                                               b'second'}


def test_jpeg_keeps_its_own_extension(tmp_path):
    album = Photos(names=['Article'], formats=['jpeg'], images=[b'\xff\xd8\xff'])
    written = photo_files.save(album, str(tmp_path / 'out'))
    assert pathlib.Path(written[0]).name == 'Article.jpg'


@pytest.mark.parametrize('fmt, suffix', [('gif', '.gif'), ('bmp', '.bmp'),
                                         ('jpg', '.jpg')])
def test_an_escdf_photos_format_keeps_its_own_extension(fmt, suffix, tmp_path):
    """An ESCDF import names the format by the attachment's suffix, and
    a GIF, a BMP and a 'jpg' were each written as '.png' around bytes
    that are not PNG (2026-10-13)."""
    album = Photos(names=['Article'], formats=[fmt], images=[b'bytes'])
    written = photo_files.save(album, str(tmp_path / 'out'))
    assert pathlib.Path(written[0]).name == f'Article{suffix}'
