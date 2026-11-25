# Sensor positions from the data

A test is sometimes left with its data and no record of where the accelerometers were, or with a
geometry that holds only some of them. Infer Geometry places the sensors from the time histories
themselves. It finds where each sensor is and how it is turned from the way the structure moves at
low frequency, and makes a geometry with a node and a coordinate system for each sensor.

This guide is written for the test engineer doing that. It covers what the data must hold, the
preview and its settings, reading several tests together, what the data cannot give, and an example
on the BARC whose answer is known.

## How it works

Below its first elastic mode a structure moves as a rigid body: three translations and three
rotations. A channel at position r that measures along the unit vector d reads

```
a(t) = d . a0(t) + (r x d) . alpha(t)
```

where a0 is the translation and alpha the rotation. Each channel therefore reads the six motions
through six numbers of its own, the three of d and the three of r x d, which depend only on where
the sensor is and which way it points. The channels' low-frequency data gives those numbers up to an
unknown mix of the six motions. The mix is found from what must hold for every sensor: d is a unit
vector, the axes of one sensor are at right angles, and the axes of one sensor pass through one
point.

The channels of one node are taken as one sensor. A record at `12X+`, one at `12Y+` and one at
`12Z+` are a tri-axial sensor at node 12, and the directions are the sensor's own axes, not the
global ones.

## What the data must hold

- Time histories of accelerations, velocities or displacements with their units declared, all of
  one quantity. Forces and other quantities in the same history are left out, as are rotations and
  records with no node; the preview says what was left out.
- At least seven channels of translation. Three tri-axial sensors are the least that can be placed.
- All six rigid motions. A free structure driven by several shakers that push different ways has
  them. A test that shakes along one axis usually does not, and is refused with how far the sixth
  motion is from standing clear; several such tests can be read together (see Several tests).
- A band below the first elastic mode in which the structure is excited.

A channel that does not move in the band is refused by name, and so is a record that holds a sample
that is not a number. Pick the records without it and run again.

## Infer Geometry

With a time history selected in the Analysis module, **Infer Geometry** is in the action table. It opens on its preview, which changes nothing; **Preview** draws it again after a setting is
changed. The preview has four parts.

The 3-D view draws each sensor as a point where it was placed, colored by how much of its motion
the fit leaves unexplained (green when all of it is explained, amber at 2.5 %, red at 5 % and
beyond), with an arrow along each channel: X red, Y green and Z blue. A gray line runs through each sensor that has
one axis.

The plot under it shows the eight strongest motions in each third-octave band, as amplitude
densities relative to the strongest. Where the structure moves as a rigid body, six of them stand
above the other two. The band that was read is marked on the plot.

The table lists each sensor: its node, the axes it measures, its position, the percentage of its
motion that is not explained, and a note where it has one axis or its DOFs look wrong. The reading beside it gives the band, the number of lines in it,
the motion not explained over all channels, and what was and was not settled about the size and the
mirror image.

| Setting | What it does |
|---|---|
| Band from, Band to | The band the rigid motion is read in, in Hz. Left at 0 and 0, the band is chosen |
| Geometry holding some of the nodes | A geometry whose nodes, by number, are sensors already placed |
| Span of the two farthest sensors | The distance between the two sensors the reading names, when no node is known |
| The other mirror image | Takes the mirror image of the layout |

### The band

When no band is set, it is the lowest run of third-octave bands in which the sixth strongest
motion is at least five times the seventh. The run has to start within one octave of the lowest
band in which anything stands clear of the noise. A band higher up in which six motions happen to
stand clear among the elastic modes is not taken.

Type the band's ends to set it, and press **Preview** to draw the result again. A band that is given
is checked at its low end and refused if six motions do not stand clear there.

Bending adds a part to the low-frequency response that grows as the square of the frequency. To
remove it, the band is cut into as many as six parts, the six directions found in each part are
fitted against the square of its frequency, and the fit is read at zero frequency. This needs four
lines in each of at least three parts. With fewer lines the band is read whole, and the reading says
so: lengthen the frames or widen the band.

The frames are the history's own averaging. A history of one capture with no averaging set is cut
into the frames the PSD computation would detect for it.

## Several tests

Tests that each shake the structure a different way can be read together. Pick the time histories
together in the tree and run **Infer Geometry**. Every history must hold the same channels. Their
co-spectra are summed, so each test adds the rigid motions it excited.

A set of three single-axis tests, one along each axis, is the common case. On the free BARC of the
example below each such test is refused alone, two together are refused, and the three together
place the sensors as accurately as one test with every drive running.

Three single-axis tests place the sensors only if the rotations are in the data as well as the
translations. A free structure driven off its center of mass rotates. A structure on a slip table
that only translates gives three motions, and no number of such tests gives six.

## What the data cannot give

Three things are left open whatever the data, and the reading says which were settled.

The size. Every position scaled up, with the rotations scaled down by the same factor, gives the
same data. With nothing else given, the geometry has no length unit and the two sensors farthest
apart are one apart.

The mirror image. Every position reflected through a point, with the sensors' axes kept, gives the
same data. One of the two images is drawn, and **The other mirror image** takes the other.

The place of a one-axis sensor along its axis. Sliding it along the line it measures on changes
nothing it reads. It is placed on that line at the point nearest the middle of the other sensors,
and the table says it may be anywhere along the axis.

A geometry that holds some of the nodes settles these. The nodes are matched by number to the
sensors with two or three axes.

| Known nodes | What they give |
|---|---|
| Two, or more in a line | The size |
| Three not in a line | The size and the frame: the sensors are turned and moved onto them |
| Four not in a plane | The mirror image as well |

A one-axis sensor whose node is in that geometry is slid along its axis to the point nearest the
node. When no node is known, type the span between the two sensors the reading names to give the
size. Select the geometry with the time histories in the tree, or choose it in the form.

## Apply

**Apply** adds the geometry, named for the first history with "Sensors" added. Each sensor is one
node, numbered as its DOFs name it, placed in the global system and measured in a coordinate system
of its own whose axes are the sensor's. The geometry is linked with the histories when their group
has no geometry yet, so the data animates on it.

From a script:

```python
import hertzforge

project = hertzforge.Project.open('test.hforge')
found = project.motion_sensors('Run 1', 'Run 2', 'Run 3')      # the reading; adds nothing
print(found.motion.band, found.misfit, found.notes)
made = project.geometry_from_motion('Run 1', 'Run 2', 'Run 3', known='Some Nodes')
```

## An example with a known answer

`hertzforge.demo.sensor_motion` puts fourteen accelerometers on the BARC of plates: five tri-axial,
four with two axes and five with one, each turned its own way on its surface and three tipped off
it. The BARC hangs free and is driven by random forces at the four bottom corners of its box. The
responses are computed with the package's own tools (the modes, their response to the forces in
time, and the expansion to the sensors' axes), and white noise of 0.0001 g/√Hz is added. Its first
elastic mode is at 194 Hz.

```python
from hertzforge.demo import sensor_motion

project = sensor_motion.project()
found = project.motion_sensors('BARC Random', known='BARC Sensors (true)')
project.geometry_from_motion('BARC Random', known='BARC Sensors (true)', name='Found')
```

With four of the tri-axial sensors known, the other sensors of two and three axes are placed within
0.25 mm on a structure 0.2 m tall, the line of every one-axis sensor passes within 0.25 mm of its
true place, and every measured axis is within 0.25 degrees of the true one.
`sensor_motion.project(sensor_motion.AXIS_TESTS)` makes the three single-axis tests of Several
tests instead.

## Mistakes in the DOFs

A DOF labeled wrong makes a layout that no rigid sensors fit. When the first fit leaves more than
5 % of the motion unexplained, it is looked into: the sensor that fits worst is released, each of
its channels fitted on its own with nothing assumed from its node, and then the next worst, up to
three. When the other sensors then fit, the released channels' own lines say what is wrong, and the
sensors that were labeled right are placed as if nothing were wrong.

| Mistake | What is said |
|---|---|
| Two axes of a tri-axial sensor exchanged, or one reversed | The sensor is named as a left-handed set. It is placed, and its row says "left-handed as labeled" |
| A channel labeled at another sensor's node | The channel is named and left out of that node, with the sensor its line passes through |
| A one-axis sensor labeled as an axis of another node | The channel is named and left out, as measuring somewhere else |

A left-handed sensor has one axis reversed or two exchanged, and the data cannot say which, since
every such correction gives a sensor that fits. Its arrows are drawn the way its channels read, so
the three do not make a right-handed set in the view. The coordinate system it is given on Apply
keeps its X and Y and has its Z reversed, which is one of the corrections and may not be the right
one: correct the DOF with **Rename DOF** and run again.

Some mistakes leave a layout that fits as well as the right one, and are not found:

- the axes of a tri-axial sensor renamed in a cycle (X to Y, Y to Z, Z to X);
- two axes of a tri-axial sensor both reversed;
- the two axes of a two-axis sensor exchanged;
- the polarity of a one-axis sensor.

Each is the same sensor turned another way. A geometry that holds the node does not show these
either, since only its position is compared.

Releasing a sensor needs about twenty conditions left among the others, which three tri-axial
sensors and one more give only just. With fewer the mistake is not named, and the reading says only
that the motion is not explained by rigid sensors.

## Reading the result

The motion not explained is what is left of the six measured directions after the best layout of
rigid sensors is fitted to them. It is a fraction of a percent on the example. When it stays above
5 % after the DOFs were looked into, the reading warns. The cause is then something other than a
label: a sensor on a part that moves on its own in the band, or a band that reaches into the
bending.

A set of sensors with fewer than about twenty conditions between them (three tri-axial sensors
give eighteen) is fitted from random starting layouts instead of from a direct solution. When those
starts do not reach a close fit, the reading says the layout may not be the right one. Sets with no
tri-axial sensor at all are the hardest to place, and more sensors with two or three axes settle it.
