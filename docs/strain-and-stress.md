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
flat color by one component at its centroid, from minus the mode's largest value to plus it, in the
display system's units (psi in the inch systems). The rest of the model is drawn gray. Color By
draws it by another quantity or component. The view reads the displacement from the shape set the
strain was read from; with that set gone, the modes are drawn still.
