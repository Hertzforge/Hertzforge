"""Models to try the toolset on, built with the toolset.

There are three: the free plate (`plate`), the BARC (`barc`) and the
Frame-Wing (`frame_wing`). Each is built with this package alone
(`hertzforge.fem`, `hertzforge.mesh` and the project verbs), so every
number they produce (geometry, mode shapes, synthesized FRFs) is
computed by this package.

    from hertzforge.demo import plate

    model = plate.build()
    shapes = model.eigensolution(maximum_frequency=3000, damping=0.02)
    model.geometry().plot()

A model is built by calling `build()`, not when the module is imported:
the Frame-Wing takes a minute to mesh and solve, which an import that
only wanted the module should not have to spend.
"""

from typing import Any

#: The submodules are not imported here. `from hertzforge.demo import
#: plate` resolves one on its own, and importing it eagerly would mean
#: `python -m hertzforge.demo.plate` loads the module twice — which Python
#: warns about, having already put the first copy in sys.modules.
__all__ = ['barc', 'color_groups', 'frame_wing', 'plate']


def color_groups(geometry: Any, colors: dict[str, int]) -> None:
    """Give every element of each element group named in `colors` that
    group's color, an index into `drawing.viz.geometry.PALETTE`. A
    meshed model's elements all come out color 1, and the 3-D view
    draws each element in its own color, so without this the parts of an
    assembly cannot be told apart."""
    for group, name in zip(geometry.group_id, geometry.group_name, strict=True):
        if name in colors:
            geometry.elem_color[geometry.elem_group == group] = colors[name]
