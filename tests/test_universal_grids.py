"""Every expansion is a grid — even one column wide.

One format for every object that expands into sub-items means one set of
habits: the same selection, the same deletion, the same icons. Rows are DOFs
(modes for a shape set), columns are references or captures, and a single
unlabeled column when the row alone is the identity. Geometry keeps its
category list, on request.
"""

from __future__ import annotations

import pytest
from conftest import fixture_path

import hertzforge


@pytest.fixture(scope='module')
def spectra():
    return hertzforge.import_file(fixture_path('plate', 'modal_spectra.nc4'))
