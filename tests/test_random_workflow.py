"""A controller run in, a report out — and the headless path it takes.

`random_vibration_report` is the whole random vibration workflow in one
call, and `random_vibration_run` is the project it does that from. What
matters about them is not that they run but that they take the same
steps the window takes and reach the same numbers, so a report written
from a script and one written from the app are the same report.

The plots are here too. Everything the app draws for a random vibration
run has a call that renders it to a file with no window: the comparison
against the specification, both bar charts, the coherence map. A gap
there is a workflow that has to be finished by hand in the GUI.
"""

from __future__ import annotations

import pathlib

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.data import MultipleCoherence, Specification, TimeHistory

RUN = 'random.nc4'


@pytest.fixture(scope='module')
def worked_up():
    """The run, worked up once. Every test here reads it and none
    changes it — the FFTs are the expensive part and doing them per
    test buys nothing."""
    return hertzforge.random_vibration_run(fixture_path('plate', RUN))


def _measured(project):
    """(narrowband PSDs, octave-band PSDs) — the specification is a Psd
    too, and the banded one carries its own bin widths."""
    psds = [obj for obj in project.psds
            if not isinstance(obj, Specification)]
    narrow = [obj for obj in psds if obj.bandwidth is None]
    banded = [obj for obj in psds if obj.bandwidth is not None]
    return narrow[0], banded[0]


def test_the_run_arrives_worked_up(worked_up):
    """Import, PSDs, octave bands, coherence — the four steps, and the
    project typed by what the file says it is."""
    assert worked_up.project_type == 'Random Vibration'
    assert isinstance(worked_up.time_history, TimeHistory)
    assert isinstance(worked_up.specifications[0], Specification)
    assert isinstance(worked_up.coherence, MultipleCoherence)
    narrow, banded = _measured(worked_up)
    assert narrow.num_records == worked_up.time_history.num_records
    assert banded.num_records == narrow.num_records
    assert len(banded.abscissa) < len(narrow.abscissa), (
        'bands are wider than lines, so there are fewer of them')


def test_the_frames_are_detected_rather_than_demanded(worked_up):
    """The file does not carry an averaging, so one is worked out — and
    stored, because a coherence averaged differently from the PSD beside
    it would describe a different measurement."""
    averaging = worked_up.time_history.averaging
    assert averaging is not None
    assert averaging.frames > 1, (
        'a single average fits every reference exactly and reads 1.0 at '
        'every line')
    assert (worked_up.coherence.ordinate <= 1.0 + 1e-9).all()
    assert worked_up.coherence.ordinate.min() < 0.99, (
        'a coherence that is 1.0 everywhere was not averaged at all')


def test_banding_keeps_the_power_it_was_given(worked_up):
    """The octave bands are the same spectrum rearranged: the area under
    it, which is the mean square the channel carries, does not move."""
    narrow, banded = _measured(worked_up)
    for record in range(narrow.num_records):
        left, right = narrow.bin_bounds()
        whole = float(np.nansum(narrow.ordinate[record] * (right - left)))
        left, right = banded.bin_bounds()
        bands = float(np.nansum(banded.ordinate[record] * (right - left)))
        assert bands == pytest.approx(whole, rel=2e-3), (
            f'record {record} lost power on the way onto bands')


def test_the_report_binds_the_narrowband_set_not_the_bands(worked_up):
    """Both are PSDs, and the report bands for itself.

    Bound to the octave set the report would band an already-banded
    spectrum for its octave section and compare bands against lines in
    its first — a wrong report that renders perfectly.
    """
    from hertzforge.core.report import resolve_binding

    narrow, _banded = _measured(worked_up)
    name = resolve_binding('@basis:Psd', worked_up, worked_up.object_groups)
    assert worked_up[name] is narrow


def test_one_call_writes_the_report(tmp_path):
    path = hertzforge.random_vibration_report(fixture_path('plate', RUN),
                                         tmp_path / 'run.html')
    html = (tmp_path / 'run.html').read_text(encoding='utf-8')
    assert str(path) == str(tmp_path / 'run.html')
    assert 'Random Vibration Test Report' in html
    for caption in ('Control against specification',
                    'RMS error by control channel',
                    'Band outside the abort limits, by control channel',
                    'Control against specification, octave bands',
                    'Multiple coherence',
                    'Instrumentation'):
        assert caption in html, caption
    # the specification is drawn only against a measurement, never
    # alone (Brandon, 2026-09-21)
    assert 'Test specification' not in html


def test_the_report_lands_beside_the_run_by_default(tmp_path):
    import shutil

    run = tmp_path / 'shaker.nc4'
    shutil.copy(fixture_path('plate', RUN), run)
    path = hertzforge.random_vibration_report(run)
    assert path == str(tmp_path / 'shaker.html')
    assert (tmp_path / 'shaker.html').exists()


def test_a_file_with_no_time_data_says_so(tmp_path):
    with pytest.raises(ValueError, match='no time data'):
        hertzforge.random_vibration_report(
            fixture_path('plate', 'geometry.npz'),
            tmp_path / 'nothing.html')


def test_the_last_seconds_of_a_run_are_what_is_imported():
    """`last=` is the import dialog's Last field for a script: the
    window opens that many seconds before the end and runs to it, and
    the work-up is the same afterwards (Brandon, 2026-09-19)."""
    whole = hertzforge.random_vibration_run(fixture_path('plate', RUN))
    part = hertzforge.random_vibration_run(fixture_path('plate', RUN),
                                               last=10.0)
    full, cut = whole.time_history.abscissa, part.time_history.abscissa
    assert cut[-1] == pytest.approx(full[-1])
    assert cut[0] == pytest.approx(full[-1] - 10.0)
    assert cut.size < full.size
    assert set(part.names) == set(whole.names), 'worked up the same'


def test_a_last_longer_than_the_run_takes_it_whole():
    """Asking for the last 100 s of a 28 s run is asking for all of it
    (Brandon, 2026-09-19), not a mistake to refuse."""
    whole = hertzforge.random_vibration_run(fixture_path('plate', RUN))
    part = hertzforge.random_vibration_run(fixture_path('plate', RUN),
                                               last=100.0)
    assert part.time_history.abscissa.size == whole.time_history.abscissa.size
    assert part.time_history.abscissa[0] == pytest.approx(
        whole.time_history.abscissa[0])


def test_last_must_be_a_positive_span():
    with pytest.raises(ValueError, match='positive'):
        hertzforge.random_vibration_run(fixture_path('plate', RUN), last=0)


def test_a_folder_with_no_photos_says_so(tmp_path):
    (tmp_path / 'empty').mkdir()
    with pytest.raises(ValueError, match='no png or jpeg'):
        hertzforge.random_vibration_run(fixture_path('plate', RUN),
                                            photos=tmp_path / 'empty')


def test_the_workflow_needs_no_application_shell(tmp_path):
    """The promise the modal example makes, made for this one too: the
    plots are Qt's job whether a window opens or not, but the app is
    never imported."""
    import pathlib
    import subprocess
    import sys

    root = pathlib.Path(__file__).resolve().parent.parent
    probe = (
        'import os, sys\n'
        'os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")\n'
        'import hertzforge\n'
        f'hertzforge.random_vibration_report({str(root / "testdata" / "plate" / RUN)!r},'
        f' {str(tmp_path / "probe.html")!r})\n'
        'print("app imported:", "hertzforge.gui" in sys.modules)\n'
    )
    result = subprocess.run([sys.executable, '-c', probe], cwd=root,
                            capture_output=True, text=True, timeout=900,
                            check=False)
    assert result.returncode == 0, result.stderr[-2000:]
    assert 'app imported: False' in result.stdout
    assert (tmp_path / 'probe.html').exists()


def test_a_folder_takes_the_run_s_own_name(tmp_path):
    """`path` may be the file to write or the folder to write into.

    A campaign is a folder of runs and nobody wants to name each
    report after its run by hand (Brandon, 2026-09-22)."""
    folder = tmp_path / 'reports'
    folder.mkdir()
    written = hertzforge.random_vibration_report(
        fixture_path('plate', RUN), folder)
    assert pathlib.Path(written) == folder / 'random.html'
    assert pathlib.Path(written).is_file()
    # and a name that is not a folder is still the file to write
    named = hertzforge.random_vibration_report(
        fixture_path('plate', RUN), tmp_path / 'named.html')
    assert pathlib.Path(named) == tmp_path / 'named.html'


def test_a_folder_that_does_not_exist_yet_is_a_file_name(tmp_path):
    """The one case with no right answer, decided and written down:
    there is no telling a folder nobody has made from a file nobody
    has written, and guessing folder would put the report where
    nothing looks for it."""
    written = hertzforge.random_vibration_report(
        fixture_path('plate', RUN), tmp_path / 'no_extension')
    assert pathlib.Path(written) == tmp_path / 'no_extension'
    assert pathlib.Path(written).is_file()


def test_a_name_ending_in_a_separator_is_a_folder_made_if_need_be(tmp_path):
    """Taken as a folder, and then not found when the report was written
    (2026-10-13): a name that says folder is made."""
    written = hertzforge.random_vibration_report(
        fixture_path('plate', RUN), str(tmp_path / 'reports') + '/')
    assert pathlib.Path(written).parent == tmp_path / 'reports'
    assert pathlib.Path(written).is_file()


def test_a_scene_names_no_unit_and_the_report_asks_for_none(tmp_path):
    """A scene draws a shape, never a coordinate or a real scale.

    The hint under each geometry figure used to end "coordinates in
    <unit>", and the only thing a declared length unit changed in a
    whole report was that label and the numbers behind it — no
    spectrum, no compliance number, no verdict (Brandon, 2026-09-22).
    Both are gone: the scenes say nothing about units, and the report
    call no longer takes one.
    """
    import inspect
    import json
    import re

    assert 'length_unit' not in inspect.signature(
        hertzforge.random_vibration_report).parameters, (
        'the report takes no length unit'
    )
    # while the run, whose project lives on, still declares one
    assert 'length_unit' in inspect.signature(
        hertzforge.random_vibration_run).parameters

    written = hertzforge.random_vibration_report(
        fixture_path('plate', RUN), tmp_path / 'scenes.html',
        geometry=fixture_path('plate', 'geometry.npz'))
    page = pathlib.Path(written).read_text(encoding='utf-8')
    assert 'coordinates in ' not in page, 'no scene names a unit'
    assert 'units undefined' not in page, (
        'nor says it lacks one, having stopped claiming to need it'
    )
    payload = json.loads(re.search(r'<script id="data"[^>]*>(.*?)</script>',
                                   page, re.DOTALL).group(1))
    scenes = [b for b in payload['blocks'] if b['kind'] == 'scene']
    assert len(scenes) == 3, 'geometry, excitation DOFs, response DOFs'
    assert not any('unit' in scene for scene in scenes), (
        'and none of them carries the field that fed the label'
    )


def _history(level, dof='101Z+'):
    from hertzforge.core.data import TimeHistory

    rng = np.random.default_rng(0)
    return TimeHistory(np.arange(0.0, 1.0, 0.01),
                       rng.normal(0.0, level, (1, 100)),
                       response_dof=[dof], ordinate_dim='acceleration',
                       ordinate_unit='m/s**2')


def test_the_quieter_recording_is_the_ambient_one():
    """A system identification is two recordings — the room, then the
    room with the shakers running — and the file does not say which is
    which. The quieter one is the ambient, by the only measure always
    there (Brandon, 2026-09-22)."""
    from hertzforge.project import _quiet_and_driven

    project = hertzforge.Project()
    project.add('First', _history(1.0))
    project.add('Second', _history(0.01))
    assert _quiet_and_driven(project) == ('Second', 'First')

    # and the other way round, so the order in the file proves nothing
    other = hertzforge.Project()
    other.add('First', _history(0.01))
    other.add('Second', _history(1.0))
    assert _quiet_and_driven(other) == ('First', 'Second')

    # one recording has no ambient at all
    alone = hertzforge.Project()
    alone.add('Only', _history(1.0))
    assert _quiet_and_driven(alone) == (None, 'Only')

    with pytest.raises(ValueError, match='no time data'):
        _quiet_and_driven(hertzforge.Project())


def test_a_system_id_run_names_its_streams_and_measures_the_plant(tmp_path):
    """The workup, on the one recording a committed fixture holds: the
    stream named for what it is, the plant by H1, the coherence and a
    density. With no ambient the signal-to-noise stands down on its
    own rather than refusing the report."""
    project = hertzforge.system_id_run(fixture_path('plate', RUN))
    assert project.project_type == 'System ID'
    assert 'Excitation Time History' in project
    assert 'Noise Time History' not in project, 'this file holds one stream'
    kinds = {type(obj).__name__ for obj in project.values()}
    assert {'Frf', 'MultipleCoherence', 'Psd'} <= kinds

    written = hertzforge.system_id_report(
        fixture_path('plate', RUN), tmp_path / 'sysid.html')
    page = pathlib.Path(written).read_text(encoding='utf-8')
    assert 'System ID Report' in page
    assert 'The measured plant' in page


def test_the_ambient_stream_borrows_the_excitation_s_frames():
    """A ratio of densities only exists on lines both hold.

    The signal-to-noise the report draws is the driven density over
    the ambient one, so the two must be averaged on the same frames —
    and nothing in the file makes that happen. No committed recording
    holds the two streams a system identification is made of, so the
    second is built here from the first, quieter by a factor of a
    hundred, and the work is done through the seam that does not read
    a file.
    """
    import copy
    import dataclasses

    from hertzforge.core.data import TimeHistory
    from hertzforge.project import work_up_system_id

    project = hertzforge.Project()
    project.import_file(fixture_path('plate', RUN))
    driven = next(name for name, obj in project.items()
                  if isinstance(obj, TimeHistory))
    quiet = copy.deepcopy(project[driven])
    quiet.ordinate = np.asarray(quiet.ordinate, dtype=float) / 100.0
    # framed differently to begin with, or borrowing the excitation's
    # frames would be a line that changed nothing and a test that
    # proved nothing
    quiet.averaging = dataclasses.replace(quiet.averaging,
                                          frame_length=1024, overlap=0.0)
    project.add('Ambient', quiet)
    assert quiet.averaging != project[driven].averaging

    noise, excitation = work_up_system_id(project)
    assert (noise, excitation) == ('Noise Time History',
                                   'Excitation Time History')
    assert project[noise].averaging == project[excitation].averaging, (
        'the ambient is averaged on the frames the excitation was'
    )
    psds = [name for name, obj in project.items()
            if type(obj).__name__ == 'Psd']
    assert len(psds) == 2, 'a density from each stream'
    first, second = (np.asarray(project[name].abscissa) for name in psds)
    assert np.array_equal(first, second), (
        'and on one set of lines, or the ratio has nowhere to live'
    )
