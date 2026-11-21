"""Estimate Again on a converted Modal Test run, as the shell reaches it
(`project.reprocess_modal_test`): a measurement's frames estimated again into new objects
beside them, the run's own FRFs left as they are, and what it refuses in the core's
words."""

from __future__ import annotations

import numpy as np
import pytest
import run_folders as rf

from hertzforge import Project
from hertzforge.core.data import Coherence, Frf
from hertzforge.core.modal_test import frf_tau
from hertzforge.io import run_convert
from hertzforge.service import projects, server
from hertzforge.service.ops_modal_test import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

OPS = {op.id: op for op in OPERATIONS}
FRAMES = 'Measurement 1 frames'


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


@pytest.fixture
def project(tmp_path):
    folder = rf.modal_test_run(str(tmp_path / 'run'), 'fixed_hammer', hits=3)
    return Project.open(run_convert.convert(folder).path)


def test_modal_test_estimate_again_adds_frfs_and_coherence_beside_the_frames(project,
                                                                            validate):
    measured = project['modal_frf'].ordinate.copy()
    views = run(project, 'project.reprocess_modal_test', source=FRAMES, method='H1',
                force=0.05, exponential=0.05, block=0, captures='1, 3', shaker_window='')
    wire_views(views, validate)
    frf, coherence = project[f'{FRAMES} H1 FRFs'], project[f'{FRAMES} H1 coherence']
    assert isinstance(frf, Frf) and isinstance(coherence, Coherence)
    assert frf.response_dof == [f'{k}Z+' for k in range(1, 7)]
    assert frf.reference_dof == ['1Z+'] * 6
    assert frf.averaging.frames == 2 and frf.averaging.window == 'boxcar'
    tau = -(rf.MODAL_BLOCK - 1) / np.log(0.05) / rf.FS
    assert frf_tau(frf) == pytest.approx(tau)
    rows = {row['label']: row['value'] for row in views[0]['rows']}
    assert rows['Frames'] == 2.0 and rows['Exponential tau'] == pytest.approx(tau)
    np.testing.assert_array_equal(project['modal_frf'].ordinate, measured)
    assert 'project.reprocess_modal_test(' in project.journal[-1]
    assert frf.source == 'SIM'


def test_modal_test_estimate_again_without_windows_finds_the_structure(project):
    run(project, 'project.reprocess_modal_test', source=FRAMES, method='H1', force=0.0,
        exponential=0.0, block=0, captures='', shaker_window='')
    frf = project[f'{FRAMES} H1 FRFs']
    # the frames' FRF of 2Z+ for the hammer at 1Z+ is the structure's, as the run's is
    lines = (frf.abscissa > 2) & (frf.abscissa < 140)
    np.testing.assert_allclose(frf.ordinate[1, lines],
                               rf.modal_accelerance(frf.abscissa[lines], 1, 0), rtol=1e-6)


@pytest.mark.parametrize('given, words', [
    ({'block': 1}, 'from 2 samples'),
    ({'captures': '4'}, 'frames run from 1 to 3'),
    ({'shaker_window': 'hann', 'force': 0.1}, 'not used with the force'),
    ({'captures': '2'}, 'average'),
])
def test_modal_test_estimate_again_refuses_in_the_cores_words(project, given, words):
    base = {'source': FRAMES, 'method': 'H1', 'force': 0.0, 'exponential': 0.0, 'block': 0,
            'captures': '', 'shaker_window': ''}
    with pytest.raises(ProtocolError, match=words):
        run(project, 'project.reprocess_modal_test', **{**base, **given})


def test_modal_test_estimate_again_is_in_the_catalog():
    ids = {op.id for op in server._operations()}
    assert 'project.reprocess_modal_test' in ids
