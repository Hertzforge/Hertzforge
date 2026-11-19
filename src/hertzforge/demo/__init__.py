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

#: The submodules are not imported here. `from hertzforge.demo import
#: plate` resolves one on its own, and importing it eagerly would mean
#: `python -m hertzforge.demo.plate` loads the module twice — which Python
#: warns about, having already put the first copy in sys.modules.
__all__ = ['barc', 'frame_wing', 'plate']
