"""A geometry's animation exported as one page that plays offline (`hertzforge
export-animation`): the view the app shows for the selection, what the page opens at, and the
command line."""

from __future__ import annotations

from pathlib import Path

import pytest

import hertzforge as hz
from hertzforge.__main__ import main
from hertzforge.animation import AnimationError, animation_page, animation_view, export_animation
from hertzforge.report.viewer import read_viewer

PLATE = Path(__file__).resolve().parent.parent / 'testdata' / 'plate'


@pytest.fixture(scope='module')
def plate():
    project = hz.Project('plate')
    project.import_file(str(PLATE / 'geometry.unv'))
    project.import_file(str(PLATE / 'shapes.npy'))
    return project


def test_the_view_is_the_one_the_app_shows_for_the_selection(plate):
    view = animation_view(plate, 'Geometry', shapes='Shape Set')
    assert view['view'] == 'shape3d' and len(view['modes']) == plate['Shape Set'].num_shapes
    with pytest.raises(AnimationError, match="no object named 'Nothing'"):
        animation_view(plate, 'Geometry', shapes='Nothing')
    with pytest.raises(AnimationError, match='show no animation'):
        animation_view(plate, 'Geometry')


def test_the_page_holds_the_animation_and_opens_where_it_was_asked(plate):
    """Brandon, 2026-11-14: the fridge's kick, live in an iframe in his talk's slides."""
    page = animation_page(plate, 'Geometry', shapes='Shape Set', mode=7, scale=2.5,
                          theme='light', title='Plate <modes>')
    assert '<html lang="en" data-theme="light">' in page
    assert '<title>Plate &lt;modes&gt;</title>' in page
    held = read_viewer(page)['animation']
    assert held['view']['view'] == 'shape3d'
    assert held['setup'] == {'scale': 2.5, 'mode': 6}
    assert 'data-theme' not in animation_page(plate, 'Geometry', shapes='Shape Set')[:400]


def test_what_the_page_cannot_open_at_is_refused(plate):
    with pytest.raises(AnimationError, match='modes 1 to 22; not 40'):
        animation_page(plate, 'Geometry', shapes='Shape Set', mode=40)
    with pytest.raises(AnimationError, match='light or dark'):
        animation_page(plate, 'Geometry', shapes='Shape Set', theme='blue')
    with pytest.raises(AnimationError, match='0 or more'):
        animation_page(plate, 'Geometry', shapes='Shape Set', scale=-1)


def test_the_page_is_written_where_it_was_asked(plate, tmp_path):
    out = export_animation(plate, tmp_path / 'plate.html', 'Geometry', shapes='Shape Set')
    assert read_viewer(out.read_text(encoding='utf-8'))['animation']['setup'] == {
        'scale': 1.0, 'mode': 0}


def test_the_command_writes_the_page_or_says_why_not(plate, tmp_path, capsys):
    path = tmp_path / 'plate.hforge'
    plate.save(path)
    out = tmp_path / 'plate.html'
    assert main(['export-animation', str(path), '-o', str(out), '--geometry', 'Geometry',
                 '--shapes', 'Shape Set', '--mode', '3', '--theme', 'dark']) == 0
    assert f'wrote {out}' in capsys.readouterr().out
    assert read_viewer(out.read_text(encoding='utf-8'))['animation']['setup']['mode'] == 2
    assert main(['export-animation', str(path), '-o', str(out), '--geometry', 'Geometry']) == 1
    assert 'show no animation' in capsys.readouterr().out
    assert main(['export-animation', str(tmp_path / 'none.hforge'), '-o', str(out),
                 '--geometry', 'Geometry']) == 1
    assert 'hertzforge export-animation:' in capsys.readouterr().out


def test_a_geometry_with_its_own_view_opens_in_it(plate):
    """Brandon, 2026-11-14: the fridge, built with Y up, opened lying on its side."""
    from hertzforge.core.geometry import View

    assert 'home' not in animation_view(plate, 'Geometry', shapes='Shape Set')
    plate['Geometry'].view = View(eye=(1, 0.55, 1.2), up=(0, 1, 0))
    try:
        view = animation_view(plate, 'Geometry', shapes='Shape Set')
    finally:
        plate['Geometry'].view = None
    assert view['home']['up'] == pytest.approx([0, 1, 0])
    assert view['home']['eye'] == pytest.approx([1, 0.55, 1.2])


def test_the_page_opens_see_through_and_from_the_camera_asked(plate):
    """Brandon, 2026-11-14: the fridge's surface translucent, set with no window open."""
    held = read_viewer(animation_page(plate, 'Geometry', shapes='Shape Set', opacity=0.3,
                                      eye=(1, 0.55, 1.2), up=(0, 1, 0)))['animation']['view']
    assert held['opacity'] == pytest.approx(0.3)
    assert held['home'] == {'eye': [1.0, 0.55, 1.2], 'up': [0.0, 1.0, 0.0]}
    top = read_viewer(animation_page(plate, 'Geometry', shapes='Shape Set', view='top'))
    assert top['animation']['view']['home'] == {'eye': [0.0, 0.0, 1.0], 'up': [0.0, 1.0, 0.0]}
    solid = read_viewer(animation_page(plate, 'Geometry', shapes='Shape Set'))['animation']['view']
    assert 'opacity' not in solid and 'home' not in solid


@pytest.mark.parametrize(('options', 'said'), [
    ({'opacity': 0}, 'above 0 and at most 1'),
    ({'opacity': 1.5}, 'above 0 and at most 1'),
    ({'view': 'sideways'}, 'one of iso'),
    ({'up': (0, 1, 0)}, 'up is given with an eye'),
    ({'eye': (0, 0, 0)}, 'direction to look from'),
    ({'eye': (0, 0, 1), 'up': (0, 0, 2)}, 'up'),
    ({'eye': (1, 0)}, 'three numbers'),
])
def test_an_opening_the_page_cannot_have_is_refused(plate, options, said):
    with pytest.raises(AnimationError, match=said):
        animation_page(plate, 'Geometry', shapes='Shape Set', **options)


def test_the_command_takes_the_opacity_and_the_camera(plate, tmp_path, capsys):
    path = tmp_path / 'plate.hforge'
    plate.save(path)
    out = tmp_path / 'plate.html'
    assert main(['export-animation', str(path), '-o', str(out), '--geometry', 'Geometry',
                 '--shapes', 'Shape Set', '--opacity', '0.4', '--eye', '1,1,0',
                 '--up', '0,0,1']) == 0
    view = read_viewer(out.read_text(encoding='utf-8'))['animation']['view']
    assert view['opacity'] == pytest.approx(0.4) and view['home']['eye'] == [1.0, 1.0, 0.0]
    capsys.readouterr()
    assert main(['export-animation', str(path), '-o', str(out), '--geometry', 'Geometry',
                 '--shapes', 'Shape Set', '--eye', 'up']) == 1
    assert 'three numbers' in capsys.readouterr().out


def test_the_page_and_the_command_open_framed_on_element_groups(plate, tmp_path, capsys):
    """Framed on a group, the page carries the box around its nodes; the scale is the one
    asked for, a share of the whole model as before."""
    geometry = plate['Geometry']
    named = geometry.group_name[0]
    geometry.group_name[0] = 'plate'
    try:
        held = read_viewer(animation_page(plate, 'Geometry', shapes='Shape Set', scale=2.0,
                                          frame=['plate']))['animation']
        assert held['view']['home']['frame'] == {'lo': pytest.approx([0, 0, 0]),
                                                 'hi': pytest.approx([0.3048, 0.3048, 0])}
        assert held['setup']['scale'] == 2.0
        path = tmp_path / 'plate.hforge'
        plate.save(path)
        out = tmp_path / 'plate.html'
        assert main(['export-animation', str(path), '-o', str(out), '--geometry', 'Geometry',
                     '--shapes', 'Shape Set', '--frame', 'plate']) == 0
        view = read_viewer(out.read_text(encoding='utf-8'))['animation']['view']
        assert view['home']['frame']['hi'] == pytest.approx([0.3048, 0.3048, 0])
        capsys.readouterr()
        assert main(['export-animation', str(path), '-o', str(out), '--geometry', 'Geometry',
                     '--shapes', 'Shape Set', '--frame', 'wing']) == 1
        assert "no element group named 'wing'; it has 'plate'" in capsys.readouterr().out
    finally:
        geometry.group_name[0] = named
