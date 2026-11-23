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
