"""The demonstration plate, held to its drawing and to the book.

Unlike the drone this is *not* marked slow: the plate exists to be
the fast article, and these run on every gate. The frequencies here
are the model's own frozen truth — the element itself is held to the
classical tables in `tests/test_fem.py`; what this file pins is that
the article stays the article.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.demo import plate


@pytest.fixture(scope='module')
def model():
    return plate.build()


@pytest.fixture(scope='module')
def shapes(model):
    return model.eigensolution(maximum_frequency=2500.0)


def test_it_weighs_what_the_drawing_says(model):
    # 12 x 12 x 0.5 in^3 of 0.098 lb/in^3 aluminum, in kilograms
    pounds = 0.098 * 12.0 * 12.0 * 0.5
    assert model.structural_mass == pytest.approx(pounds * 0.45359237,
                                                  rel=1e-9)


def test_it_is_free_free_with_six_rigid_modes(shapes):
    assert int(np.sum(shapes.frequency == 0.0)) == 6


def test_the_modes_are_where_the_article_froze_them(shapes):
    """The first six elastic frequencies, pinned the day the article
    was authored. If the mesh, the material or the element changes,
    this changes — deliberately or not, and this test is what makes
    it deliberate."""
    elastic = shapes.frequency[shapes.frequency > 0.0][:6]
    assert elastic == pytest.approx(
        [439.2, 647.2, 823.5, 1142.4, 1142.4, 2085.2], abs=0.05)


def test_the_degenerate_pair_is_exactly_degenerate(shapes):
    """A square plate's (2,0)/(0,2) pair is split only by asymmetry,
    and this mesh has none."""
    elastic = shapes.frequency[shapes.frequency > 0.0]
    assert elastic[3] == pytest.approx(elastic[4], rel=1e-9)


def test_the_survey_grid_is_five_by_five_with_a_corner_drive(model):
    found = plate.instrumented(model)
    assert len(found['grid']) == 25
    assert len(set(found['grid'])) == 25
    assert found['drive'] == [101], 'the corner node, survey-numbered'
    positions = np.array([model.position(n) for n in found['grid']])
    # the grid spans the whole plate, corner to corner
    assert positions[:, 0].min() == 0.0
    assert positions[:, 0].max() == pytest.approx(plate.SIDE)
    assert positions[:, 1].min() == 0.0
    assert positions[:, 1].max() == pytest.approx(plate.SIDE)


def test_the_mesh_density_is_a_dial(model):
    coarse = plate.build(mesh=6)
    assert coarse.num_nodes == 49
    # the article is the same article at any mesh: same mass...
    assert coarse.structural_mass == pytest.approx(model.structural_mass)
    # ...and the same first mode, to discretization
    fine = model.eigensolution(num_modes=7).frequency[-1]
    rough = coarse.eigensolution(num_modes=7).frequency[-1]
    assert rough == pytest.approx(fine, rel=0.02)
    assert rough >= fine, 'coarser converges from above'


def test_the_geometry_is_the_model(model):
    geometry = model.geometry()
    assert geometry.length_unit == 'm'
    assert len(geometry.node_id) == model.num_nodes
    assert list(geometry.elem_type) == [44] * len(model.plates)
