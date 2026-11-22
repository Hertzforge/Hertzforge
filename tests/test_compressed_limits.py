"""A compressed save compresses a specification's limits too
(`io.native._write_array`).

A 48-channel CPSD specification carries four limit arrays the size of its
ordinate, NaN on every cross term. Written plainly they were 288 MB of the
BARC example's 406 MB; compressed they are a few.
"""

import h5py
import numpy as np

import hertzforge
from hertzforge.core.data import Psd


def specified():
    project = hertzforge.Project('limits')
    dofs = ['1X+', '2X+', '3X+']
    pairs = [(a, b) for a in dofs for b in dofs]
    freq = np.arange(0.0, 4097.0)
    level = np.full(freq.size, 1e-3)
    project.add('Spec', Psd(freq, np.array([level if a == b else 0.1 * level for a, b in pairs]),
                            response_dof=[a for a, _ in pairs], reference_dof=[b for _, b in pairs],
                            ordinate_dim='acceleration**2/frequency', ordinate_unit='m/s^2'))
    name = project.convert_to_specification('Spec')
    spec = project[name]
    auto = np.array([a == b for a, b in pairs])
    asd = np.where(auto[:, None], np.real(spec.ordinate), np.nan)
    spec.limits = {'abort_upper': asd * 4.0, 'abort_lower': asd / 4.0}
    return project, name


def test_the_limits_are_compressed_and_read_back(tmp_path):
    project, name = specified()
    path = project.save(tmp_path / 'spec', compress=True)
    with h5py.File(path) as f:
        found = []
        f.visititems(lambda key, obj: found.append(obj) if key.endswith('limit_abort_upper') else None)
        assert found and found[0].compression == 'gzip'
    back = hertzforge.Project.open(path)[name]
    for key, values in project[name].limits.items():
        assert np.array_equal(back.limits[key], values, equal_nan=True)
