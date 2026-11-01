"""DOF arrows: labeled direction markers on the geometry.

Force DOFs come from wherever the project says force was applied —
including an FRF's reference DOFs — and acceleration DOFs from every
measured response. Arrows scale to the geometry and carry their DOF
name, in the GUI's 3D scene and in report scenes alike.
"""

from __future__ import annotations

import json

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.report import (
    Report,
    dof_quantities,
    modal_template,
    quantity_dofs,
    series_dof_quantities,
    series_quantity_dofs,
)
from hertzforge.report import render_html


@pytest.fixture
def project(survey):
    shapes, frfs = survey
    frfs.define_units('m/s^2', reference_units='N')
    return {'Geometry': hertzforge.import_file(
                fixture_path('plate', 'geometry.npz')),
            'Shape Set': shapes, 'FRF': frfs}


def test_quantities_and_dofs_come_from_the_whole_project(project):
    assert dof_quantities(project) == ['acceleration', 'force']
    frf = project['FRF']
    assert quantity_dofs(project, 'force') == sorted(
        set(frf.reference_dof)), 'an FRF per force names its force DOFs'
    accelerations = quantity_dofs(project, 'acceleration')
    assert set(accelerations) == set(frf.response_dof)
    assert quantity_dofs(project, 'voltage') == []


def test_a_record_pick_narrows_the_dofs(project):
    """The selection is the source of truth: picked grid records give
    arrows for exactly those records' DOFs."""
    frf = project['FRF']
    series = [('FRF', frf, [0])]
    assert series_quantity_dofs(series, 'acceleration') == [
        frf.response_dof[0]]
    assert series_quantity_dofs(series, 'force') == [frf.reference_dof[0]]
    assert series_dof_quantities(series) == ['acceleration', 'force']


def test_a_dofs_scene_carries_labeled_arrows(project):
    report = Report('R', [
        {'kind': 'scene', 'geometry': 'Geometry', 'shapes': '',
         'dofs': 'force', 'dofs_source': 'FRF',
         'caption': 'Excitation DOFs'},
        {'kind': 'scene', 'geometry': 'Geometry', 'shapes': '',
         'dofs': 'acceleration', 'dofs_source': 'FRF',
         'caption': 'Response DOFs'},
        {'kind': 'scene', 'geometry': 'Geometry', 'shapes': '',
         'dofs': 'voltage', 'dofs_source': 'FRF',
         'caption': 'nothing measured as voltage'},
    ])
    payload = json.loads(render_html(report, project).split(
        'type="application/json">')[1].split('</script>')[0])
    assert len(payload['blocks']) == 2, (
        'no voltage anywhere, so that scene renders nothing')
    force, response = payload['blocks']
    assert [a['label'] for a in force['arrows']] == quantity_dofs(
        project, 'force')
    geometry = project['Geometry']
    rows = {int(n): i for i, n in enumerate(geometry.node_id)}
    axis_color = {'X': '#e5534b', 'Y': '#3fb950', 'Z': '#4c92d9'}
    for arrow in force['arrows'] + response['arrows']:
        node = int(arrow['label'].rstrip('+-XYZR'))
        assert arrow['node'] == rows[node]
        assert np.linalg.norm(arrow['vector']) == pytest.approx(1.0)
        letter = arrow['label'].rstrip('+-').lstrip('0123456789R')[:1]
        assert arrow['color'] == axis_color[letter], (
            'colored by axis, the orientation marker convention')
    # forces point into the node; responses leave it
    assert force['arrows_incoming'] is True
    assert response['arrows_incoming'] is False
    # one shared length per plot. On this survey both sets sit at the
    # 12%-of-extent cap — the grid is not dense enough to shrink below
    # it — so equality is allowed; the shrink rule itself is pinned in
    # test_dense_arrows_shrink_below_the_cap.
    assert len(response['arrows']) > len(force['arrows'])
    assert response['arrow_length'] <= force['arrow_length']


def test_dense_arrows_shrink_below_the_cap():
    """The rule the scenes rely on: arrows never overlap a close
    neighbor, however coarse the 12%-of-extent default would be."""
    from hertzforge.drawing.viz.geometry import dof_arrow_length

    sparse = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]
    assert dof_arrow_length(sparse, extent=1.0) == pytest.approx(0.12)
    dense = [[0.01 * i, 0.0, 0.0] for i in range(11)]
    assert dof_arrow_length(dense, extent=1.0) == pytest.approx(0.009)


def test_the_template_includes_excitation_and_response_dofs(project):
    """The DOFs figures read from the FRFs by default — the excitation
    from its references, the responses from its responses — in the
    standing order: excitations first, then the responses (Brandon,
    2026-08-23). Only the quantities the source measures get a scene
    (2026-09-08): an FRF per unit force names force and acceleration
    and no voltage, so no voltage card is left empty in the editor."""
    template = modal_template(project)
    blocks = [b for b in template.blocks
              if b.get('kind') == 'scene' and b.get('dofs')]
    assert [b['dofs'] for b in blocks] == ['force', 'acceleration']
    assert all(b['dofs_source'] == '@basis:Frf' for b in blocks)
    # a stale source makes the block an unbound slot, never an error
    template.blocks[template.blocks.index(blocks[0])]['dofs_source'] = 'X'
    assert template.blocks.index(blocks[0]) in template.unbound(project)


def _grid(nodes_per_side):
    from hertzforge import mesh

    return mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0),
                      1.0 / (nodes_per_side - 1))


def test_the_arrow_length_does_not_compare_every_pair():
    """The nearest spacing by tree: the all-pairs table it replaced held
    nodes² × 3 floats — 600 MB at five thousand nodes."""
    from hertzforge.drawing.viz.geometry import dof_arrow_length

    rng = np.random.default_rng(0)
    points = rng.uniform(0, 1, (20000, 3))
    points[7] = points[3] + [0.0, 0.0, 1e-4]
    assert dof_arrow_length(points, extent=1.0) == pytest.approx(0.9e-4)


def _turned():
    """Two nodes, the first measured in a system turned 90 degrees about
    Z: its X is global Y, its Y global -X."""
    from hertzforge.core.geometry import Geometry

    turned = np.array([[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0],
                       [0.0, 0.0, 0.0]])
    return Geometry(np.array([1, 2]), np.array([[0.0, 0.0, 0.0],
                                                 [1.0, 0.0, 0.0]]),
                    node_disp_cs=[2, 1], cs_id=[1, 2], cs_type=[0, 0],
                    cs_matrix=np.stack([np.vstack([np.eye(3), np.zeros(3)]),
                                        turned]),
                    length_unit='m')


def test_a_report_s_arrows_turn_with_the_displacement_system():
    from hertzforge.core.data import TimeHistory

    history = TimeHistory(np.arange(8) / 8.0, np.zeros((1, 8)),
                          response_dof=['1X+'])
    history.define_units('m/s^2')
    report = Report('R', [{'kind': 'scene', 'geometry': 'Geometry',
                           'shapes': '', 'dofs': 'acceleration',
                           'dofs_source': 'Run', 'caption': 'DOFs'}])
    payload = json.loads(render_html(
        report, {'Geometry': _turned(), 'Run': history}).split(
        'type="application/json">')[1].split('</script>')[0])
    (arrow,) = payload['blocks'][0]['arrows']
    assert np.allclose(arrow['vector'], [0.0, 1.0, 0.0])
