"""The Specify workspace's tools: a specification derived from measured
PSDs, drawn over the records it was derived from."""

from __future__ import annotations

from pathlib import Path

import pytest

from hertzforge.service import projects
from hertzforge.service.ops_specify import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}


@pytest.fixture
def project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    state.project.compute_psds('Time History')
    return state.project


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def test_a_specification_is_derived_over_one_quantitys_records(project, validate):
    (view,) = run(project, 'project.derive_specification', dof='101Z+', margin_db=3.0)
    wire_views([view], validate)
    added = 'Time History PSDs Derived Specification'
    assert projects.kind_of(project[added]) == 'specification'
    roles = [s['role'] for s in view['series']]
    assert roles.count('reference') == 1 and roles.count('measured') == 8, \
        'the eight accelerometers, not the force gauges'
    assert view['y']['scale'] == 'log'


def test_a_set_over_several_dofs_needs_the_dof_named(project):
    with pytest.raises(ProtocolError, match='name the DOF') as error:
        run(project, 'project.derive_specification')
    assert error.value.code == 'refused'


def test_records_of_two_quantities_draw_the_larger_group(project):
    (view,) = run(project, 'project.derive_specification', records='1-12', dof='101Z+')
    assert sum(s['role'] == 'measured' for s in view['series']) == 8
