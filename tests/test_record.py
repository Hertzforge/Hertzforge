"""Recording a geometry's animation to a video with no window (`hertzforge record`): what is
drawn, when each frame is drawn, the camera, the page, and the command line. One test draws a
real video when Chrome and ffmpeg are on the machine."""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import pytest

import hertzforge as hz
from hertzforge.__main__ import main
from hertzforge.record import (
    CYCLE_S,
    RecordError,
    Recording,
    animation_view,
    chrome_arguments,
    find_chrome,
    find_ffmpeg,
    frame_times,
    record,
    record_page,
)
from hertzforge.report.viewer import read_viewer

PLATE = Path(__file__).resolve().parent.parent / 'testdata' / 'plate'


@pytest.fixture(scope='module')
def plate():
    project = hz.Project('plate')
    project.import_file(str(PLATE / 'geometry.unv'))
    project.import_file(str(PLATE / 'shapes.npy'))
    return project


def test_measured_motion_plays_at_its_speed_or_its_length():
    """Brandon, 2026-11-14: the fridge's 0.8 s kick at 5 kHz, slowed to fit a talk."""
    info = {'motion': True, 'samples': 4000, 'step': 1 / 5000}
    tenth = frame_times(info, Recording('G', speed=0.1, fps=30))
    assert len(tenth) == 240, '0.8 s at a tenth of real time is 8 s of video'
    assert tenth[0] == 0 and tenth[1] == pytest.approx(5000 / 30 * 0.1)
    assert max(tenth) <= 3999
    assert len(frame_times(info, Recording('G', seconds=4, fps=25))) == 100
    assert len(frame_times(info, Recording('G', fps=30))) == 24, 'real time when nothing is said'
    with pytest.raises(RecordError, match='speed is above 0'):
        frame_times(info, Recording('G', speed=0))


def test_a_mode_swings_whole_cycles_of_the_app_s_own_length():
    times = frame_times({'motion': False}, Recording('G', fps=30, cycles=2))
    assert len(times) == round(2 * CYCLE_S * 30)
    assert times[0] == 0 and all(0 <= t < 1 for t in times)
    assert times[1] == pytest.approx(1 / 30 / CYCLE_S)
    with pytest.raises(RecordError, match='frame rate'):
        frame_times({'motion': False}, Recording('G', fps=0))


def test_the_recording_opens_see_through_and_from_its_camera(plate):
    """Brandon, 2026-11-14: the fridge recorded translucent, from its own Y-up view."""
    view = animation_view(plate, Recording('Geometry', shapes='Shape Set', opacity=0.45,
                                           eye=(1, 0.55, 1.2), up=(0, 1, 0)))
    assert view['opacity'] == pytest.approx(0.45)
    assert view['home'] == {'eye': [1.0, 0.55, 1.2], 'up': [0.0, 1.0, 0.0]}
    plain = animation_view(plate, Recording('Geometry', shapes='Shape Set'))
    assert 'opacity' not in plain and 'home' not in plain, "iso is the geometry's own view"
    with pytest.raises(RecordError, match='one of iso'):
        animation_view(plate, Recording('Geometry', shapes='Shape Set', view='sideways'))
    with pytest.raises(RecordError, match='at most 1'):
        animation_view(plate, Recording('Geometry', shapes='Shape Set', opacity=2))


def test_the_view_is_the_one_the_app_shows_for_the_selection(plate):
    view = animation_view(plate, Recording('Geometry', shapes='Shape Set', mode=7))
    assert view['view'] == 'shape3d' and len(view['modes']) == plate['Shape Set'].num_shapes
    with pytest.raises(RecordError, match='modes 1 to 22; not 40'):
        animation_view(plate, Recording('Geometry', shapes='Shape Set', mode=40))
    with pytest.raises(RecordError, match="no object named 'Nothing'"):
        animation_view(plate, Recording('Geometry', shapes='Nothing'))
    with pytest.raises(RecordError, match='show no animation'):
        animation_view(plate, Recording('Geometry'))


def test_the_page_is_the_report_viewer_holding_the_recording(plate):
    page = record_page(plate, Recording('Geometry', shapes='Shape Set', mode=7, width=640,
                                        height=360, scale=2.0, view='front', theme='light'))
    assert '<html lang="en" data-theme="light">' in page
    held = read_viewer(page)['record']
    assert held['view']['view'] == 'shape3d'
    assert held['setup'] == {'width': 640, 'height': 360, 'scale': 2.0, 'mode': 6}
    assert held['view']['home'] == {'eye': [0.0, -1.0, 0.0], 'up': [0.0, 0.0, 1.0]}
    with pytest.raises(RecordError, match='light or dark'):
        record_page(plate, Recording('Geometry', shapes='Shape Set', theme='blue'))


def test_the_command_says_what_it_cannot_record(plate, tmp_path, capsys):
    path = tmp_path / 'plate.hforge'
    plate.save(path)
    assert main(['record', str(path), '-o', str(tmp_path / 'v.mp4'), '--geometry', 'Geometry',
                 '--size', 'big']) == 1
    assert 'WIDTHxHEIGHT' in capsys.readouterr().out
    assert main(['record', str(path), '-o', str(tmp_path / 'v.mp4'), '--geometry', 'Geometry']) == 1
    assert 'show no animation' in capsys.readouterr().out


def _tools():
    try:
        return find_chrome(), find_ffmpeg()
    except RecordError as error:
        pytest.skip(str(error))


def test_chrome_is_started_without_the_system_keychain():
    """A Chromium for testing with no keychain item of its own had macOS ask for the login
    password partway through a recording."""
    command = chrome_arguments('/bin/chrome', '/tmp/profile', 640, 480)
    assert command[0] == '/bin/chrome' and command[-1] == 'about:blank'
    assert {'--use-mock-keychain', '--password-store=basic', '--headless=new',
            '--user-data-dir=/tmp/profile', '--window-size=640,480'} <= set(command)


@pytest.mark.slow
def test_a_mode_is_recorded_to_an_mp4_with_no_window(plate, tmp_path):
    _tools()
    out = record(plate, Recording('Geometry', shapes='Shape Set', mode=7, width=321, height=180,
                                  fps=10, seconds=1), tmp_path / 'mode7.mp4')
    probe = shutil.which('ffprobe') or str(Path(find_ffmpeg()).with_name('ffprobe'))
    said = subprocess.run([probe, '-v', 'error', '-count_frames', '-select_streams', 'v:0',
                           '-show_entries', 'stream=nb_read_frames,width,height,codec_name',
                           '-of', 'csv=p=0', str(out)], capture_output=True, text=True, check=True)
    # an odd width is made even, as H.264 with 4:2:0 color needs
    assert said.stdout.strip() == 'h264,322,180,10'


@pytest.fixture(scope='module')
def corner():
    """The plate with its elements in two named groups: 'corner', the elements within 0.1 of the
    origin in X and Y, and 'plate', the rest."""
    import numpy as np

    project = hz.Project('plate')
    project.import_file(str(PLATE / 'geometry.unv'))
    project.import_file(str(PLATE / 'shapes.npy'))
    geometry = project['Geometry']
    geometry.group_name[0] = 'plate'
    group = geometry.add_group('corner')
    xyz = dict(zip(geometry.node_id.tolist(), np.asarray(geometry.node_xyz)))
    inside = [all(xyz[n][0] <= 0.1 + 1e-9 and xyz[n][1] <= 0.1 + 1e-9 for n in conn)
              for conn in geometry.elem_conn]
    geometry.elem_group[np.asarray(inside)] = group
    return project


def test_a_recording_framed_on_groups_opens_on_their_nodes(corner):
    """The band-average movies of a component bolted to a plate: framed on the component, with
    the whole plate still drawn around it, not cut down to fit."""
    import numpy as np

    geometry = corner['Geometry']
    view = animation_view(corner, Recording('Geometry', shapes='Shape Set', frame=['corner']))
    rows = [k for k, conn in enumerate(geometry.elem_conn)
            if geometry.elem_group[k] == geometry.group_id[1]]
    used = sorted({int(n) for k in rows for n in geometry.elem_conn[k]})
    xyz = np.asarray(geometry.node_xyz)[np.isin(geometry.node_id, used)]
    assert len(used) == 16, 'the corner is 3 by 3 elements of 1 in.'
    assert view['home']['frame']['lo'] == pytest.approx(xyz.min(axis=0).tolist())
    assert view['home']['frame']['hi'] == pytest.approx(xyz.max(axis=0).tolist())
    assert view['home']['frame']['hi'][:2] == pytest.approx([0.0762, 0.0762], abs=1e-6)
    assert 'eye' not in view['home'], "iso keeps the app's own opening direction"
    # every node is still drawn, the plate as well as the corner
    assert len(view['node_ids']) == geometry.num_nodes
    both = animation_view(corner, Recording('Geometry', shapes='Shape Set',
                                            frame=['corner', 'plate'], view='top'))
    assert both['home']['eye'] == [0.0, 0.0, 1.0]
    assert both['home']['frame']['lo'] == pytest.approx([0, 0, 0])
    assert both['home']['frame']['hi'] == pytest.approx([0.3048, 0.3048, 0], abs=1e-6)


def test_a_framed_view_is_one_the_protocol_allows(corner):
    import json

    import jsonschema

    from hertzforge.service import frames

    schema = json.loads((Path(__file__).resolve().parent.parent / 'schema'
                         / 'protocol.schema.json').read_text(encoding='utf-8'))
    check = jsonschema.Draft202012Validator(
        {'$schema': schema['$schema'], '$defs': schema['$defs'], '$ref': '#/$defs/views'})
    for frame, eye in ((['corner'], None), (['corner'], (1, 0, 0))):
        view = animation_view(corner, Recording('Geometry', shapes='Shape Set', frame=frame,
                                                eye=eye))
        wire, _arrays = frames.to_wire({'views': [view]})
        check.validate(wire['views'])
    with pytest.raises(jsonschema.ValidationError):
        check.validate([{**wire['views'][0], 'home': {'frame': {'lo': [0, 0, 0]}}}])


@pytest.mark.parametrize(('frame', 'said'), [
    (['bracket'], "no element group named 'bracket'; it has 'plate', 'corner'"),
    (['corner', 'bracket'], "no element group named 'bracket'"),
    ([], 'names no element group'),
])
def test_a_frame_the_geometry_cannot_give_is_refused_by_name(corner, frame, said):
    with pytest.raises(RecordError, match=said):
        animation_view(corner, Recording('Geometry', shapes='Shape Set', frame=frame))


def test_the_page_and_the_command_carry_the_frame(corner, tmp_path, capsys):
    page = record_page(corner, Recording('Geometry', shapes='Shape Set', frame=['corner'],
                                         scale=1.5))
    held = read_viewer(page)['record']
    assert held['view']['home']['frame']['hi'][:2] == pytest.approx([0.0762, 0.0762], abs=1e-6)
    assert held['setup']['scale'] == 1.5, 'framing leaves the amplitude as it was asked'
    path = tmp_path / 'corner.hforge'
    corner.save(path)
    assert main(['record', str(path), '-o', str(tmp_path / 'v.mp4'), '--geometry', 'Geometry',
                 '--shapes', 'Shape Set', '--frame', 'corner', 'bracket']) == 1
    assert "no element group named 'bracket'" in capsys.readouterr().out


def _modal_motion():
    """The plate in millimeters, its rigid modes, and modal displacements through them."""
    import numpy as np

    from hertzforge.core.data import TimeHistory
    from hertzforge.service import projects

    state, _ = projects.import_file(None, {'path': str(PLATE / 'geometry.unv'),
                                           'units': {'length_unit': 'mm'}})
    project = state.project
    shapes = project.generate_rigid_body_modes('Geometry')
    count = project[shapes].num_shapes
    time = np.arange(50) / 100.0
    rows = np.vstack([1e-3 * np.sin(2 * np.pi * (k + 1) * time) for k in range(count)])
    project.add('Responses', TimeHistory(
        time, rows, [f'M{k + 1}' for k in range(count)], ordinate_dim='modal_length',
        comment=[f'{shapes}: modal displacement, mode {k + 1}' for k in range(count)]))
    return project, shapes


def test_a_magnification_draws_the_true_displacement_that_many_times_over():
    """Recordings of different models drawn at one scale, without the caller working out
    each model's size."""
    import numpy as np

    from hertzforge.record import drawn_scale

    project, shapes = _modal_motion()
    asked = Recording('Geometry', data='Responses', shapes=shapes, magnification=40.0)
    view = animation_view(project, asked)
    nodes = np.asarray(view['nodes'], dtype=float).reshape(-1, 3)
    diagonal = float(np.linalg.norm(nodes.max(axis=0) - nodes.min(axis=0)))
    scale = drawn_scale(view, asked)
    # the shell draws the peak at scale x 0.075 x the diagonal (`amplitudeUnit`)
    assert scale * 0.075 * diagonal == pytest.approx(40.0 * view['motion']['peak'], rel=1e-9)
    assert view['motion']['peak'] > 0
    held = read_viewer(record_page(project, asked))['record']['setup']
    assert held['scale'] == pytest.approx(scale)
    # framing closer changes neither the scale nor the size it is a share of
    framed = Recording('Geometry', data='Responses', shapes=shapes, magnification=40.0,
                       frame=[project['Geometry'].group_name[0]])
    framed_view = animation_view(project, framed)
    assert 'frame' in framed_view['home']
    assert drawn_scale(framed_view, framed) == pytest.approx(scale, rel=1e-12)


@pytest.mark.parametrize(('options', 'said'), [
    ({'magnification': 10.0, 'scale': 2.0}, 'scale or the magnification, not both'),
    ({'magnification': 0.0}, 'above 0'),
])
def test_a_magnification_that_cannot_be_drawn_is_refused(options, said):
    from hertzforge.record import drawn_scale

    project, shapes = _modal_motion()
    asked = Recording('Geometry', data='Responses', shapes=shapes, **options)
    with pytest.raises(RecordError, match=said):
        drawn_scale(animation_view(project, asked), asked)


def test_a_magnification_needs_measured_displacement_on_a_model_with_units(plate, corner):
    with pytest.raises(RecordError, match="a mode's size is arbitrary"):
        record_page(plate, Recording('Geometry', shapes='Shape Set', magnification=10.0))
    from hertzforge.record import drawn_scale

    view = animation_view(plate, Recording('Geometry', shapes='Shape Set'))
    moving = {**view, 'motion': {'quantity': 'Acceleration', 'peak': 1.0}}
    with pytest.raises(RecordError, match='of displacement, and Kick is acceleration'):
        drawn_scale(moving, Recording('Geometry', data='Kick', magnification=10.0))
    still = {**view, 'motion': {'quantity': 'Length', 'peak': 1.0}, 'unit': 'undefined'}
    with pytest.raises(RecordError, match='no length unit'):
        drawn_scale(still, Recording('Geometry', data='Kick', magnification=10.0))
    resting = {**view, 'motion': {'quantity': 'Length', 'peak': 0.0}, 'unit': 'm'}
    with pytest.raises(RecordError, match='Kick never moves'):
        drawn_scale(resting, Recording('Geometry', data='Kick', magnification=10.0))
