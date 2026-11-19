"""Nastran punch eigenvectors: modes in, ShapeSet out.

The fixtures are authored by hand from the public punch layout, with
the trailing sequence numbers in columns 73-80 that a real Nastran
writes — the reader must drop them unread or every third grid gains
a phantom component.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.io import punch


def _pad(line):
    return line.ljust(72)


def _punch_text():
    """Two real modes over grids 1 and 5, rotations on -CONT- lines.

    Eigenvalues are exact squares of round circular frequencies:
    lambda = (2 pi 10)^2 and (2 pi 25)^2, so the frequencies read
    back as 10 Hz and 25 Hz to the last digit.
    """
    lam1 = (2.0 * np.pi * 10.0) ** 2
    lam2 = (2.0 * np.pi * 25.0) ** 2
    lines = [
        '$TITLE   = HAND-AUTHORED MODAL PUNCH',
        '$SUBTITLE=',
        '$LABEL   =',
        '$EIGENVECTOR',
        '$REAL OUTPUT',
        '$SUBCASE ID =           1',
        f'$EIGENVALUE = {lam1:15.7E}  MODE =    1',
        ('         1       G      1.000000E-01      0.000000E+00'
         '      2.000000E-01'),
        ('-CONT-                  3.000000E-03      0.000000E+00'
         '      0.000000E+00'),
        ('         5       G      0.000000E+00      4.000000E-01'
         '      0.000000E+00'),
        ('-CONT-                  0.000000E+00      0.000000E+00'
         '      0.000000E+00'),
        f'$EIGENVALUE = {lam2:15.7E}  MODE =    2',
        ('         1       G     -1.000000E-01      0.000000E+00'
         '      2.000000E-01'),
        ('-CONT-                  0.000000E+00      0.000000E+00'
         '      0.000000E+00'),
        ('         5       G      0.000000E+00     -4.000000E-01'
         '      5.000000E-01'),
        ('-CONT-                  0.000000E+00      0.000000E+00'
         '      0.000000E+00'),
    ]
    return '\n'.join(f'{_pad(line)}{n + 1:8d}'
                     for n, line in enumerate(lines)) + '\n'


def test_a_punch_file_reads_as_a_shape_set(tmp_path):
    path = tmp_path / 'modes.pch'
    path.write_text(_punch_text())
    shapes = hertzforge.import_file(str(path))
    assert np.allclose(shapes.frequency, [10.0, 25.0]), \
        'sqrt(lambda)/2pi, from eigenvalues authored as exact squares'
    assert list(shapes.coordinate) == [
        '1X+', '1Y+', '1Z+', '1RX+', '1RY+', '1RZ+',
        '5X+', '5Y+', '5Z+', '5RX+', '5RY+', '5RZ+']
    assert shapes.shape_matrix[0, 3] == pytest.approx(3.0e-3), \
        'the rotation rode in on the -CONT- line'
    assert shapes.shape_matrix[1, 8] == pytest.approx(0.5)
    assert np.all(np.asarray(shapes.damping) == 0.0), \
        'real normal modes carry no damping'
    assert shapes.unscaled, 'the punch never says how it was normalized'


def test_scalar_points_are_skipped_knowingly(tmp_path):
    path = tmp_path / 'modes.pch'
    text = _punch_text().replace(
        _pad('$SUBCASE ID =           1'),
        _pad('$SUBCASE ID =           1') + '\n'
        + _pad('       901       S      7.000000E-01'), 1)
    path.write_text(text)
    shapes = punch.load(str(path))
    assert all(not c.startswith('901') for c in shapes.coordinate)
    assert shapes.shape_matrix.shape == (2, 12)


def test_complex_output_refuses_rather_than_guessing(tmp_path):
    path = tmp_path / 'modes.pch'
    path.write_text(_punch_text().replace('$REAL OUTPUT',
                                          '$COMPLEX OUTPUT'))
    with pytest.raises(ValueError, match='complex'):
        punch.load(str(path))


def test_modes_over_differing_grids_refuse(tmp_path):
    path = tmp_path / 'modes.pch'
    lines = _punch_text().splitlines(keepends=True)
    # drop grid 5 (and its continuation) from mode 2 only
    path.write_text(''.join(lines[:-2]))
    with pytest.raises(ValueError, match='differing grids'):
        punch.load(str(path))


def test_a_punch_without_eigenvectors_is_not_sniffed(tmp_path):
    """A forces or stresses punch is a real .pch too; only the modal
    one belongs to this reader."""
    other = tmp_path / 'forces.pch'
    other.write_text(_pad('$ELEMENT FORCES') + '\n'
                     + _pad('$REAL OUTPUT') + '\n')
    assert not punch.sniff(str(other))
    modal = tmp_path / 'modes.pch'
    modal.write_text(_punch_text())
    assert punch.sniff(str(modal))
