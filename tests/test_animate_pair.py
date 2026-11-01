"""Two mode shapes overlaid, from a script: `ShapeSet.animate_pair` and
`Project.animate_pair`, the window's picked MAC cell without the window.

The pairing — the second mode phase-aligned to the first, through the
projection when the two sets live on different geometries — the caption
and the opacities are one implementation each, shared by the window,
the report's overlay block and these calls (PLAN.md, "Saving an
animation").
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path, plate_geometry_and_shapes

import hertzforge
from hertzforge.core.shapes import ShapeSet


@pytest.fixture(scope='module')
def plate():
    """The plate's dense geometry and truth shapes, a sign-flipped copy
    of them (the same modes, each the other way up), and a sparse test
    set: the truth at the ten test nodes, on the test geometry."""
    geometry, truth = plate_geometry_and_shapes()
    flipped = ShapeSet(truth.frequency, truth.damping, truth.coordinate,
                       -truth.shape_matrix)
    sparse = hertzforge.import_file(fixture_path('plate',
                                                     'test_geometry.npz'))
    sparse.define_units('m')
    kept = [k for k, dof in enumerate(truth.coordinate)
            if dof not in set(sparse.missing_dofs(truth.coordinate))]
    test = ShapeSet(truth.frequency, truth.damping,
                    [truth.coordinate[k] for k in kept],
                    truth.shape_matrix[:, kept])
    return geometry, truth, flipped, sparse, test


def test_sets_sharing_no_dofs_align_only_through_the_projection():
    """A test set and an FE model rarely share a DOF label; the
    projection — the model carried onto the test DOFs — is what the
    alignment is measured on, and it is applied to the model itself."""
    from hertzforge.core.shapes import paired_mode

    up = np.array([[1.0, 2.0]])
    test = ShapeSet([10.0], [0.01], ['1X+', '2X+'], up)
    model = ShapeSet([10.0], [0.01], ['101X+', '102X+'], -up)
    projected = ShapeSet([10.0], [0.01], ['1X+', '2X+'], -up)
    assert np.allclose(paired_mode(test, 0, model, 0, projected), up[0])
    assert np.allclose(paired_mode(test, 0, model, 0), -up[0]), \
        'without it there is nothing shared to turn by'
