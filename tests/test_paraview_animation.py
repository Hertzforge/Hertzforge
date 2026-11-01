"""The exodus we write is the exodus ParaView animates.

Not a round trip — `test_export.py` owns those. This is the other half:
a file we can read back perfectly can still be one ParaView cannot use,
and the ways that happens are silent. Both of the ones pinned here were
found by driving the real pipeline rather than by reading the file.

ParaView animates a modal exodus with **Animate Modes**
(`vtkAnimateModes`), reading it through **vtkIOSSReader**. VTK ships
both classes and pyvista already brings VTK, so the check costs no new
dependency and asks the actual consumer instead of a model of it.

Two things have to hold, and neither is visible in a round trip because
our own reader is forgiving about both:

1. **One time step per mode, its frequency as the time.** Coincident
   frequencies collapse — six rigid-body modes at exactly 0 Hz arrive as
   one time step and five modes vanish — which is why the writer nudges
   them a hair apart.
2. **One stem, then `X`/`Y`/`Z`** — `DispX`/`DispY`/`DispZ`. IOSS joins
   components on a shared stem with those three suffixes into a single
   three-component `Disp` vector, and a vector is what Animate Modes
   needs for its input array. Named without that shape — `D1`/`D2`/`D3`,
   or a stem that does not match across the three — the file still
   loads, still reads back into hertzforge, and offers ParaView
   three unrelated scalars it cannot animate.

   Measured rather than assumed, because the obvious guesses were
   wrong in both directions. IOSS is *more* forgiving than expected
   about the separator and the case — `Disp_X`, `disp_x` and `DISPX`
   all join, as does `Ux`/`Uy`/`Uz` — and completely unforgiving about
   the suffix: `DispX`/`DispY`/`DispQ` joins nothing. So this test
   guards the stem-and-XYZ shape, not the exact spelling.

A third fact is ParaView's rather than ours, and is worth writing down
because it looks like a bug in the file: `vtkAnimateModes` publishes a
*continuous* time range 0..1 and no discrete time steps of its own. An
animation snapping to time steps therefore falls back to the only
discrete ones in the pipeline — the reader's, which are the mode
frequencies. The cure is a Sequence of frames over 0..1, and nothing
about the export changes it.
"""

from __future__ import annotations

import pytest
from conftest import fixture_path

import hertzforge


def survey(name: str) -> str:
    return fixture_path('plate', name)


@pytest.fixture(scope='module')
def source() -> tuple:
    """The survey's 64 modes — six of them at coincident frequencies,
    which is what makes the nudge worth testing."""
    shapes = hertzforge.import_file(survey('shapes.npy'), mass_unit='kg')
    return shapes, hertzforge.import_file(survey('geometry.npz'))


@pytest.fixture(scope='module')
def written(source, tmp_path_factory) -> str:
    shapes, mesh = source
    path = str(tmp_path_factory.mktemp('paraview') / 'modes.exo')
    hertzforge.io.exodus.save(shapes, path, geometry=mesh)
    return path


def first_leaf(algorithm):
    """The one block of a partitioned dataset — the mesh itself."""
    out = algorithm.GetOutputDataObject(0)
    walk = out.NewIterator()
    walk.InitTraversal()
    return walk.GetCurrentDataObject()
