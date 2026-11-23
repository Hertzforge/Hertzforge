"""Recordings and stills with an element shape set's elements colored by strain or stress
(`Recording.strain_stress`): modal coordinates played through their shape set, or the set's own
modes, each element colored by a scalar of its summed tensor on the clip's largest value or a
stated scale said in the display system chosen. One test draws a real picture when Chrome is on
the machine."""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pytest

import hertzforge as hz
from hertzforge.__main__ import main
from hertzforge.core.data import TimeHistory
from hertzforge.core.stress_scalars import von_mises
from hertzforge.record import RecordError, Recording, animation_view, frame_at, picture

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_element_shapes import _cells


@pytest.fixture(scope='module')
def cells():
    project = hz.Project('cells')
    project.add('Cells', _cells(along=4))
    project.solve_modes('Cells', num_modes=9)
    read = project.element_shapes('Cells Modes', modes=[6, 7, 8])
    time = np.arange(40) / 1000.0
    rows = np.vstack([1e-3 * np.sin(2 * np.pi * 37.0 * time),
                      5e-4 * np.sin(2 * np.pi * 74.0 * time)])
    project.add('Clip', TimeHistory(time, rows, ['M7', 'M9'], ordinate_dim='modal_length'))
    return project, read


def test_a_clip_colors_its_elements_by_the_von_mises_of_the_summed_tensor(cells):
    project, read = cells
    view = animation_view(project, Recording('Cells', data='Clip', shapes='Cells Modes',
                                             strain_stress=read))
    held = view['element_values']
    centroid = project[read].stress[:, project[read].point == 0][[0, 2]]
    np.testing.assert_allclose(held['shapes'], centroid, rtol=1e-6)
    every = von_mises(np.einsum('ms,mec->sec', project['Clip'].ordinate, centroid))
    assert held['max'] == pytest.approx([float(every.max())], rel=1e-6)
    assert held['unit'] == 'Pa' and 'stated' not in held


def test_a_stated_scale_is_said_in_the_units_chosen_and_drawn_at_that_top(cells):
    project, read = cells
    view = animation_view(project, Recording('Cells', data='Clip', shapes='Cells Modes',
                                             strain_stress=read, color_max=2.5,
                                             units='mm-kg-N-s'))
    held = view['element_values']
    assert held['unit'] == 'MPa' and held['max'] == [pytest.approx(2.5)] and held['stated']
    # the tensors are in the same unit as the scale, so the colors read against it
    pascals = project[read].stress[0, project[read].point == 0]
    np.testing.assert_allclose(held['shapes'][0], pascals / 1e6, rtol=1e-5)
    # the model is still drawn in meters: only the stress is said in the units chosen
    plain = animation_view(project, Recording('Cells', data='Clip', shapes='Cells Modes',
                                              strain_stress=read))
    np.testing.assert_allclose(view['nodes'], plain['nodes'])


def test_the_colored_groups_are_named_as_the_frame_names_them(cells):
    project, read = cells
    view = animation_view(project, Recording('Cells', data='Clip', shapes='Cells Modes',
                                             strain_stress=read, color_groups=['wedges']))
    assert view['element_values']['elements'] == 2


def test_with_no_data_the_sets_own_modes_swing_each_on_its_own_scale(cells):
    project, read = cells
    view = animation_view(project, Recording('Cells', strain_stress=read, mode=2))
    held = view['element_values']
    assert len(view['modes']) == 3 and len(held['max']) == 3
    peaks = [float(von_mises(project[read].stress[k, project[read].point == 0]).max())
             for k in range(3)]
    assert held['max'] == pytest.approx(peaks, rel=1e-6)


@pytest.mark.parametrize(('options', 'said'), [
    ({'color_max': 2.0}, 'need the strain and stress to color by'),
    ({'color_groups': ['wedges']}, 'need the strain and stress to color by'),
    ({'strain_stress': 'READ', 'color_groups': ['beams']},
     "Cells has no element group named 'beams'; it has bricks, wedges"),
    ({'strain_stress': 'READ', 'color_max': 0.0}, 'top of the color scale is above 0'),
    ({'strain_stress': 'READ', 'units': 'cubits'}, "the units are one of m-kg-N-s"),
    ({'strain_stress': 'READ', 'color': 'tresca', 'quantity': 'strain'},
     'tresca is a stress; strain is colored by one component'),
    ({'strain_stress': 'READ', 'shapes': 'Cells Modes'}, 'name no shape set without data'),
])
def test_a_coloring_that_cannot_be_drawn_is_refused(cells, options, said):
    project, read = cells
    if options.get('strain_stress') == 'READ':
        options = {**options, 'strain_stress': read}
    with pytest.raises(RecordError, match=said):
        animation_view(project, Recording('Cells', **options))


def test_a_still_is_drawn_at_a_time_of_the_data_or_a_point_of_the_cycle():
    info = {'motion': True, 'samples': 40, 'start': 0.5, 'step': 0.001, 'unit': 's'}
    assert frame_at(info, None) == 0.0
    assert frame_at(info, 0.513) == pytest.approx(13.0)
    with pytest.raises(RecordError, match='runs from 0.5 to 0.539 s; 0.6 is outside it'):
        frame_at(info, 0.6)
    assert frame_at({'motion': False}, None) == 0.25, 'a mode at its full deflection'
    assert frame_at({'motion': False}, 1.75) == 0.75


def test_the_still_command_draws_a_frame_and_no_dof_arrows_with_it(cells, tmp_path, capsys):
    project, read = cells
    path = tmp_path / 'cells.hforge'
    project.save(str(path))
    code = main(['still', str(path), '-o', str(tmp_path / 'a.png'), '--geometry', 'Cells',
                 '--strain-stress', read, '--dofs', 'Clip'])
    assert code == 1 and 'draws no DOF arrows' in capsys.readouterr().out
    code = main(['record', str(path), '-o', str(tmp_path / 'a.mp4'), '--geometry', 'Cells',
                 '--data', 'Clip', '--color-max', '3'])
    assert code == 1 and 'need the strain and stress to color by' in capsys.readouterr().out


def _pixels(path: Path) -> np.ndarray:
    """A PNG of 8-bit RGB or RGBA, as Chrome writes them, as rows of RGB."""
    import struct
    import zlib

    data = path.read_bytes()
    at, chunks, width = 8, b'', 0
    while at < len(data):
        size, kind = struct.unpack('>I4s', data[at:at + 8])
        body = data[at + 8:at + 8 + size]
        if kind == b'IHDR':
            width, height, depth, color = struct.unpack('>IIBB', body[:10])
            assert depth == 8 and color in (2, 6)
            channels = 3 if color == 2 else 4
        elif kind == b'IDAT':
            chunks += body
        at += 12 + size
    raw = zlib.decompress(chunks)
    stride = width * channels
    out = np.zeros((height, stride), dtype=np.int64)
    for y in range(height):
        kind, line = raw[y * (stride + 1)], np.frombuffer(raw, np.uint8, stride, y * (stride + 1) + 1)
        up = out[y - 1] if y else np.zeros(stride, dtype=np.int64)
        row = line.astype(np.int64)
        if kind == 2:
            row = (row + up) % 256
        elif kind in (1, 3, 4):
            for x in range(stride):
                left = row[x - channels] if x >= channels else 0
                corner = up[x - channels] if x >= channels else 0
                if kind == 1:
                    guess = left
                elif kind == 3:
                    guess = (left + up[x]) // 2
                else:
                    p = left + up[x] - corner
                    pa, pb, pc = abs(p - left), abs(p - up[x]), abs(p - corner)
                    guess = left if pa <= pb and pa <= pc else up[x] if pb <= pc else corner
                row[x] = (row[x] + guess) % 256
        out[y] = row
    return out.reshape(height, width, channels)[..., :3]


def _chrome():
    from hertzforge.record import find_chrome

    try:
        return find_chrome()
    except RecordError:
        return None


@pytest.mark.slow
@pytest.mark.skipif(_chrome() is None, reason='needs Chrome')
def test_a_still_of_the_clip_is_drawn_with_its_legend(cells, tmp_path):
    project, read = cells
    out = picture(project, Recording('Cells', data='Clip', shapes='Cells Modes',
                                     strain_stress=read, color_max=2.5, units='mm-kg-N-s',
                                     width=640, height=360, theme='light'),
                  tmp_path / 'frame.png', at=0.013)
    image = _pixels(out)
    assert image.shape == (360, 640, 3)
    # the legend's bar runs the colormap at the bottom left: its yellow end is drawn
    corner = image[250:, :400].reshape(-1, 3).astype(int)
    assert np.any((corner[:, 0] > 220) & (corner[:, 1] > 200) & (corner[:, 2] < 80))
