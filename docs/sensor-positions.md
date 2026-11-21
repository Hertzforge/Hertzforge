# Sensor positions from a measuring arm

A coordinate measuring arm gives the position and the orientation of every accelerometer on a test
article more accurately than a tape and a protractor. The arm's probe is touched to the faces of each
accelerometer, the points are exported, and Hertzforge fits each accelerometer to its points and
makes the test geometry and the channel table from the fits.

This guide is written for the test engineer doing that. It follows the order of the work: what to
probe and how to name it, the file, the import, Fit Sensors and its table, correcting a mistake made
at the arm, and what Apply makes. The example used throughout is a made-up file that
`examples/measured_points/make_points.py` writes, so every number in it can be checked.

## What to Probe

Each surface probed is one feature in the arm's software, and the feature's name says which sensor it
is on and which surface it is: `<sensor>_<label>`.

| Label | The surface |
|---|---|
| `1`, `2`, `3` | A flat face whose outward normal is the accelerometer's X+, Y+ or Z+ |
| `-1`, `-2`, `-3` | A flat face whose outward normal is X-, Y- or Z- |
| `c` | The round side of a uni-axial accelerometer |
| `p` | The top of a uni-axial accelerometer: Z+ points out of the face probed |
| `b` | The bottom of a uni-axial accelerometer, the face it is mounted by: Z+ points away from it |

A tri-axial accelerometer needs two or three faces, on different axes. With three, the faces meet at
a corner. With two, the third axis is the one that makes a right-handed set with them, and the
position along the edge they share is taken from the middle of the points. One face gives one axis,
which does not orient the accelerometer, so that sensor is left out.

A uni-axial accelerometer needs its round side and one flat face. The direction comes from the flat
face and the position from the side. A flat face labelled `3` beside a cylinder is read as the top.

A uni-axial accelerometer that is a flat block, such as the 7270A, has no round side. Probe its flat
faces as a tri-axial accelerometer's: `3` is the face it senses through, away from the mounting
surface, `1` or `-1` an end, with X+ toward the cable, and `2` or `-2` a long side. Then name its
model in the row. It gives one channel, Z+.

The sensor's name becomes its node number when it is a whole number. Three points make a plane and
five a cylinder; more show how flat the face is. A side probed over less than a third of the way
round fixes its diameter poorly, and the fit says so.

## The File

The arm's software exports a "Feature Measured Points Table" as a `.csv` file: a header that names
the length unit, then a block for each feature with its name and its points.

```
Feature Measured Points Table,,,,,
Length Units,Millimeters,,,,
Coordinate System,world,,,,
Data Alignments,original,,,,
,,,,,
Name,11_1,,,,
No. Pts,5,,,,
Min Dev,-0.002,,,,
Max Dev,0.002,,,,
,Index,Dev,X,Y,Z
Feature,1,0.002,122.880,45.218,27.237
```

The length unit is taken from the `Length Units` row and only from there. A file that names no unit,
or one Hertzforge does not know, imports with its units undefined, and the unit is declared with
**Units** before the sensors are sized.

A laser scan exported as bare `x, y, z` rows imports too, as a cloud of points to look at. Sensors
are not fitted to a cloud.

## Importing

**Import** the file. It comes in as a geometry. Every point is a node, and each feature is an
element group of its name, listed under **Points** in the geometry's tree. Selecting the geometry
draws the points, each feature in its own color.

The points are in the arm's coordinates. To put them in the part's, move or turn the geometry's
coordinate system, under Coordinate systems in its tree: the nodes are placed in it and go with it.

To write the example file and import it from a script:

```
python examples/measured_points/make_points.py probed_points.csv
```

```python
import hertzforge

project = hertzforge.Project()
scan = project.import_file('probed_points.csv')[0]
```

The example has two tri-axial accelerometers, sensors 11 and 21, and two uni-axial ones, 31 and 32.
Two things are wrong in it on purpose. Sensor 21 was probed on its X- face and named so (`21_-1`),
which is right and needs nothing. The tops of the two uni-axial accelerometers were named for each
other's sensor, which is a mistake.

## Fit Sensors

With the geometry selected, **Fit Sensors** is in the action table. It opens on its preview, which
changes nothing, and has **Apply** and nothing to fill in. The preview draws:

- every measured point, colored by how far it lies from the surface fitted to it: green on the
  surface, amber at 0.05 mm, red at 0.1 mm and beyond, gray when no surface was fitted to it;
- each accelerometer's body where it would be placed, a little under size so the points on its
  faces show;
- an arrow along each channel, X red, Y green and Z blue.

Beside the view is the table of sensors, one row a sensor.

| Column | What it holds |
|---|---|
| Sensor | The sensor's name in the file |
| Node | Its node number; typed |
| Model | Its model, from the catalog's list or any name; typed, and optional |
| Shape | Cube or Cylinder as it was probed, or the catalog body's shape |
| Length | A cube's side, or a round body's length; typed, in the display units |
| Diameter | A round body's diameter, blank for a cube; typed, in the display units |
| Fit | Placed, or why not, and what the fit warns of |

Each cell typed draws the preview again. The choices are kept with the geometry, so they are saved
with the project.

### Sizes

The node goes at the center of the body, which is half the body's size back from each face probed.
The size is therefore needed, and it comes from the first of these there is:

1. a length or diameter typed in the row;
2. the model's, when the model is in the sensor catalog or is itself a size (`Block 17.5 mm`,
   `Cylinder 16 x 9 mm`);
3. for a cube, the smallest tri-axial model in the catalog that the points fit in;
4. what was measured: a cube's side as the furthest its points reach from the corner, which is the
   least it can be, and a round body's diameter from its side.

A round body's length is taken as twice how far its top lies, along its axis, from the middle of
the points on its side: the middle of those points is taken as the middle of the body. It is right
when the side was probed evenly about its middle, and too long or too short by twice the offset
when it was not, so type the length, or name a model, when it matters. The reading under the table
says each length that was taken this way. A cube whose points reach close to its edges needs
nothing.

Choosing a catalog model fills Length and Diameter from the catalog and takes back what was typed
before it. A force transducer's model makes the row a force channel.

### The Scan Sets

Each sensor's row opens, by the mark at its left, onto the sets of points it was fitted to: the
set's name, the sensor it belongs to, its surface, its points, and how far they lie from the fitted
surface (RMS). A sensor that is not placed starts opened, since one of its sets is most likely what
is wrong.

**Sensor** is typed and **Surface** is chosen from X+, X-, Y+, Y-, Z+, Z-, Cylinder, Uni-axial Top
and Uni-axial Bottom. Changing either renames the set, which is all that says what it is, so the
tree and the table always agree. When the sensor already has a set for that surface, the two
exchange.

In the example, sensors 31 and 32 are not placed. Each row's Fit reads that its flat face is not on
its cylinder, and that `31_3` and `32_3` look exchanged. Open sensor 31, type 32 as the Sensor of
`31_3`, and both are placed: the top that was sensor 32's comes back to sensor 31.

```python
project.set_scan_set(scan, '31_3', sensor=32)
project.set_sensor(scan, 31, length=0.005)
project.set_sensor(scan, 32, length=0.009)
```

Sets whose names say no sensor are listed in a table of their own, and take a sensor there.

### What the Fit Warns Of

| The Fit says | What it means |
|---|---|
| A face is rough | Its points lie more than 0.05 mm RMS from the plane: a point on an edge, a label, or a burr |
| Faces are some degrees from square | Two faces of one accelerometer are more than 1 degree off 90: a point probed on the wrong face |
| The faces make a left-handed set of axes | One label is wrong; the sensor is not placed |
| One face gives one axis | A second face is needed; the sensor is not placed |
| The flat face is not on this cylinder | The top or bottom belongs to another sensor; the sensor is not placed |
| The flat face is some degrees from square to the side | More than 3 degrees: a rough side, or a short one |
| The side was measured over so many degrees | Less than 120: the diameter is poorly fixed |
| The flat face is not past the end of the measured side | It may be another surface, such as a flange |

## Apply

**Apply** adds two objects, linked in one group and named for the geometry:

- a geometry with one node for each accelerometer, at the center of its body. Each node is measured
  in a coordinate system of its own, named `Sensor 11`, whose axes are the accelerometer's X+, Y+
  and Z+;
- a channel table with acceleration channels in g. A tri-axial accelerometer gives X+, Y+ and Z+,
  whatever faces were probed. A uni-axial one gives the one axis its flat face was labelled with,
  always positive. A uni-axial block probed by its faces gives Z+.

Selecting the two together draws the accelerometers' bodies and their channels' arrows. A sensor
with no catalog model has its size written where its model would be, and is drawn at that size.

The measured points are left as they are. The two objects are not made again when the points or the
choices change: run Fit Sensors and Apply again. To join the new geometry to another, use **Merge**.

```python
made = project.create_sensors(scan, name='Part')
geometry = project[made['geometry']]
channels = project[made['channels']]
```

In the example the four nodes come out within 0.02 mm of the centers the file was made from, and the
two cubes take the catalog models their points fit in.
