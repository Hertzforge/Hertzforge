# Strain and Stress Shapes

A mode shape is a displacement at every node. Its strain is the derivative of that displacement, and
in a finite element model the derivative is continuous inside an element and jumps from one element
to the next. So Hertzforge reads strain and stress element by element, the way an Exodus file holds
element variables, and keeps them as an element shape set beside the shape set they were read from.

## Reading a set

In the app, pick a shape set solved on a model of solids (Solve Modes) and run Strain and Stress.
The form previews the model with each element colored by one component while you type; Apply adds
the set to the shape set's group. Leave Element groups blank to read every group of solids, or give
the groups wanted, by number. Pick modes in the tree to read only those. Strain and stress of a
large model take a lot of memory: the RC plate's 52,000 bricks at nine stress points each, for 38
modes, are 1.6 GB, and a set past 1 GB is refused with its size, so pick the groups the question is
about (the RC's channels and beam are 800 bricks).

From Python:

```python
name = project.element_shapes('RC Plate Modes', groups=['right channel', 'left channel', 'beam'])
read = project[name]
read.values('stress', 'xx', 6)          # each element's xx stress at its centroid, mode 7
read.stress[6, read.rows_of(read.elements[0])]   # one brick's nine stress points, all six components
read.strain_energy()                    # per mode, half the weighted stress dot strain
```

## What a set holds

Every row is a stress point: one element at its centroid (point 0) or at one of the points of the
volume rule its stiffness was integrated with (points 1 on). A brick has nine rows, a wedge seven, a
ten-node tetrahedron 28 and a four-node tetrahedron two. For each mode and row the set holds the six
strain and the six stress components, in the order xx, yy, zz, xy, yz, xz, in the global axes and
in SI, per unit modal coordinate. The shear strains are engineering shears (du/dy + dv/dx). Each row
also has its position and its weight, its share of the element's volume, which is zero at the
centroid.

The components are what a response calculation needs. They are linear in the modal coordinates, so
a response's stress at a point is the sum over the modes of each mode's stress times its modal
coordinate. Von Mises and principal stresses are not linear in the modal coordinates, so they are
found from the summed components, not summed themselves.

The values come from the element's own strain matrix and elasticity, the ones its stiffness is
built from. For the brick, Wilson's incompatible modes are recovered from the nodal displacements
before the strain is formed, through the same static condensation the stiffness used. Read at the
rule's points with their weights, a mass-normalized mode's strain energy over the whole model is
omega^2 / 2, which is a check a reader can make with `strain_energy`.

Solids are covered: bricks (element types 115 and 104), wedges (112 and 101) and tetrahedra (111
and 118). A group of plates, beams or any other element is refused by name; point masses, springs,
rigid links and grounds hold no strain field and are passed over when no groups are named.

## Strain at a gauge

A channel table's channel of type strain, with a node and a direction, is a strain gauge: it reads
the normal strain along its direction at its node, in the node's displacement frame. Each solid
meeting at the node is read there through its own strain matrix, the brick's incompatible modes
included, and the strain tensors are averaged over those solids. A gauge turned end for end reads
the same. The node has to be on the solid's surface, and the direction in the surface there, within
10 degrees of one of the outside faces at the node; an inside node, a node on no solid with a
material, and a direction out of the surface are refused by name. A gauge reads the strain at a
point, where a real gauge averages over its grid, so the mesh around a gauge wants to be finer than
the grid is long.

When the table is on a test geometry and the shape set on the model, each strain channel's node is
matched to the model's nearest node, as Project onto Basis DOFs matches it, and its direction is
taken on the test geometry. A channel with no model node in reach is dropped and named.

Expand to Physical Responses with modal FRFs and a channel table answers each strain channel with
its strain per unit force, each mode's gauge strain standing where a shape's row would, beside an
acceleration at the same node and direction if there is one. The reading says how many strain
channels it read as gauges. Project onto Basis DOFs onto a table leaves strain-only channels out,
since a shape set holds displacements, and its reading says which. From Python:

```python
values, dofs, report = project.gauge_strains('RC Plate Modes', 'RC Plate Channels')
# values is (modes, strain channels), strain per unit modal coordinate
```

## Reading it with care

- A truncated set of modes converges more slowly for stress than for displacement. A response built
  from the modes up to some frequency gets the displacement nearly right with few modes, but the
  local stress near supports, bolts and other stiff details needs the higher modes and the static
  correction they stand in for. Check the stress there against a set solved to a higher frequency.
- Stress from linear elements converges more slowly with the mesh than their displacement does. A
  stress that changes quickly from one element to the next needs a finer mesh there.
  The integration points read the strain better than the nodes would, which is why the set is kept
  at the points and not averaged to the nodes.
- A rigid-body mode at 0 Hz strains nothing. What it holds is round-off, and the view draws it at
  zero.

## The view

The set opens on its model, animated in the modes picked, with each element read drawn in its own
flat color at its centroid, in the display system's units (psi in the inch systems); the rest of
the model is drawn gray. Color By draws it by another quantity and scalar: one component of the
strain, or for stress also von Mises, Tresca or a principal stress. It can limit the colors to some
of the element groups read, and state the top of the color scale. Each mode's scalar is formed
from its tensor at each point of its cycle, so the colors swing with the shape; a component runs
from minus the mode's largest value to plus it, von Mises and Tresca from zero. The view reads the
displacement from the shape set the strain was read from; with that set gone, the modes are drawn
still.

## Stress as the model moves

Modal coordinates played on the model color the elements the same way. Pick the geometry, the
shape set, the modal displacements and the element shape set together, or pick the element shape
set and the clip and run Play Strain/Stress, whose form says what to color by, which groups, and
the top of the scale. At every frame each element's tensor is the sum over the played modes of its
stress shape times the mode's coordinate at that instant, and the color is the scalar of that sum:
the service sends each mode's tensors once, and the 3-D view sums them as it sums the displacement,
so a long clip on a large model is never stored element by element in time. The scale is the
clip's largest value of the scalar over every element and frame, or the value stated, so every
frame, and every clip drawn on one stated scale, reads on the same colors. The clip has to be
modal displacement of the shape set the strain was read from, and the set has to hold every mode
the clip plays.

A recording does the same with no window open, the legend drawn into each frame:

```python
from hertzforge.record import Recording, record

record(project, Recording(geometry='RC Plate', data='Pseudoinverse Deformation',
                          shapes='RC Plate Modes', strain_stress=name, color='von_mises',
                          frame=['right channel', 'left channel', 'beam'],
                          color_groups=['right channel', 'left channel', 'beam'],
                          color_max=25.0, units='mm-kg-N-s'), 'pinv.mp4')
```

`hertzforge record` takes the same settings as `--strain-stress`, `--quantity`, `--color`,
`--color-max`, `--color-groups` and `--units`, and `hertzforge still` with them draws one frame of
the clip at `--at` seconds, or of a mode at `--at` of its cycle, to a PNG.
