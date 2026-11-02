"""Project types: gray slots for what a report of that kind expects.

Setting a project type lists every object the report needs; missing
ones show as gray placeholders that disappear as the real objects
arrive, offer to compute themselves where the data allows, and ride
the project file so a reopened project remembers what it is.
"""

from __future__ import annotations

import re

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import Psd, TimeHistory
from hertzforge.core.report import PROJECT_TYPES, Report, project_expectations

ROLE_REFERENCE = None  # resolved from the module in a fixture


def _burst_time():
    t = np.arange(256) / 256.0
    rows = [np.sin(2 * np.pi * 16 * t)] * 2
    time = TimeHistory(t, np.array(rows), response_dof=['1X+', '1X+'],
                       block=['avg 1', 'avg 2'])
    time.define_units('m/s^2')
    return time


def test_expectations_name_what_the_templates_bind():
    expectations = project_expectations('Modal Test')
    names = [name for name, *_rest in expectations]
    # Matched Modes comes after both groups, not between them: it
    # belongs to neither side, and a group's rows have to stay
    # contiguous to be drawn as one bracket
    # every slot named for its type and nothing more (Brandon,
    # 2026-09-02): the comparison pair reads as a second Geometry and
    # a second Shape Set, on the side that has no name
    assert names == ['Geometry', 'Photos', 'Channel Table',
                     'Time History', 'PSD', 'FRF', 'Multiple Coherence',
                     'Shape Set', 'Geometry', 'Shape Set',
                     'Matched Modes', 'Report']
    from hertzforge.core.report import OTHER_SIDE

    sides = [side for *_rest, side in expectations]
    assert sides == ['Basis'] * 8 + [OTHER_SIDE] * 2 + [None] * 2, (
        'eight Basis slots, the comparison pair on the other side, and '
        'two slots on neither')
    optional = [name for name, _cls, _icon, _count, opt, _group
                in expectations if opt]
    assert optional == ['Geometry', 'Shape Set', 'Matched Modes'], (
        'correlation is best practice, not required')
    groups = {name: group for name, _cls, _icon, _count, _opt, group
              in expectations if name not in ('Geometry', 'Shape Set')}
    assert groups['FRF'] == 'Basis'
    assert groups['Photos'] == 'Basis', (
        'setup photos document the test — they belong with its group')
    assert groups['Report'] is None, (
        'a report reads the whole project and belongs to no one side')
    assert groups['Matched Modes'] is None, (
        'and a comparison names a set on each side, so it is on neither')
    assert project_expectations(None) == []


@pytest.mark.parametrize('project_type', PROJECT_TYPES)
def test_every_type_expects_the_report_itself_last(project_type):
    """The report is the thing the rest of it is for.

    A project holding everything a report draws from and no report is
    not a finished project, so the report is a slot like the others —
    gray until it is generated, and last, because it is what the others
    add up to.
    """
    from hertzforge.core.report import PROJECT_TEMPLATES

    # one report a template: a random-and-sine project makes two, the
    # random's and the sine's (2026-10-05)
    reports = len(PROJECT_TEMPLATES[project_type])
    expectations = project_expectations(project_type)
    for k, slot in enumerate(expectations[-reports:], start=1):
        name, cls, icon, count, optional, group = slot
        assert (name, icon, count, optional, group) == (
            'Report', 'Report', k, False, None)
        assert cls is Report
    assert all(slot[1] is not Report
               for slot in expectations[:-reports]), 'the reports are last'


@pytest.mark.parametrize('project_type', PROJECT_TYPES)
def test_a_type_has_a_slot_for_everything_its_report_binds(project_type):
    """Nothing the report reaches for is missing from the tree.

    The slots exist so that a project shows what its report will need
    before the report is generated — which only holds while the two
    lists agree. They drifted once already: the shock template binds
    '@basis:ShockSpecification' and '@basis:Srs', and neither token was
    a binding type at all, so both blocks resolved to nothing and the
    shock report rendered its specification and its spectra as empty
    slots however complete the project was.
    """
    from hertzforge.core.report import (
        PROJECT_TEMPLATES,
        TEMPLATE_BUILDERS,
        binding_tokens,
    )

    types = binding_tokens()
    # every report the type makes (two for a random-and-sine project)
    wanted = {token for template in PROJECT_TEMPLATES[project_type]
              for block in TEMPLATE_BUILDERS[template](
                  hertzforge.Project(), object_groups=[]).blocks
              for value in block.values()
              for token in re.findall(r'@\w+:(\w+)', str(value))}
    assert wanted, 'the template binds symbolically or this proves nothing'
    unknown = sorted(token for token in wanted if token not in types)
    assert not unknown, (
        f'{project_type} binds tokens that are not binding types at '
        f'all, so they resolve to nothing: {unknown}')
    slots = [cls for _name, cls, *_rest
             in project_expectations(project_type)]
    for token in sorted(wanted):
        cls, _banded = types[token]
        assert any(issubclass(slot, cls) or issubclass(cls, slot)
                   for slot in slots), (
            f'the {project_type} report binds {token} and the project '
            'tree never asks for one')


def test_a_random_report_is_not_finished_without_the_octave_bands():
    """The sixth-octave PSD is a deliverable, not an optional reading.

    It is the *second* `Psd` in the experimental group rather than a
    class of its own: `Psd.to_octave` returns a `Psd` because banding
    conserves the area and changes nothing about what the object is.
    """
    from hertzforge.core.report import project_expectations

    slots = project_expectations('Random Vibration')
    octave = next(s for s in slots if s[2] == 'OctavePsd')
    name, cls, icon, count, optional, tag = octave
    assert name == 'PSD', 'named for its type, like every slot'
    assert cls is Psd, 'banding does not make a different type'
    assert count == 2, 'the second PSD in the measured group'
    assert not optional, 'a random report is incomplete without it'
    assert icon == 'OctavePsd', 'and it does not wear the narrowband glyph'
    assert tag == 'Basis'

    icons = [s[2] for s in slots]
    assert icons.index('Psd') < icons.index('OctavePsd'), (
        'it is derived from the narrowband PSD and reads after it')


def test_a_random_report_wants_the_octave_band_specification_too():
    """The requirement on octave bands beside the PSD on them: the
    second Specification in the group, as the octave PSD is the second
    Psd (Brandon, 2026-09-18)."""
    from hertzforge.core.data import Specification
    from hertzforge.core.report import project_expectations

    slots = project_expectations('Random Vibration')
    octave = next(s for s in slots if s[2] == 'OctaveSpecification')
    name, cls, _icon, count, optional, tag = octave
    assert name == 'Specification'
    assert cls is Specification and count == 2 and not optional
    assert tag == 'Basis'
    icons = [s[2] for s in slots]
    assert icons.index('Specification') < icons.index('OctaveSpecification')
