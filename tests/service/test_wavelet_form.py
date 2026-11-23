"""Wavelet's form: drawn as it opens and again after each change, with no Apply, and asking
which record by name when more than one is in play."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.data import TimeHistory
from hertzforge.service import display_units, projects
from hertzforge.service.server import OPERATIONS

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
US = 'in-slinch-lbf-s (g)'


@pytest.fixture
def state():
    held, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    return held


def wavelet_row(state, records=None):
    out = projects.actions(state, {'objects': ['Time History'], 'records': records or {},
                                   'part': None})
    return next(r for r in out['table']['columns'][0]['rows'] if r['id'] == 'project.wavelet')


def test_the_form_is_live_and_drawn_as_it_opens(state):
    row = wavelet_row(state)
    assert row['live'] is True and row['shows'] is True


def test_a_history_picked_whole_offers_every_record_by_name(state):
    row = wavelet_row(state)
    labels = row['labels']['record']
    assert list(labels) == [str(k) for k in range(1, 13)]
    assert 'record' not in row.get('omit', [])
    # an acceleration and a force at 101Z+: every label says its quantity
    assert labels['1'] == '101Z+ acceleration' and labels['9'] == '101Z+ force'
    assert labels['2'] == '104Z+ acceleration'


def test_records_picked_are_the_choices_and_the_first_is_shown(state):
    row = wavelet_row(state, {'Time History': [8, 2]})
    labels = row['labels']['record']
    assert list(labels.items()) == [('3', '110Z+'), ('9', '101Z+')], 'in record order'
    assert row['fill']['record'] == '9', 'the first picked'


def test_one_record_picked_is_filled_and_not_asked(state):
    row = wavelet_row(state, {'Time History': [3]})
    assert row['fill']['record'] == '4'
    assert 'record' in row['omit'] and 'labels' not in row


def test_labels_still_alike_take_their_record_number():
    t = np.arange(512) / 512.0
    project = Project()
    project.add('Twice', TimeHistory(t, np.vstack([np.sin(t), np.cos(t), np.sin(2 * t)]),
                                     response_dof=['1Z+', '1Z+', '2Z+'],
                                     ordinate_dim=['acceleration'] * 3,
                                     ordinate_unit=['m/s**2'] * 3))
    out = projects.actions(projects.OpenProject(project), {'objects': ['Twice'], 'records': {},
                                                           'part': None})
    row = next(r for r in out['table']['columns'][0]['rows'] if r['id'] == 'project.wavelet')
    assert row['labels']['record'] == {'1': '1Z+ acceleration, record 1',
                                       '2': '1Z+ acceleration, record 2',
                                       '3': '2Z+ acceleration'}


def test_the_flat_scalogram_reads_as_the_surface_does_in_the_display_system(state):
    op = next(o for o in OPERATIONS if o.id == 'project.wavelet')
    values = op.resolve({'history': 'Time History', 'record': 9},
                        objects={'Time History': 'time_history'})
    surface, picture, _ridge, _reading = op.run(state.project, values)
    top_si = picture['value_range'][1]
    display_units.convert([surface, picture], US)
    assert picture['value_label'] == f'Force [{surface["z"]["unit"]}]' == 'Force [lbf]'
    assert picture['value_range'][1] == pytest.approx(top_si / 4.4482216, rel=1e-5)
    assert float(np.max(picture['values'])) == pytest.approx(
        float(np.nanmax(surface['z_values'])), rel=1e-4)
