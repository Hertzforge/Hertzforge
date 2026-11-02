"""An exodus modal file whose rigid-body modes came out slightly negative.

An eigensolver reports a rigid-body eigenvalue as a number near zero of
either sign, and the file writes the frequency as it came out: -3.2e-7
Hz. A ShapeSet refuses a negative frequency, so such a file did not
import at all (Brandon, 2026-09-25). It imports now: the mode is a
rigid-body mode and reads as 0 Hz, the file's own value is kept on the
mode's description, and the import says so — as a warning a script's
log shows, and in a dialog at the window, which had no channel for a
reader's caveats before.
"""

from __future__ import annotations

import warnings

import netCDF4
import numpy as np
import pytest

import hertzforge
from hertzforge.core.geometry import Geometry
from hertzforge.core.shapes import ShapeSet


def _modal_file(tmp_path, frequencies):
    geometry = Geometry(node_id=[1, 2, 3],
                        node_xyz=[[0, 0, 0], [1, 0, 0], [2, 0, 0]],
                        length_unit='m')
    shapes = ShapeSet([0.0, 0.0, 12.5], [0.0, 0.0, 0.01],
                      ['1Z+', '2Z+', '3Z+'],
                      [[1, 1, 1], [1, 0, -1], [1, -2, 1]])
    path = tmp_path / 'modes.exo'
    hertzforge.export_file(shapes, str(path), format='exodus',
                               geometry=geometry)
    # the writer nudges coincident frequencies apart; the file under test
    # carries exactly the solver's numbers
    with netCDF4.Dataset(path, 'r+') as ds:
        ds.variables['time_whole'][:] = np.asarray(frequencies)
    return str(path)


def test_negative_frequencies_import_as_rigid_body_modes_and_say_so(tmp_path):
    path = _modal_file(tmp_path, [-3.2e-7, -1.1e-8, 12.5])
    from hertzforge.io import ImportNote

    with pytest.warns(ImportNote, match=r'2 modes with a negative frequency '
                      r'\(-3\.2e-07, -1\.1e-08 Hz\) read as 0 Hz'):
        back = hertzforge.import_file(path)
    shapes = back['shapes']
    assert list(shapes.frequency) == [0.0, 0.0, 12.5], (
        'the negative ones are zero, the real one untouched')
    assert shapes.description[0] == (
        'frequency -3.2e-07 Hz in the file, read as 0 (a rigid-body mode)')
    assert shapes.description[1].startswith('frequency -1.1e-08 Hz')
    assert shapes.description[2] == '', 'nothing said about a good mode'


def test_a_file_with_no_negative_frequency_is_read_in_silence(tmp_path):
    path = _modal_file(tmp_path, [0.0, 1e-9, 12.5])
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter('always')
        back = hertzforge.import_file(path)
    assert not [w for w in caught if 'negative frequency' in str(w.message)]
    assert back['shapes'].description == ['', '', '']


def _answer_dialogs(window_module, monkeypatch, shown):
    """The window asks how an exodus file's steps read; a person picks
    mode shapes. The report dialogs are recorded rather than shown."""
    monkeypatch.setattr(
        window_module.QInputDialog, 'getItem',
        lambda parent, title, label, items, *rest: (items[0], True))
    for kind in ('information', 'warning'):
        monkeypatch.setattr(
            window_module.QMessageBox, kind,
            lambda parent, title, text: shown.append((title, text)))
