"""Freeze one .hforge per schema, so no release can orphan a file.

Run when `io.native.SCHEMA_VERSION` bumps, to write that schema's file
under `hforge_corpus/`. `tests/test_hforge_corpus.py` opens every file
there and pins values that would drift if a loader regressed. A frozen
file is written once and never refreshed. The project is small on
purpose (a few KB): it is a schema exercise, not a dataset, but it
holds every object kind and the optional fields (averaging, scale_db,
complex modal mass and damping, element-group properties, object groups,
roles, project type, predictions, a run log extract) whose handling
when absent is what future readers must keep. From schema 3 it also
holds a converted control run: its channel table, streams, specification,
results and run record. The run log extract is
taken from the frozen run log in `vdlog_corpus/` and stored as if the
log sat beside the project file, so the file holds a relative path and
the log is not found beside it, a case a reader must keep too.

The three files in `vdyn_corpus/` are kept as they were written, since
Hertzforge reads `.vdyn` and no longer writes it.
"""

import os
from datetime import datetime, timedelta, timezone

import numpy as np

import hertzforge
from hertzforge.core.averaging import Averaging
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import Frf, TimeHistory
from hertzforge.core.fem import GroupProperties, Material, Section
from hertzforge.core.geometry import Geometry
from hertzforge.core.matches import MatchedModes
from hertzforge.core.photos import Photos
from hertzforge.core.prediction import Prediction, Predictions
from hertzforge.core.report import Report
from hertzforge.core.run_log_extract import RunLogExtract, take
from hertzforge.core.shapes import ShapeSet
from hertzforge.core.shocks import Shock
from hertzforge.core.sine import (
    SineLevel,
    SineLevelSet,
    SineSweepSpecification,
    SineTone,
)
from hertzforge.io.native import RUN_RECORD_SCHEMA, SCHEMA_VERSION, SUFFIX

#: the smallest legal PNG: 1x1, opaque. The corpus needs image *bytes*
#: that survive verbatim, not a picture.
ONE_PIXEL_PNG = bytes.fromhex(
    '89504e470d0a1a0a0000000d494844520000000100000001080200000090'
    '7753de0000000c4944415408d763f8cfc00000030101'
    '00c9fe92ef0000000049454e44ae426082')


HERE = os.path.dirname(os.path.abspath(__file__))


def run_log_extract() -> RunLogExtract:
    """Test 1 of the frozen run log, as if the log were beside the
    corpus file, taken at a fixed time."""
    taken = take(os.path.join(HERE, 'vdlog_corpus', 'v1.vdlog'), test=1)
    return RunLogExtract(
        taken.text, path=os.path.join(HERE, 'hforge_corpus', 'v1.vdlog'),
        test=taken.test, upto=taken.upto,
        taken=datetime(2026, 10, 30, 12, 0,
                       tzinfo=timezone(timedelta(hours=-6))),
        log_size=taken.log_size, after=taken.after,
        after_in_test=taken.after_in_test, problems=taken.problems)


def build_project() -> hertzforge.Project:
    geometry = Geometry(
        node_id=[1, 2, 3, 4],
        node_xyz=[[0.0, 0.0, 0.0], [0.5, 0.0, 0.0],
                  [1.0, 0.0, 0.0], [1.0, 0.5, 0.0]])
    geometry.add_beams([1, 2, 3, 4])
    geometry.length_unit = 'm'
    # what the element group is made of, so the file holds
    # `group_properties`, one of the names `.vdyn` kept as a block
    geometry.group_properties = {int(geometry.group_id[0]): GroupProperties(
        Material('6061-T6', 68.9e9, 2700.0, 0.33),
        section=Section('tube', 1.2e-4, 3.1e-9, 3.1e-9, 6.2e-9))}

    t = np.arange(256) / 256.0
    history = TimeHistory(
        t, [np.sin(2 * np.pi * 12 * t), 0.5 * np.cos(2 * np.pi * 20 * t)],
        response_dof=['1X+', '2Y+'],
        ordinate_dim='acceleration', ordinate_unit='m/s**2')
    history.averaging = Averaging(frame_length=64, overlap=0.5)
    history.shocks = (Shock(0.125, 0.25),)

    frequency = np.arange(33) * 4.0
    frf = Frf(frequency,
              np.outer([1.0, 2.0], 1.0 / (1.0 + 1j * frequency / 40.0)),
              response_dof=['1X+', '2Y+'], reference_dof='1X+',
              ordinate_dim='acceleration/force',
              ordinate_unit='m/s**2', reference_unit='N')

    coordinate = ['1X+', '2Y+', '3X+', '4Z+']
    shapes = ShapeSet(
        frequency=[12.5, 47.25], damping=[0.01, 0.032],
        coordinate=coordinate,
        shape_matrix=np.array([[1.0, -0.5, 0.25, 0.0],
                               [0.1 + 0.2j, 0.3 - 0.1j, -0.4j, 1.0]]),
        modal_mass=[1.0, 1.5 + 0.25j],
        modal_damping=[0.02, 0.05 - 0.01j],
        mass_unit='kg',
        comment=['first bending', 'complex import'])

    table = ChannelTable({'channel': [1, 2], 'node': ['1', '2'],
                          'direction': ['X+', 'Y+'],
                          'unit': ['m/s**2', 'm/s**2'],
                          'serial number': ['SN-1', 'SN-2']})

    tone = SineTone(
        name='fundamental', start_time=0.0,
        frequency=[20.0, 200.0], amplitude=[[1.0], [2.0]],
        segment_type=[1], segment_rate=[2.0],
        warning_upper=[[1.5], [3.0]], abort_upper=[[2.0], [4.0]])
    sine_spec = SineSweepSpecification(
        tones=[tone], response_dof=['1X+'],
        ordinate_dim='acceleration', ordinate_unit='m/s**2',
        comment='schema exercise sweep')
    lines = np.array([20.0, 63.0, 200.0])
    levels = SineLevelSet([SineLevel(
        lines, [1.02 * lines / lines], tone='fundamental', onset=1.25,
        seconds=[0.0, 30.0, 60.0], response_dof=['1X+'],
        ordinate_dim='acceleration', ordinate_unit='m/s**2')])

    report = Report(
        title='Frozen Schema Report',
        blocks=[{'kind': 'text',
                 'text': 'One block, so the JSON round-trips.'}],
        marking='CUI', marking_color='red')

    matches = MatchedModes('Shapes', 'Shapes', pairs=[[0, 0], [1, 1]],
                           macs=[1.0, 0.978], first_geometry='Geometry',
                           second_geometry='Geometry')

    photos = Photos(names=['setup'], formats=['png'],
                    images=[ONE_PIXEL_PNG])

    # predictions, one of peaks and one of a curve
    peaked = np.abs(1.0 / (1.0 - (frequency / 48.0) ** 2
                           + 1j * frequency / 480.0))
    predictions = Predictions([
        Prediction('2Y+', 'asd', [(20.0, 0.5, 10.0)], unit='g',
                   level_db=0.0, frequency_uncertainty=10.0,
                   level_uncertainty=3.0, source='schema exercise',
                   customer=True),
        Prediction('2Y+', 'transmissibility', [(48.0, None, None,
                                                 (36.0, 60.0))],
                   reference='1X+', curve=(frequency, peaked))],
        comment='schema exercise predictions')

    project = hertzforge.Project('Frozen Schema Exercise')
    project.project_type = 'Modal Test'
    project.add('Geometry', geometry)
    project.add('Time History', history)
    project.add('FRF', frf)
    project.add('Shapes', shapes)
    project.add('Channel Table', table)
    project.add('Sine Specification', sine_spec)
    project.add('Sine Levels', levels)
    project.add('Report', report)
    project.add('Matches', matches)
    project.add('Photos', photos)
    project.add('Predictions', predictions)
    # the run log brought in
    project.add('Run Log, Test 1', run_log_extract())
    # through the project's own verbs, so the file carries provenance
    # — the staleness fingerprints have to survive a save too
    psds = project.compute_psds('Time History')
    project[psds].scale_db = 3
    project['Report'].add({'kind': 'prediction', 'source': 'Predictions',
                           'measured': psds, 'level': -3.0,
                           'caption': 'schema exercise'})
    srs = project.compute_srs('Time History')
    project.link('Geometry', 'Time History', psds, srs, 'FRF', 'Shapes')
    project.set_basis('Geometry', 'Time History', psds, srs, 'FRF',
                      'Shapes')
    if SCHEMA_VERSION >= RUN_RECORD_SCHEMA:
        add_converted_run(project)
    return project


def add_converted_run(project: hertzforge.Project) -> None:
    """A short Random run, written as a run folder by the tests' own
    builder and converted, its objects added under their names (a
    clash takes a number) and grouped as the conversion grouped them.
    Schema 3 is the run record, so its file holds one."""
    import sys
    import tempfile

    sys.path.insert(0, os.path.dirname(HERE))
    from tests import run_folders

    from hertzforge.io import run_convert

    with tempfile.TemporaryDirectory() as scratch:
        folder = run_folders.random_run(os.path.join(scratch, 'golden_random'), 256)
        run = hertzforge.Project.open(run_convert.convert(folder).path)
    names = [project.add(name, obj) for name, obj in run.items()]
    project.object_groups.append({'members': names, 'role': None,
                                  'name': 'golden_random'})


if __name__ == '__main__':
    import os
    root = os.path.join(os.path.dirname(__file__), 'hforge_corpus')
    os.makedirs(root, exist_ok=True)
    path = os.path.join(root, f'schema{SCHEMA_VERSION}{SUFFIX}')
    if os.path.exists(path):
        raise SystemExit(f'{path} is frozen; a schema bump writes a '
                         'new file, never this one again')
    build_project().save(path)
    print(f'froze {path} ({os.path.getsize(path)} bytes)')
