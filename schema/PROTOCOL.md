# Suite Protocol, Version `sds/1`

The one contract between the shell (TypeScript), the analysis service (Python) and the control
service (Rust). `protocol.schema.json` beside this file is the machine-readable form; when this
text and the schema disagree, fix one of them in the same change. Golden frames in `golden/` are
the shared test data every language's codec must reproduce.

`sds` is a working name. Renaming it is a mechanical find and replace.

## Transport

WebSocket, one connection per service, loopback by default. The control service listens on
loopback only (see "Controller and Observers").

| Service  | Default address           | Started by                              |
|----------|---------------------------|-----------------------------------------|
| analysis | `ws://127.0.0.1:8791/`    | `hertzforge serve --port 8791` |
| control  | `ws://127.0.0.1:8792/`    | `cargo run -p sds-control -- --port 8792` |
| shell    | `http://127.0.0.1:5191/`  | `npm --prefix ui run dev -- --port 5191` |

On connect the **server** sends a `hello` first. The client compares `protocol` for an exact
match and closes the connection on any difference (no graceful degradation across versions).

## Frames

- **Text frame:** one UTF-8 JSON message that carries no arrays.
- **Binary frame:** one message that carries arrays. Layout, all integers little-endian:

| Offset | Size | Content |
|--------|------|---------|
| 0 | 4 | ASCII magic `SDS1` |
| 4 | 4 | `u32` header length `H` in bytes |
| 8 | `H` | UTF-8 JSON header (the message), right-padded with spaces (0x20) so `8 + H` is a multiple of 8 |
| `8 + H` | rest | Payload: the arrays, each starting at a multiple of 8 from the payload start |

The header is a normal message plus a top-level `"arrays"` list. Each entry:
`{"id": "a0", "dtype": "f64", "shape": [n, ...], "offset": o, "nbytes": b}`. `offset` is counted
from the payload start (`8 + H`), is a multiple of 8, and `nbytes` equals the element count times
the element size. Data is C order (row-major), little-endian.

Inside the message, an array value is the object `{"$array": "<id>"}`. A decoder replaces each
reference with a typed array (TS: `Float64Array` etc., Python: `numpy.ndarray`, Rust: `Vec<T>`
plus shape) and drops the `arrays` list. Every listed array is referenced exactly once.

`dtype` is one of `f32`, `f64`, `i32`, `u32`, `u8`. Complex data travels as two real arrays
(`re`, `im`) by convention; there is no complex dtype.

## Messages

```
hello     {"type":"hello","protocol":"sds/1","service":"analysis"|"control","version":"0.1.0"}
request   {"type":"request","id":<int>,"method":"<name>","params":{...}}
response  {"type":"response","id":<int>,"result":{...}}
          {"type":"response","id":<int>,"error":{"code":"<code>","message":"<text>",
                                                  "reason":"<name>","detail":{...}}}
event     {"type":"event","topic":"<name>","seq":<int>,"data":{...}}
```

Error codes: `bad_request`, `unknown_method`, `not_found`, `refused`, `internal`. The message
names the cause in words a user can act on; an error never comes back as an empty result.
`reason` (optional) names the cause for a program: a stable snake_case name a client branches
on, so no client ever parses `message`. `detail` (optional) carries the facts behind it. Every
error from the hardware and Random methods carries a `reason` (see "Refusals" below).

## Views: The Viz Kit's Input

Every result and every live event carries `views`, a list the shell renders with its shared viz
kit. Services never send pixels and the shell never computes the numbers.

**`spectrum`** (also used for any x-y line data)

```
{"view":"spectrum","title":"...",
 "x":{"label":"Frequency","unit":"Hz","scale":"log"|"linear"},
 "y":{"label":"PSD","unit":"g²/Hz","scale":"log"|"linear"},
 "series":[{"name":"...","role":"measured"|"reference"|"tolerance"|"abort"|"limit"|"drive"|"other",
            "x":{"$array":..},"y":{"$array":..},
            "shape":"line"|"steps"|"law", "edges":{"$array":..}, "follows":0}],
 "link":"frequency",
 "regions":[{"x0":0.5,"x1":0.9,"kind":"frame"|"cut","label":"..."}]}
```

`role` sets the line style so no meaning rests on color alone: measured solid, reference
dashed, tolerance dotted, abort dash-dot, limit dash-dot-dot, drive thin solid. `limit` is a
ceiling the controller holds a channel under; it never aborts. `link` (optional) names the cursor
group; panes with the same `link` share a cursor position on x.

`regions` (optional) shade spans of x behind the curves, as Visual Dynamics draws a time
history's settings on it: a `frame` is one frame its spectra are averaged from (frames that
overlap shade darker where they do), and a `cut` is a span left out, such as the ends a truncation
discards; a `span` draws its edges alone. `label` is said when the span is hovered. A region with
`"drag":{"op","params","low","high"?}` is moved on the plot: its middle slides it and, with
`high`, an edge moves that edge. On release the shell runs `op` with `params`, the new x0 as
`low` and the new x1 as `high`, and the answer's views take the result's place, as Visual
Dynamics' averaging and truncation regions are dragged. They are not listed in the legend; the
result's reading says what they are.

`follows` (optional) is the index, in this view's `series`, of the measurement a curve follows:
a model's prediction of it, such as an FRF synthesized from fitted modes. A follower is drawn
dashed in its measurement's color, above it, and left out of the legend, as Visual Dynamics draws
it. It names an earlier series that is not itself a follower.

`shape` (optional, default `line`) says how the points mean to be drawn, and the service that
computed the data decides it. Spectral content is plotted the way it is calculated:

- `steps`: each `y` is a value over its own bin and is drawn flat across it, from `edges[i]`
  to `edges[i+1]`, so the area drawn is the area a PSD integrates to. `edges` is required,
  ascending, one longer than `y`. For FFT lines the edges sit midway between lines and the
  outer edges mirror the nearest midpoints, so every bin is `df` wide; banded data sends its
  own band edges. A cursor reads the bin it is over.
- `law`: a breakpoint specification, already resampled along its power law by the service so a
  polyline is within a pixel of it. Drawn as a line.
- `line`: anything that is a sample at a point (an FRF, a coherence, a time history).

This is Visual Dynamics' drawing rule (`plot.drawing_shape`, `plot.bin_edges`); the analysis
service takes it from there rather than restating it.

**`reading`**

```
{"view":"reading","title":"...",
 "rows":[{"label":"...","value":<number|string>,"unit":"..."|null,"status":"pass"|"warn"|"fail"|"info"}]}
```

`status` is shown as a word or symbol as well as a color.

**`shape3d`**

```
{"view":"shape3d","title":"...","unit":"m",
 "nodes":{"$array":.. f32 [N,3]},
 "lines":{"$array":.. u32 [L,2]},          optional
 "triangles":{"$array":.. u32 [T,3]},      optional
 "modes":[{"index":1,"frequency_hz":12.3,"damping":0.01|null,"shape":{"$array":.. f32 [N,3]},
            "shape_imag":{"$array":.. f32 [N,3]},   optional
            "label":"Operating shape"}]}             optional
```

A shape is one translation vector per node, in the same node order as `nodes`. A complex shape (a
complex mode, or an operating deflection shape read from FRFs at one line) carries its real part in
`shape` and its imaginary part in `shape_imag`; a real shape has no `shape_imag`. The shell animates
`Re((shape + i shape_imag) exp(i theta))` over a cycle, so a real shape moves as a standing wave and
a complex one shows its traveling part. `label`, when present, names the shape in place of
"Mode <index>", for a shape that is not a mode.

`overlays` and `overlay_phase` (optional) carry a second set of shapes on the same nodes, for a
`matrix` view that `selects` this one (below).

`pick` (optional) says what a click in the view does, as Visual Dynamics' Add mode and its
planning pane take clicks. A click is a press and release in one place; a drag still turns the
view. The shell runs `op` with `params` and what was clicked, and the views it answers with take
the result's place:

```
"pick":{"op":"project.add_node","params":{"geometry":"Plate"},
        "target":"point"|"nodes"|"faces",
        "hint":"Click in the view to place a node",
        "ids":[1,2,3,...],             the node id of each of the model's own rows, first rows first
        "count":3,                     nodes: how many make one; 0 takes any number, then Enter
        "param":"nodes",               nodes: the param the ids go in, as "4, 7, 9"
        "faces":{"$array":.. i32 [T]}, faces: each triangle's surface, -1 for none
        "kept":[1,3],                  faces: the surfaces in use now
        "out":"leave_out","back":"put_back",   faces: the params a click names a surface in
        "off":2}                       nodes: marked places, shown through the model; this many of
                                       the last `ids` are left out and drawn dim
```

A `point` click sends `x`, `y` and `z` in the view's unit: where it lands on the model's surface,
on the model's plane when the model is flat, or else at the depth of the node nearest it. A
`faces` click names a surface in `kept` in the `out` param and any other in the `back` param,
taking it out of the other; the shell draws the surfaces not in `kept` muted.

A `shape3d` may instead play measured data back on the mesh: `modes` is empty and `motion` says
how each measured DOF moves, sample by sample.

```
"motion":{"label":"Time","unit":"s","start":0,"step":0.000244,   the abscissa: start + k step
          "quantity":"Acceleration","value_unit":"m/s²",
          "node":{"$array":.. u32 [D]},          the node of `nodes` each DOF moves
          "direction":{"$array":.. f32 [D,3]},   the unit direction it moves in
          "values":{"$array":.. f32 [D,S]},      DOF d at sample k
          "peak":130.4}                          the furthest any node gets, in value_unit
```

With `modes` (f32 [M,D,3]), `values` is instead [M,S], modal coordinates: `node` lists the D nodes
that move, `direction` is empty, and sample k's offset at node d is the sum over modes of
`modes[m, d] * values[m, k]`, so the physical response never has to be formed or sent.

At sample k each node is offset by the sum over its DOFs of `direction * values[d, k]`, scaled
so that `peak` is the amplitude the person chose. Sending each DOF's samples rather than a frame
of every node keeps a large mesh with a few sensors as small as the sensors' data; nodes with no
DOF stay where they are. Several DOFs may move one node (X, Y and Z, say). The shell plays a
record through in about 10 s at normal speed, from an eighth to eight times that, as Visual
Dynamics does, and colors each node by how far it is from rest at that sample.

`link` (optional) ties playback to a `spectrum` of the same result with the same `link`: the
spectrum marks the time the mesh shows with a line, and dragging that line moves playback there.
The tie holds within one result; the hover cursor `link` names is shared as for any spectrum.

**`waterfall`** (many records of one quantity, each at its own depth)

```
{"view":"waterfall","title":"...",
 "x":{"label":"Frequency","unit":"Hz","scale":"linear"|"log"},
 "z":{"label":"PSD","unit":"(m/s²)²/Hz","scale":"log"|"linear"},
 "records":["101Z+","102Z+",...],           one label per record, front to back
 "offsets":{"$array":.. u32 [R+1]},         record r is points offsets[r] .. offsets[r+1]
 "x_values":{"$array":.. f32 [P]},
 "z_values":{"$array":.. f32 [P]},          NaN is a gap: the line breaks there
 "x_range":[low, high],                     where something is drawn
 "z_range":[low, high],                     optional: a level range the data pins
 "page":{"number":1,"count":3,"records":905},   optional: which page of how many
 "quantities":[{"value":"acceleration|","label":"Acceleration","records":220},
               {"value":"force|","label":"Force","records":40}],   optional: two or more
 "quantity":"acceleration|"}                optional: the one drawn
```

Values are in display units and never logged: the shell takes log10 of an axis whose `scale` is
`log`, so a value at or below zero is a gap there. The service has already drawn each record as it
is calculated, the same rule as `spectrum`'s `shape`: a time history arrives peak-decimated to a
few thousand points that keep every extreme, a PSD as its step outline (two points per bin, never
thinned, so the area under it is its mean square), and a breakpoint specification filled in along
its power law. The shell colors every point by its level on the sequential colormap, with a
numeric legend, and names at most 40 records, every nth past that. One quantity is drawn on a
stage, since one level axis cannot hold accelerations and newtons at once; an object holding more
than one lists them in `quantities`, and choosing another reruns the operation with its `quantity`
param set to that `value`, starting again at page 1. Hovering a curve reads its record, abscissa
and level. Records that do not fit one
scene are on later pages, never drawn coarser. This is Visual Dynamics' 3-D data view
(`hertzforge.drawing.viz.waterfall`).

A waterfall can also be one of Visual Dynamics' stages, with these optional fields:

```
 "stations":["101Z+","102Z+",...],          the depth axis's labels, front to back
 "depths":[0,0,1,1,...],                    each record's station: two records can share one
 "roles":["reference","measured",...],      each record's pen; "measured" is colored by level,
                                            any other role is drawn in that role's color
 "zones":[{"station":0,"kind":"warning"|"abort","side":"upper"|"lower",
           "x":{"$array"..},"inner":{"$array"..},"outer":{"$array"..}}],
                                            a limit band; no outer runs it to the top or floor
 "exceed":[{"station":0,"side":"upper"|"lower","x0":..,"x1":..,"z":..}],
                                            lines past an abort limit: one rectangle each, from
                                            its limit's level to the top (upper) or floor (lower)
 "walls":[{"$array"..}],                    one abscissa a station: a translucent wall
 "surface":true                             the records are one surface, each on the same
                                            abscissa, rather than separate lines
 "regions":[region...]                      spans of the abscissa across the whole stage, as a
                                            spectrum's regions are, dragged the same way
 "note":"..."                               optional words under the toolbar
```

With `stations` the depth axis is the stations, not the records; `records` still names each
curve for hovering.

A waterfall may instead be a stage of paths: with `y` (an axis) and `y_values` (f32 [P], one a
point) every point stands at a depth of its own on that axis, `y_range` its span, rather than at
its record's station. This is Visual Dynamics' sine stage: frequency across, time receding,
amplitude up, each tone's requirement and limits a path through time and each extracted level its
own path placed by its clock. A `zones` entry then carries `y` beside `x` and an `exceed` entry
`y0` and `y1` beside `x0` and `x1`, so the bands and the rectangles follow the path; the depth axis
is ticked in `y`'s unit and each record is named at the start of its path. The paired stage overlays two densities at each channel they share, the
banded stage draws a specification's bands with or without a measurement, and a scalogram is a
surface with its cone of influence as two walls.

**`matrix`** (one value per row and column, such as a MAC)

```
{"view":"matrix","title":"...",
 "rows":{"label":"Identified","ticks":["439.2 Hz","647.2 Hz",...]},     one tick per row
 "columns":{"label":"FE model","ticks":["439.2 Hz",...]},              one tick per column
 "values":{"$array":.. f32 [R,C]},          row r, column c
 "value_label":"MAC",
 "value_range":[0, 1]}                      the color scale, pinned
```

The shell draws one cell per value, colored on the sequential colormap over `value_range` with a
numeric legend, the first row at the top and the first column at the left, and writes each value
in its cell when the cells are large enough to hold it. Hovering a cell reads its row, column and
value. A pinned range makes two matrices of the same quantity read alike.

A matrix that compares two sets of shapes (a MAC) may come with the `shape3d` view of those shapes
in the same result, and then says `"selects":"shape3d"`. That view's `modes` are the rows' shapes
and its `overlays` the columns', and `overlay_phase` (f32 [R,C]) is the angle that aligns column
c's shape with row r's, since a real mode is defined only up to its sign and a complex one up to a
rotation. Clicking cell (r, c) shows mode r with overlay c drawn over it, rotated by that angle and
in one flat color, scaled to the same amplitude; the playback and the camera carry on through a
click. Visual Dynamics draws a MAC pair the same way.

**`table`** (rows of text under column headings, such as a mode table)

```
{"view":"table","title":"...",
 "columns":["Mode","Frequency [Hz]","Damping [%]","Description"],
 "rows":[["1","439.2000","1.250",""],...]}          one cell per column, as text
```

Every cell is text, written the way the value is usually quoted (the service formats it). The
shell draws the table with its headings held at the top as it scrolls, puts a cell that reads as
a number at the right of its column, and copies the whole table as tab-separated text, the
headings first, so it pastes into a spreadsheet.

**`image`** (a photo of the test)

```
{"view":"image","title":"setup","format":"png"|"jpeg","bytes":{"$array":.. u8 [N]}}
```

The file's bytes as they were read, never re-encoded, so the report and the view show the same
picture; the shell draws it at its own aspect under its name. A Photos object selected in the tree
shows one `image` a photo, the picked ones when some are picked, stacked as Visual Dynamics stacks
them.

A `shape3d` built from a geometry carries `node_ids`, the drawn nodes' ids (u32, one per node), so
the view can caption the node under the pointer, coincident nodes listed together, as Visual
Dynamics' scene does.

A table may carry `"edit":{"op","params","columns","choices"?}`: its cells are typed into in place. `columns`
gives each column's key, or null for a column that is read only. A cell typed into runs `op` (an
operation, by `op.run`) with `params` and `row` (from 1), `column` (the key) and `value` (the text),
and the first `table` view of the answer takes the table's place; a value the operation refuses
leaves the cell as it was and the refusal is shown. Tab-separated text pasted into a cell sets
the block it covers, cell by cell, as Visual Dynamics' tables paste.

## Operations: Declarative Panels

The analysis service describes each operation; the shell builds the panel from the description.
A feature that fits this pattern needs no TypeScript.

```
{"id":"project.fit_modes","title":"Fit Modes","workspace":"modal","summary":"one sentence",
 "params":[{"name":"frf","label":"FRF","type":"enum","options_from":"verb:fit_modes"},
           {"name":"low_hz","label":"From","type":"number","unit":"Hz","default":0,"min":0},
           {"name":"high_hz","label":"To","type":"number","unit":"Hz","default":1000,"min":0},
           {"name":"count","label":"Modes, at most","type":"integer","default":10,"min":1,"max":60}],
 "outputs":["shape3d","reading"]}
```

Param `type`: `number`, `integer`, `enum`, `boolean`, `string`.

An `enum` that names one of the project's objects sets `"options_from":"objects:<kind>"`, or
`"objects:<kind>|<kind>"` for more than one kind; the shell fills it with the names of the open
project's objects of those kinds, in the project's order. Left out, it takes the first of them.
An operation run when the project has none of the kinds a required param takes is refused, and
the refusal names the kind.

An `objects:` or `verb:` enum whose `default` is `""` is optional: `""` names no object, the shell
offers it first as "(none)", and the tool is shown even when the project has no such object.

An `enum` that names an object a core verb takes sets `"options_from":"verb:<verb>"`, such as
`verb:integrate`; the shell fills it with the objects whose `verbs` (in the project summary) list
that verb. Which verb applies to what is the core's own table (`Project.verbs`): Integrate takes a
time history whose channels are accelerations or velocities, Compute FRFs one whose channels have
both a response and a reference side, and so on, which no list of kinds can say. One more is
listed beside the core's: `overload_check`, for a time history a linked channel table describes,
which the Overload reading reads against that table's ranges. Left out, it
takes the first of them, and an operation run when no object takes the verb is refused.

A param with `"setting":"<group>.<field>"` opens on that setting of the object the operation's
`source` param names, from the object's `settings` in the project summary, such as
`averaging.frame_length`; when the object has no such setting, it opens on its default. The shell
fills it again each time the source changes, so the form shows what the object holds before it is
changed.

A param with `"in_view":true` is not shown in the panel: the result's view sets it. The shell runs
the operation again with the panel's values and the view's choice, and shows the new result in
place of the old. A waterfall's `quantity` is one: the view lists the quantities it can draw and
the person picks one there.

`workspace` is the module the operation belongs to, the rail entry that offers it: `acquire`,
`simulate`, `signals` (recordings and what is read straight off them), `random`, `shock`, `sine`,
`modal`, `geometry`, `model`, `specify` or `report`. Visual Dynamics put every analysis under one
window and typed the project instead; Hertzforge splits the analysis by what is studied, so a sine
test is not offered shock tools. The modules share one project, one tree and one way of showing a
selection; they differ in which operations their action table offers. An operation's `modules`
lists the modules that offer it: its own `workspace`, or more when several studies need it (Export
in all of them, Averaging in Signals, Random and Modal).
Every operation runs on the connection's own project, and `op.run` is refused while no project
is open. Its result carries the project's summary beside the views, `{"views":[...],"project"}`,
and the project counts as modified when the operation added an object. The operations are the
core's verbs (Waterfall, Compute PSDs, Compute FRFs, Fit Modes, Mode Shapes and the rest); the
shell shows a tool only when the project holds something it runs on ("tools follow the data"). A
view drawn by a verb that adds an object offers no in-view choice, since choosing would run the
verb again. Until 2026-11-05 the service also offered a catalog of built-in datasets (the plate
runs and three finite element models) with operations of their own; those are now example
projects downloaded from hertzforge.com and opened like any other file.

## Methods

**Analysis service**

| Method | Params | Result |
|---|---|---|
| `catalog.list` | `{}` | `{"operations":[descriptor...]}` |
| `op.run` | `{"op","params":{...}}` | `{"views":[view...],"project"}` |
| `project.new` | `{"title"?}` | `{"project"}` |
| `project.open` | `{"path"}` | `{"project"}` |
| `project.import` | `{"path","units"?}` | `{"added":[name...],"project"}` |
| `project.save` | `{"path"?,"compress"?}` | `{"path","project"}` |
| `report.*` | see Reports below | |
| `project.rename` | `{"name","to","index"?}` | `{"project"}` |
| `project.remove` | `{"names"}` | `{"project"}` |
| `project.delete_parts` | `{"parts":{name:[index,...]}}` | `{"project"}` |
| `project.activate` | `{"geometry"}` | `{"project"}` |
| `project.duplicate` | `{"names"}` | `{"project"}` |
| `project.refresh` | `{"names"}` | `{"project"}` |
| `project.units` | `{"name"}` | `{"name","tables":[{"side","rows":[...]}]}` |
| `project.children` | `{"name"}` | what the object holds (below) |
| `project.show` | `{"objects","records"?,"part"?,"prefs"?}` | `{"title","views","note","offers"}` |
| `project.actions` | `{"objects","records"?,"part"?}` | `{"table"}` |
| `project.journal` | `{}` | `{"lines"}` |
| `project.define_units` | `{"name","units":[{"side","key","unit"}]}` | `{"converted":[record...],"project"}` |
| `units.system` | `{"name"?}` | `{"name","systems"}` |
| `fit.*` | see Fitting modes below | |

**Gizmos.** A `shape3d` with `gizmo` draws arrows and rings at `origin`, along the three `axes`.
Letting go of one runs `slide` or `turn`: its `op` with its `params`, the amount in meters or degrees
in the param `values` names for that axis, and what `adds` (optional) gives for that axis.

**Previews.** An operation with `previews` can be shown while its form is typed: `op.run` with
`"preview": true` runs it on a copy of its geometry and answers `{"views"}`, with no `project`.
Nothing reaches the project or its journal. A reading's `Added` row says `Would add`.

**The console.** `project.journal` gives the session as Python: every verb, settings change and
import since the project was made or opened, as the line a script would make (`Project.journal`),
the first line making the project. The shell shows it read-only along the bottom of the window, as
Visual Dynamics' console does, to be copied into a script; nothing is typed into it.

**The tree.** Each object in the summary carries `holds`, a few words on what it contains, and
`banded` when a PSD or a specification is on proportional bands, which changes its icon.
`project.children` answers what one object opens into, as Visual Dynamics' tree opens it, by
`kind`: a `geometry` lists its `parts` (nodes and coordinate systems with counts), its element
`families` (every family, each with the element `groups` holding elements of it and their counts),
its `solids` (a boolean's operands as its `children`) and its `mates`; a data object is a `grid`,
`rows` of a DOF and a quantity (a time history's with a `role`), `columns` of what tells one record
from another (reference DOFs, capture labels, or one blank), `cells` of record indices (-1 for a
hole), one icon key per record in `icons` (a quantity, two as `response/reference`, or empty for
the object's own icon) and the records whose units are `undefined`; a shape set lists its `modes`;
a channel table its `channels`; photographs their names; matched modes their `matches`; anything
else is `none`. The answer describes the object as it stands and is asked again when the project
changes.

**Links and what fits.** The summary's `links` are the project's object groups as Visual Dynamics
brackets them down the tree: each group's `members`, its `role` (`Basis`, the group comparisons are
made against, or null) and its `name` (blank when it has none). An object that does not fit the
geometry it answers to (its group's, else the active one), or a modal object with no shape set to
answer to, carries `issue`: the `message` to show, and the `sub_items` (record, mode or channel
indices) at fault, for the grid to mark. Incompatibility warns and never blocks, so a linked object
with an issue is still linked. `project.link`, `project.unlink`, `project.set_basis`,
`project.place`, `project.set_role` and `project.name_object_group` are operations (run by `op.run`)
and answer with the project, as every act does.

**The selection shows.** `project.show` answers what a selection in the tree shows, as Visual
Dynamics' window shows it: `objects` are the selected objects' names, `records` maps an object's name
to the record indices picked in its grid (a mode's or a channel's index for a shape set or a channel
table), and `part` names a part of a geometry picked (`{"kind","id"}`: a family, an element group, a
solid, a mate) or is null. One object shows by its kind: a geometry draws; a data object is its
waterfall, or the records picked as curves; a shape set is its modes on the geometry of its group (or
the active one) with its mode table, or the table and its MAC against itself without one; a channel
table is its table. Two together show what the pair means: data on a geometry animates it (a shape set,
a time history played back, an FRF's or a spectrum's operating deflection shape at its loudest line, a
PSD's envelope), a channel table on a geometry marks its channels, two shape sets give their MAC, a
measurement beside its requirement gives the comparison's spectra, two PSDs overlay. A selection with
no joint reading shows each object on its own, and `note` says what was left out. The views come in the
connection's display units, as an operation's do. `prefs` is how the window reads data, kept across
selections as Visual Dynamics keeps its bar: `flat` (the plot in place of the stage for a whole data
object), `log_frequency` (decades on every frequency axis) and `component` (`magnitude`, `real`, `imag`
or `phase` of complex data) and `line`, the frequency in Hz the stage's operating shape or envelope is
read at (null for the loudest line); `offers` says which of them the selection can take (`flat` for a
data object, `complex` for complex frequency data, `frequency` for anything on a frequency axis). A
`shape3d` that reads at a line carries `cursor: "frequency"`, and its plot carries `link: "frequency"`,
so the plot's cursor moves the line. A `waterfall` may say its opening `facing` (a coherence opens on
the map, `top`). Several data objects of one type show on one stage, merged for the drawing; a CPSD
on a geometry shows its principal shape, or the shape relative to one reference when one column of its
grid is picked. Two shape sets (with or without a geometry in the selection) show their MAC and, on a
geometry, the stage the matrix `selects` a pair on; a `matrix` may carry `matched`, the `[row, column]`
pairs already kept in a Matched Modes object for the two sets. `prefs.projection` (offered as
`projection` when the two sets are on different geometries) animates the second set projected onto the
first's geometry; `project.add_matches` keeps picked squares as matched modes. A lone shape set shows
one view at a time, named by `prefs.shape_view` (`offers.shapes`): `animate` (its modes on its geometry,
and the default), `table`, `mac` (with the stage its squares animate) or `values`, the
values read at `prefs.values_dofs` (`all` or a channel choice of the table in its group, listed in
`offers.values_dofs`) in `prefs.values_form` (`shapes`, `response`, `reference`). An FRF beside a shape
set shows the picked records with their resynthesis following dashed (`follows`). `prefs.arrows` and
`prefs.sensors` (offered as `arrows` and `sensors`) put DOF arrows and the channel table's sensors over
every stage as `marks` on the `shape3d`: static line work, `nodes` f32 [M, 3] in the geometry's
coordinates and `lines` u32 [L, 2] indexing them. A lone channel table shows typed into, its cells
edited with the widgets `edit.kinds` names (a flag by a check box, a choice from its list, a date in a
date field, a unit from a shortlist); `prefs.environment` picks the loop whose control channels the
Control column shows (`offers.environments`) and `prefs.table_view` the table or one of that loop's
transformations (`offers.table_views`). `project.save_as` takes `units`: `display` for the connection's
display unit system (the server puts its name in), `si` for the stored values.

**Export's formats.** The Export row carries `formats`, each `{"value","label","suffix"}`: what that
object can be written in, `.hforge` first. The desktop app lists them as the save window's file types.

**The actions a selection takes.** `project.actions`, with the same `objects` and `records`, answers
for the action table beside the project tree, as `{"table":{"columns":[...]}}`. An operation is offered
when the selected objects fill its object params: each object on a param that takes its kind (or whose
verb applies to it), every object used and every required object param filled. With nothing selected, the first column is the project: the acts that take no object
and the project's files. `fill` is the params to set from the selection (and `records`, as `'1-3, 7'`,
when records are picked and the operation takes them).

The first column has `"current": true` and is the selection; each other column is the selection with one
more object of the last type in its `kinds`, `"*"` meaning any object. A row is
`{"id","verb","kind","ready","reason"?,"modules"?,"fill"?,"candidates"?,"part"?}`. `kind` is `default`
for what the selection draws by itself (`id` starting `show.`), `run` for an operation (`id` is the
operation's) or another of a lone shape set's views (`id` starting `show.`), and `self` for an act on the selection that is not an operation (`id` starting
`self.`). A row that cannot run has `ready` false and a `reason` to show, including one whose operation
would refuse what the selection holds, as far as that can be told with nothing typed. A row without `modules` is
offered in every module. In a combination column, `candidates` names the project's objects that
complete the row, and a row with none is not ready. `part`, the part of a geometry picked in the tree
as `project.show` takes it, adds the operations on that part, which carry `"part": true`.

**Display units.** Every view the analysis service sends is drawn in its connection's unit
system, as Visual Dynamics draws its window in the system its selector names. A connection starts
in Visual Dynamics' default (`in-slinch-lbf-s (g)`: inches, pound-force, accelerations in g);
`units.system` with `name` (one of `systems`) changes it, and without one says which it is. Each
operation computes in SI; the service converts what it sends: a `spectrum`'s axes, series, step
edges and regions, a `reading`'s rows, a `waterfall`'s axes and values, the fit's CMIF, and the
report's page, and a `table`'s columns headed "<name> [<SI unit>]" (each number in them). A column
typed into (`edit`) stays in SI, so a value typed means what its operation reads. A unit it does
not recognize and a `matrix` are sent as they are. A `shape3d`'s coordinates are put in the system's length unit (its `unit` says which) and a measured motion's values in theirs; its shapes stay as they are, and a `point` pick carries `to_si` [scale, offset], what each coordinate clicked is taken back to SI by (SI = value × scale + offset) before it is sent. A view already on screen is not converted again; the shell draws it again.

**Projects.** Each connection has at most one project of its own, as a Visual Dynamics window
has one, and it ends with the connection. A file reaches the service by its path on the service's
machine; nothing is uploaded. `project.open` replaces the project with the one a `.hforge`, `.vdyn`,
`.mat` or ESCDF `.h5` (`.hdf5`, `.escdf`) file holds, and refuses any other file with a pointer to
import it. `project.import` adds a file's objects to the project, making an empty one first when
none is open. `units` declares what the file does not state, as the core's `import_file` options
(`length_unit`, `ordinate_unit` and the rest). `project.save` writes to `path`, or to the file the
project came from; a `.vdyn` is opened but never written. `project.new` takes an optional `title`
and defaults it to "Project". `project.rename` gives the object `name` the name `to`, and every
group, provenance record and report block that names it follows; a name in use is refused.
With `index`, `project.rename` renames one photo of a Photos object instead (the photo at that
index, from 0), and the report blocks that show it follow. `project.remove` takes `names`, a list,
and removes them all or, when one is not in the project, none. `project.delete_parts` deletes
sub-items picked in the tree, `parts` mapping an object's name to the indices picked: records of a
data object, modes of a shape set, channels of a channel table, photos of a Photos object, as
Visual Dynamics' Delete does on grid picks. Taking every item is refused ("delete the object
instead"), and the journal records the call a script would make. `project.activate` makes `geometry` the active geometry, the one data is drawn on when
nothing else names one. `project.duplicate` adds a copy of each object in `names` beside it, named
"<name> copy", in no group. `project.refresh` computes each derived object in `names` again from
its source, in place and under its own name; an empty list refreshes every stale object, sources
first. Each answers with the project. A `.png`, `.jpg` or `.jpeg` path is a photo: it is added to the project's Photos object,
made on first use, and `added` names that object.

Units a file did not state are declared by channel, not by record, as Visual Dynamics' Imported
Units pane declares them. `project.units` gives an object's tables: a geometry's one row (`side`
`object`, key `length`), a shape set's (key `mass`), and a data object's channels, one row per
(DOF, quantity), since a drive point's accelerometer and load cell share a DOF and are two
channels. An FRF has a `response` and a `reference` table; a cross-spectral matrix and every other
data object one `channels` table. Each row has `key`, `label` (the DOF), `quantity` (what the
channel measures, from its dimension or the file's claim, or null), `unit` (null while undeclared)
and `choices`, the units offered first; any unit Hertzforge reads is accepted. `project.define_units`
declares units by `side` and `key`. Every unit is read before anything changes. A record converts
when each of its channels is named, here or before, and one only half named waits; `converted`
lists the records converted. Project requests on one connection run one
at a time, in the order they arrive.

The project as the shell lists it:

```
{"title":"Bracket Survey","file":"/Users/.../survey.hforge"|null,"modified":true,
 "objects":[{"name":"FRF","kind":"frf","records":22,"source":"SIM",
             "units_defined":true,"groups":["Group 1"],"verbs":["fit_modes"]},
            {"name":"Geometry","kind":"geometry","units_defined":false,"groups":[],
             "verbs":["generate_rigid_body_modes","add_solid_block"]}]}
```

`kind` is the object's class name in lower case with underscores (`time_history`, `psd`, `frf`,
`multiple_coherence`, `shape_set`, `geometry`, `channel_table`, `specification` and so on).
`records` is present for data objects, `source` for measured data, and `units_defined` is false
for an object imported without its units, which the shell marks as Visual Dynamics does. `groups`
names the object groups it belongs to, by role (`Basis`), name, or `Group <n>` in the project's
order. `active_geometry` is the active geometry's name, or null. `unresolved`, present only when
there are any, lists the objects the project holds by reference to another `.hforge` file that
was `not found` or has `changed` since: each with its `name`, `kind` and the `path` it was looked
for at. They are not loaded and no tool lists them; saving keeps their references, and
`project.remove` drops one. `stale` (present only when it is) says why a derived object no longer
matches its source: the source's settings changed after it was computed. `verbs` are the core's
processing verbs that apply to the object, in the core's order, then `overload_check` where a
linked channel table describes a time history; `verb:` params are filled from them. `settings` is
a time history's analysis settings, each group
present only when set: `averaging` (`frame_length` in samples, `overlap` as a fraction, `window`,
`frames`, `start` in seconds, `detrend`), `filtering` (`kind`, `low` and `high` in Hz as the kind
has them, `order`) and `truncation` (`start`, `stop` in seconds). A geometry's are `rigid`: the
reference point `x`, `y`, `z` in meters, `normalize`, and with it `mass` and the inertia terms
`ixx`, `iyy`, `izz`, `ixy`, `ixz`, `iyz`. A report's `settings` are
`features` (each report feature's key, true where it is on) and `identity` (each identification
field as text, people authorized comma-separated). `modified` is true after an import or an operation that added,
removed or changed an object, and false after an open or a save.

A reader that can count reports its progress while an import runs, as event topic
`project.progress`, `{"done","total","path"}`, at most ten a second and always the last one. A
file the core cannot read is refused in the core's own words (`refused`, or `not_found` for a path
with no file).

**Fitting modes.** One mode at a time, on the residual, as Visual Dynamics' fitting screen fits
them; the session is the core's `ModalFitSession`. Each connection has at most one fit, on its
project, and opening or starting another project ends it.

| Method | Params | Result |
|---|---|---|
| `fit.start` | `{"frf","shapes"?,"coherence"?,"records"?}` | `{"fit"}` |
| `fit.find` | `{"low"?,"high"?}` | `{"fit"}` |
| `fit.move` | `{"frequency"}` | `{"fit"}` |
| `fit.damping` | `{"damping"}` | `{"fit"}` |
| `fit.fit` | `{}` | `{"fit"}` |
| `fit.confirm` | `{"description"?}` | `{"fit"}` |
| `fit.refine` | `{}` | `{"fit"}` |
| `fit.delete` | `{"modes"}` | `{"fit"}` |
| `fit.edit` | `{"mode","damping"?,"description"?}` | `{"fit"}` |
| `fit.residual` | `{"show"}` | `{"fit"}` |
| `fit.publish` | `{"name"?}` | `{"name","project","fit"}` |
| `fit.end` | `{}` | `{}` |

`fit.start` fits the FRF `frf`, on the `records` given (indices from 0) or all of them, or, with `shapes`, edits that shape set's fit (Edit Fit): its
modes are adopted as confirmed. A suggestion is made at once, so the pending mode already has a
frequency and damping. `fit.find` moves the pending mode to the most prominent peak left in the
residual CMIF between `low` and `high` in Hz, the frequencies on screen (the whole range when left
out): the zoom is the search band. `fit.move` puts the cursor at `frequency` and searches the
damping there; `fit.damping` holds the damping at a fraction of critical until the next suggestion.
`fit.fit` fits the pending mode without adding it (the preview), so its synthesis can be judged,
and `fit.confirm` adds it. `fit.refine` fits every confirmed mode's residues again, together.
`fit.delete` takes back the confirmed modes at `modes` (indices into `modes`, from 0) and drops
the preview. `fit.edit` types a confirmed mode's `damping` (a fraction of critical, above 0 and up
to 1) or `description` into its row, as Visual Dynamics' fit table does; a damping changes the
synthesis and the residual. `fit.residual` draws the residual's own CMIF in the view, or stops. With
`coherence`, the name of a coherence or multiple coherence in the project, `fit.start` weights each
response at each line by γ²/(1-γ²), as the core does (`ModalFitSession`).
`fit.publish` adds the confirmed modes to the project as a shape set named `name` (default
"<frf> Modes"), or replaces the set the fit was started from; later publishes replace the set
published. A step the core refuses is refused in its words; a fit whose FRF has left the project
ends.

`fit` is `{"frf","shapes","pending":{"frequency","damping","held"},"preview","modes":[...],"view"}`.
`shapes` is the set the fit edits or last published, or null. `preview` is null or the previewed
mode's `{"frequency","damping"}`; each of `modes` is `{"frequency","damping","description"}` in
frequency order, damping a fraction of critical. `view` is a `spectrum` view, frames carrying its
arrays as `op.run` does: x `Frequency` Hz, y `CMIF` log, `link` `frequency`, the measured CMIF's
first four singular values (`CMIF 1`...), the synthesis's (`CMIF 1, synthesized`...) each
`follows` its measured line, and the pending mode's curve (`Pending mode`, role `reference`).
`mac` is a `matrix` view of the confirmed modes' auto-MAC with the preview's row and column last
(`New: <f> Hz`), how alike the shapes are; null for fewer than two. `coherence` names the
coherence the fit is weighted by, or null; `residual` says whether the view draws the residual's
CMIF (`CMIF 1, residual`..., role `drive`). `half_power` is the half-power reading of the residual
CMIF peak under the cursor: `{"frequency","low","high","q","damping","notes"}`, each of `low`,
`high`, `q` and `damping` null where it could not be read, and `notes` saying why.

**Run log.** The log keeper's run log, as Visual Dynamics' Run Log window keeps it while a test
runs. A connection holds at most one log open; its lock is released when the log is closed or the
connection ends, and a log another writer holds opens read-only, saying why.

| Method | Params | Result |
|---|---|---|
| `runlog.new` | `{"path","title"?,"mirror"?}` | `{"log"}` |
| `runlog.open` | `{"path","mirror"?}` | `{"log"}` |
| `runlog.close` | `{}` | `{"log":null}` |
| `runlog.reload` | `{}` | `{"log"}` |
| `runlog.state` | `{}` | `{"log"}` (null with none open) |
| `runlog.form` | `{"form","axis"?,"target"?}` | `{form...,"taken","time","preview"}` |
| `runlog.preview` | `{"form","values","axis"?,"target"?}` | `{"text"}` |
| `runlog.write` | `{"form","values","text"?,"time"?,"taken","author","axis"?,"target"?}` | `{"log","said"}` |
| `runlog.export` | `{"path","view","test"?}` | `{"path"}` |

`log` is the window's view: its `path`, `title`, `writable` and `read_only` (why it cannot be
written), the `status` the strip reads (the current test, its axis, run, level and outcome, when
the run started and stopped), the day's `rows` (each entry's `id`, `kind`, `test`, `date`, `line`,
the `corrections` that changed it, and when it was entered if `late`), the `tests` with their
states, the `holds` and `owed` lists, the current test's `header`, the `run` a level row goes into,
the `level` it offers, and the `readings` forms the test's runs take.

A `form` is one of: `level row`, `abort`, `note`, `clock check`, `person in`, `person out`, `run
start`, `run stop`, `cause found`, `correct` (with the entry's id as `target`), `open test`,
`roll`, `close test`, `release`, `header`, `sweep`, `shot`, `result`, `notch`, `deviation`,
`anomaly`, `disposition`, `setting change`, `opinion` and `new log`. `runlog.form` describes it:
its `title`, its `fields` (each `field`, `label`, `box` kind, `choices` for a choice and the
`value` it opens with; a `heading` box titles a group), the `required` fields, the label of its
line of free `text` ('' for none), a `note`, whether it takes an event time (`with_time`) and
whether it computes something as it is typed (`live`). It is opened at `taken`, which `time` shows
as HH:MM:SS. Each box's text goes back as typed in `values`, by field, and the service reads it by
its box kind; `preview` is what a `live` form computes from it (the projection to full level, a
sweep's current against its rating, a shot's checks and pulses, a notch's place in the plan, the
hold a deviation answers), with the predictions drawn for the connection's project beside what is read: the notch Notch Prediction cut beside a notch, and the drives Drive Prediction asked for beside the level row. `runlog.write` writes it as its entries, the event time read from
`time` against `taken`, every entry checked before any is written; a refusal says why. `said` is
what the window says after (an abort's owed first minute, an unplanned notch's hold), or ''.

`runlog.export` writes one view of the open log to `path`, as Visual Dynamics' run-log dock exports it: `view` `text` (the log as it reads, every entry in the order written, corrections struck through and restated), `csv` (every entry as stored, corrections included, one row each) or `machine` (the trips and machine events for the lab's own machine log); `test` is a test number for that test alone, or `all` (the default) for the whole log. The view's extension is added when the path has none. The answer is the `path` written.

**Reports.** Visual Dynamics' report on the connection's project, as the core writes it.

| Method | Params | Result |
|---|---|---|
| `report.generate` | `{"template"?,"name"?}` | `{"name","project"}` |
| `report.render` | `{"name","theme"?}` | `{"name","html"}` |
| `report.export` | `{"name","path","anyway"?}` | `{"path","owed","index"}` |
| `report.editor` | `{"name","selected"?,"theme"?,"page"?}` | the editor (below) |
| `report.operate` | `{"name","op",...,"selected"?,"theme"?}` | the editor, and `"project"` |
| `report.print` | `{"name"}` | `{"html"}` |
| `report.issue` | `{"name","step","path"?,"anyway"?}` | `{"html","status"}`, `{"index"}`, `{"entry"}` or `{}`, and `"project"` |

`report.generate` builds a report from a starter template bound by type to the project's objects
(`Project.generate_report`): `template` is `modal` (the default), `random`, `shock`,
`transient`, `sine`, `sysid`, `mimo` or `empty`, and `name` defaults to "Report". `report.render`
gives the report `name` as one HTML document, with its stylesheet, data and viewers inline and
nothing loaded from a network, in the `light` or `dark` theme (the reader's own when left out); the
shell shows it in a sandboxed frame. `report.export` writes that page, light, to `path` (`.html`
added when it has another suffix), beside the target first and renamed over it. First it makes
the checks Visual Dynamics makes before a report leaves the project: with skeleton sections on,
the sections (a flash report's parts) the report still owes; with the data index on, every
indexed file hashed again and those that do not match the checksum indexed. A report that owes
or whose files have `CHANGED` or are `NOT FOUND` is not written unless `anyway` is true; the
answer then carries `owed` (the sections' lines) and `index` (`{name, status}` a file) with
`path` null, and the shell asks "Export anyway?". Written, the answer is the `path` with both
lists empty.

`report.editor` gives Visual Dynamics' report editor: `html`, the core's edit page (every block
tagged with its index, the block at `selected` framed, and unbound blocks shown as cards to bind),
and the pane for the selection. A click on a block posts `{"hforgeReport":{"op":"select","at":n}}`
(`at` null outside every block) to the page's parent, which the shell answers with
`report.editor` at that block and `"page":false`: the page has framed the block itself, so the
answer leaves `html` and `references` out and nothing is rendered again. `title`, `marking` and `marking_color` (`ink` or `red`) are the
report's; `blocks` how many it has; `insert` the Insert menu as `[kind, label]` pairs (Markdown,
then `core.report.insert_options`); `references` the `[label, caption]` of each numbered figure
and table, for a text block's `{{figure:<caption>}}` token. `block` is null with nothing selected,
or the selected block's `index`, `kind` and `mode`, and either its `text` (a text block; `special`
names a skeleton section, check or requirement block, which has a pane of its own) or its
`fields` (`{field,label,value,options:[[value,label]...]}`, the objects it may name, symbolic
bindings such as `@basis:ShapeSet` first) and `caption`.

`report.print` gives the reader's page as it is printed: light, its figures drawn at print
resolution (`#print`), and asking for the print dialog once they are drawn, where the reader saves a
PDF. Its print stylesheet carries the margins the identity rides in. An issued report that has
changed since is refused: a change after issue is an amendment. `report.issue` is Visual Dynamics'
Issue in three steps around that print. `begin` checks the report can be issued (Report Identity
on, every required field entered, not already issued), makes the data index check `report.export`
makes (answering `{"index":[...]}` alone, nothing marked, when files are astray and `anyway` is not
true), marks it issued (or amended), and gives the page to print and the `status`. `record` reads the PDF saved from it at `path` and records the
issue with the SHA-256 of the PDF and of the page; its `entry` is the record's new line. `cancel`
puts the report back as it was. The summary's `settings` for a report carry its `identity` fields
as text beside its `features`.

`report.operate` applies one of VD's editor operations and answers with the editor at the block it
leaves selected: `insert` (`kind`, one of `insert`'s, at `at` or the end, bound to the first
object the project has for it), `remove` (`at`), `move` (`from`, to before `to`), `text` (`at`,
`value`), `field` (`at`, `field` `caption` or one of the block's `fields`, `value`), `title`,
`marking` and `marking_color` (`value`).

**Control service**

The hardware and Random methods are in "Hardware and Random Control" below. The `sim.*` methods
came first and are superseded for running tests: `random.*` on the `sim` station runs the same
closed loop with the full configuration, controller ownership and the safe stop. They stay,
unchanged, for the Simulate workspace's demonstrations (they alone run faster than real time).

| Method | Params | Result |
|---|---|---|
| `sim.profiles` | `{}` | `{"profiles":[{"id","title","summary"}]}` |
| `sim.start` | `{"profile","level_db","speed"}` | `{"run_id"}` (`speed` 1 = real time, 0 = as fast as possible) |
| `sim.stop` | `{}` | `{"state"}` |
| `sim.state` | `{}` | `{"state":"idle"|"running"|"stopped","run_id"|null}` |

`speed` is a multiple of real time from 0 to 1000; 0 means as fast as the engine runs.

The service runs one test at a time. `sim.start` is refused with reason `run_active` while a
`random.*` run is live (armed, running, holding for review or stopping), and `random.arm` and
`random.start` are refused the same way while a `sim.start` run is running. `sim.stop` and `sim.state` see only runs that
`sim.start` started.

Event topic `random.display`, about 4 to 10 per second while running:
`{"run_id","source":"SIM","state","elapsed_s","level_db","views":[spectrum, reading],
"phase","outcome","reason"}`. `phase` is the controller's own phase name. Once `state` is
`stopped`, `outcome` is one of `completed`, `operator_stop`, `aborted`, `held`, `fault`,
`law_fault`, and `reason` says why in words; the shell shows any outcome other than `completed` or
`operator_stop` as an abort with that reason.
`source` is always present on control data; the shell shows it as a badge. A simulated run is
never presented as a hardware result.

## Hardware and Random Control

The control service runs one test at a time on one station. The methods below select the
station (the simulator or NI-DAQmx hardware), edit the channel table, configure a closed-loop
Random test, run it, and show it live. The control loop runs on the service's own thread beside
the hardware. The socket carries the operator's intent down and display data back, and never a
sample of the loop.

A stop or an abort sent over the socket is a request. The controller's own protection (the
envelope, kinematic, drive and divergence aborts, and the station's watchdog) and the
amplifier's emergency stop are the abort authority, and the shell labels its Stop and Abort
accordingly.

Every value from a run carries `source`: `SIM` or `HARDWARE`. Only physical NI devices give
`HARDWARE`. The simulator and NI's driver-simulated devices both give `SIM`, and `class` says
which. The shell shows `HARDWARE` as its live badge and `SIM` as the simulated badge, and names
driver-simulated devices as such. Nothing simulated is ever presented as a measurement.

### Refusals

Every error from a method in this section carries `reason`, a stable name a client branches on.
`message` is the words a person can act on and is shown verbatim; a client never parses it. It is
plain words: it never names a protocol method.
`detail`, when present, carries the facts named below.

```
{"type":"response","id":12,"error":{"code":"refused","reason":"controller_held",
 "message":"Another client controls this service (Shell on bench-pc). Close that client, or wait for it to release control.",
 "detail":{"holder":"Shell on bench-pc"}}}
```

| `reason` | `code` | Cause | `detail` |
|---|---|---|---|
| `bad_params` | `bad_request` | A parameter is missing, of the wrong type, or not one of its stated values | `param` |
| `not_controller` | `refused` | A method that changes the station, the table or the run (other than `random.stop` and `random.abort`), from a connection that does not hold control | `holder` (null when nobody does) |
| `controller_held` | `refused` | `session.claim` while another connected client holds control | `holder` |
| `run_active` | `refused` | The call would change what a live run (armed, running, holding for review or stopping) depends on, or start a second run | `run_id` |
| `not_configured` | `refused` | `random.arm` or `random.start` with no configuration | |
| `run_finished` | `refused` | `random.arm` or `random.start` on a finished run; reset or configure first | `outcome` |
| `wrong_state` | `refused` | The verb does not apply in the run's present state | `state`, `phase` |
| `manual_mode_required` | `refused` | `random.level` while the ladder is automatic | `mode` |
| `not_a_rung` | `bad_request` | `random.level` `to_db` is not a level of the schedule | `levels_db` |
| `feasibility_exceeded` | `refused` | The profile exceeds the shaker's ratings; the run is blocked until armed with an acknowledgement | `feasibility` |
| `override_not_needed` | `refused` | `override_ack` sent while the run is not blocked | |
| `arm_refused` | `refused` | The station refused to arm: the rate it programmed, the analysis grid at that rate, its sample clock, or the driver. The message is the controller's own | |
| `ni_driver_unavailable` | `refused` | The NI-DAQmx runtime is not installed on this computer | |
| `no_ni_devices` | `refused` | The scan found no NI devices | |
| `scan_failed` | `refused` | The driver refused the scan | |
| `devices_absent` | `refused` | Selected devices are not in the latest scan | `missing`, `found` |
| `mixed_simulated_and_real` | `refused` | The selection mixes driver-simulated and physical devices | `simulated`, `real` |
| `mixed_timing_families` | `refused` | The selection spans devices that cannot share one sample clock | `families` |
| `not_enough_channels` | `refused` | The selected devices have fewer channels than the table uses | `want_inputs`, `want_outputs`, `have_inputs`, `have_outputs` |
| `hardware_busy` | `refused` | Another program holds the NI hardware | `holder` |
| `stale_revision` | `refused` | `channels.set` quoted a revision that is no longer current | `current`, `provided` |
| `table_invalid` | `bad_request` | The channel table has errors | `errors`, `warnings` |
| `config_invalid` | `bad_request` | The Random configuration has errors | `errors`, `warnings` |
| `shaker_record_invalid` | `bad_request` | The feasibility shaker record does not parse | |
| `shutting_down` | `refused` | The control service is shutting down (see "Shutting Down"): `random.arm`, `random.start`, `random.proceed`, `random.level`, `random.mode` and `sim.start` are refused | |

`errors` and `warnings` are lists of items, `{"reason","message"}` plus whichever of `channel`
(a table row's `index`), `row` (a breakpoint row, from 0) and `field` (a configuration field
path such as `control.damping`) locates the problem. Item reasons:

| Where | Errors | Warnings |
|---|---|---|
| Channel table | `duplicate_index`, `no_channel_enabled`, `zero_sensitivity`, `non_positive_range`, `control_group_gap`, `reference_without_control`, `unsupported_schema`, and on an NI station `channel_unbound`, `physical_channel_unknown`, `physical_channel_wrong_direction`, `physical_channel_shared`, `mixed_timing_families` | `iepe_without_bias`, `bias_on_single_ended`, `reference_count_mismatch`, `drive_channel_present` |
| Profile | `profile_empty`, `first_row_not_anchor`, `row_level_ambiguous`, `frequency_not_positive`, `frequency_not_ascending`, `psd_negative` | `alarm_wider_than_abort` |
| Acquisition | `n_pts_not_power_of_two`, `band_above_grid`, `too_few_lines` | |
| Schedule | `levels_empty`, `levels_not_ascending`, `dwell_missing` | |
| Control and limits | `no_control_channel`, `weight_unknown_channel`, `weights_sum_not_positive`, `limit_unknown_channel`, `limit_rows_not_ascending` | |
| Any number | `out_of_range` (`field` names it, `message` states the range) | |

A refusal not listed here is a defect in the service.

### Controller and Observers

One connection at a time holds control; any number observe. Every connection starts as an
observer. Observers receive every event and may call every method that only reads
(`session.state`, `station.list`, `channels.get`, `random.state`, `random.config`, `random.log`,
`random.subscribe`, and the `dry_run` checks).

**Stopping is open to everyone.** `random.stop` and `random.abort` are accepted from any
connection, controller or observer. They only ever end a run, so a second client using them
cannot leave the run in a state nobody asked for, and whoever is watching a test that goes wrong
can end it from the screen in front of them. `random.stop` is the gentler of the two, so whoever
may abort may also stop. The control service listens on loopback only, so every client is on
the computer that runs the test. Every other method in this section changes what runs or how,
and is refused `not_controller` from an observer: one client at a time decides what the test
does. The run log and `end.message` name the client that stopped or aborted the run.

| Method | Params | Result |
|---|---|---|
| `session.state` | `{}` | `{"role","controller","orphan_s","heartbeat_s","timeout_s"}` |
| `session.claim` | `{"name","lease"}` (both optional) | `{"role":"controller","lease","resumed","heartbeat_s","timeout_s"}` |
| `session.release` | `{}` | `{"role":"observer","orphan_s"}` |

`role` is `controller` or `observer`. `controller` is `{"name","present"}`, or null when nobody
holds control. `name` is how other clients see the holder; it defaults to the peer address.
`orphan_s` is null, or the seconds left before an unattended run is stopped (below). A claim
while another connected client holds control is refused `controller_held`; there is no forced
takeover.

**Heartbeat.** The service sends a WebSocket ping every `heartbeat_s` (5 s). Any inbound frame
shows a connection is alive, and browsers answer pings on their own, so a client sends nothing
for it. After three heartbeats with nothing inbound, the connection is treated as gone.

**When the controlling client goes.**

- **Without a goodbye** (the network drops, the computer sleeps): the run continues, and the
  seat is kept for `timeout_s` (30 s) with `controller.present` false and `orphan_s` counting
  down. A `session.claim` that presents the same `lease` in that time resumes control
  (`resumed: true`); a claim without it takes control from the absent holder.
- **With a goodbye** (a clean close, or `session.release`): control is free at once, and a live
  run is kept for `timeout_s` for a client to claim.
- **Nobody claims in time:** if a run is live when `orphan_s` reaches 0, the service ends it
  with the controlled stop, the same path as `random.stop` (while driving, the level ramps down
  over the configured `shutdown_ramp_s`), never with an abort. The run's outcome is
  `safe_stop`.

`session.changed` (text event, to every connection on every change):
`{"role","controller","orphan_s"}`. `role` is the receiving connection's own (`controller` or
`observer`), so each connection learns from the event whether it holds control; every connection
receives its own copy.

### Stations

| Method | Params | Result |
|---|---|---|
| `station.list` | `{"rescan"}` (optional, default false) | `{"stations":[sim, ni],"selected"}` |
| `station.select` | `{"kind":"sim"}` or `{"kind":"ni","devices":[id...]}` | `{"selected"}` |

The service starts with the simulator selected. It never selects hardware on its own, and never
falls back to the simulator when hardware is missing, busy or refused: the selection is refused
by name instead.

```
{"kind":"sim","title":"Simulator","available":true,"class":"sim","inputs":8,"outputs":1,"clock":"shared"}
{"kind":"ni","title":"NI-DAQmx","available":true,"unavailable":null,"scanned_unix":1793577600,
 "devices":[{"id":"cDAQ1Mod1","product":"NI 9234","serial":"01A2B3C4","simulated":false,
             "parent":"cDAQ1","slot":1,"timing_family":"cDAQ1",
             "inputs":[{"id":"ai0","label":"ai0"}],"outputs":[]}]}
```

- The simulator is a fixed plant with two resonances (near 200 and 550 Hz) on one shared sample
  clock, paced in real time at the configured rate.
- `available` false on `ni` comes with `unavailable: {"reason","message"}`, using the station
  reasons above (`ni_driver_unavailable`, `no_ni_devices`, `scan_failed`).
- `simulated` is the driver's own statement that no physical device is behind the id. It is
  never inferred from a name.
- A chassis is listed as a device without channels; its modules name it as `parent` with their
  `slot`.
- `timing_family` groups the devices that can share one sample clock: every module in one
  chassis is one family (named by the chassis), and a standalone device is its own family. All
  of a run's channels must be in one family.
- The scan does not read a module's sample-rate ladder, IEPE capability or output range, so none
  is reported. The rate the hardware actually runs at is reported when the run arms. A known
  limit: a client cannot tell from the scan whether a module supports IEPE; a row set to `iepe`
  on a module without it is refused by the driver when the run arms (`arm_refused`).

`selected`:

```
{"kind":"ni","devices":["cDAQ1Mod1","cDAQ1Mod2"],"class":"ni_real","source":"HARDWARE",
 "inputs":4,"outputs":4,"timing_family":"cDAQ1"}
```

`class` is `sim` (the simulator's plant), `ni_simulated` (driver-simulated NI devices: the real
driver and tasks, no physical signal) or `ni_real` (physical NI devices). `source` follows from
it. `devices` is empty and `timing_family` null for the simulator.

`station.select` checks the selection against a fresh scan. Refusals: `bad_params` (an unknown
kind, or `ni` with no devices), `not_controller`, `run_active`, `ni_driver_unavailable`,
`no_ni_devices`, `scan_failed`, `devices_absent`, `mixed_simulated_and_real`,
`mixed_timing_families`, `hardware_busy`. Selecting opens nothing: devices are opened when a run
arms. `station.changed` (text event, to every connection when the selection or the scan
changes) carries the `station.list` result.

### Channel Table

One table, held by the control service, used by every run.

| Method | Params | Result |
|---|---|---|
| `channels.get` | `{}` | `{"revision","channels":[row...],"report"}` |
| `channels.set` | `{"revision","channels":[row...],"dry_run"}` (`dry_run` optional) | `{"revision","report"}` |

```
{"index":1,"enabled":true,"description":"Control","role":"control",
 "physical":{"device":"cDAQ1Mod1","channel":"ai0"},"interface":"single_ended","iepe_bias_v":null,
 "units":"g","sensitivity_mv_per_eu":100,"range_max":50,"polarity":"positive",
 "control_group":null,"abort":{"accel_g":null,"velocity_ips":null,"displacement_mil":null}}
```

| Field | Values | New row |
|---|---|---|
| `index` | integer, unique in the table; the channel's number everywhere else | |
| `enabled` | boolean | true |
| `description` | text | empty |
| `role` | `control` (closes the loop), `monitor`, `drive` (an output), `reference` | `monitor` |
| `physical` | `{"device","channel"}` from the scan, or null (unbound) | null |
| `interface` | `single_ended` or `iepe` | `single_ended` |
| `iepe_bias_v` | number, or null | null |
| `units` | `g`, `in/s`, `mil`, `lbf`, `V`, or another unit's name | `g` |
| `sensitivity_mv_per_eu` | number, mV per engineering unit; not zero on an enabled input | 100 |
| `range_max` | number above 0, full scale in the row's unit | 100 |
| `polarity` | `positive` or `negative` | `positive` |
| `control_group` | integer from 0 with no gaps, or null | null |
| `abort` | `{"accel_g","velocity_ips","displacement_mil"}`, each a limit or null | all null |

Every row carries every field. A row's `abort` limits are absolute: a run aborts when that
channel passes one, at any level.

**IEPE is off unless a client turns it on for a row.** The service's starting table (one
control input, one drive output, both unbound) and every row it creates are `single_ended`, and
the service never turns IEPE on by itself. IEPE drives a constant current into the input: an
input wired to a voltage source, such as a loopback from an output or a signal generator, stays
`single_ended`.

**Validation.** `report` is `{"errors":[item...],"warnings":[item...]}` with the item reasons
listed under Refusals. On the simulator, `physical` is kept but not used or checked. On an NI
station, every enabled row must be bound to a channel of the selected devices in the right
direction (an input row to an input, a drive row to an output), no two rows to one channel, and
all in one timing family. Selecting another station checks the table again: `channels.get`
reports the errors until the table is fixed, and `random.configure` is refused `table_invalid`.

**Revisions.** Every write increments `revision`. A `channels.set` that quotes an older revision
is refused `stale_revision`; the client reads the table again and shows the operator what
changed before sending, and never retries on its own, because the table carries the abort
limits. A table with errors is refused `table_invalid` and not written; warnings never block.
`dry_run: true` checks the table and returns the report without writing; it needs no control and
is never refused for the table's content. `channels.changed` (text event, to every connection
after a write or a station change) carries the `channels.get` result.

### Random Test Configuration

| Method | Params | Result |
|---|---|---|
| `random.configure` | `{"config","dry_run"}` (`dry_run` optional) | `{"state","derived","warnings"}` |
| `random.config` | `{}` | `{"run_id","config","derived","views"}` |

```
{"title":"Qualification Random",
 "profile":{"breakpoints":[{"freq_hz":20,"psd_g2_hz":0.01},{"freq_hz":80,"slope_db_oct":3},
                           {"freq_hz":350,"psd_g2_hz":0.04,"alarm_db":[2,2]},
                           {"freq_hz":2000,"slope_db_oct":-3}],
            "alarm_db":[3,3],"abort_db":[6,6]},
 "acquisition":{"fs_hz":8192,"n_pts":2048,"n_avg":4},
 "schedule":{"mode":"auto","levels":[{"db":-12,"dwell_s":30},{"db":-6,"dwell_s":30},{"db":0}],
             "full_level_s":300},
 "control":{"review_after_sysid":true,"shutdown_ramp_s":3},
 "limits":[],"feasibility":null}
```

**Profile.** The reference, in g²/Hz. Each breakpoint row is either a PSD anchor
(`psd_g2_hz`) or a slope from the row before (`slope_db_oct`), never both; the first row is an
anchor and frequencies rise strictly. The test band runs from the first row to the last.
`alarm_db` and `abort_db` are `[below, above]` in positive dB: the band runs from the reference
times 10^(-below/10) to the reference times 10^(above/10). A row's own bands hold from that row
to the next; elsewhere the profile's bands hold (defaults `[3,3]` and `[6,6]`). An alarm band
wider than its abort band is a warning.

**Acquisition.** `fs_hz` is the rate requested of the station; hardware may run at a nearby
rate, and the run reports the rate it actually ran at. `n_pts` is the FFT length (a power of
two) and `n_avg` the averages per control loop. The lines are `fs_hz / n_pts` apart. A profile
whose top breakpoint the grid cannot reach is refused `band_above_grid`, and fewer than 8 lines
in the band is refused `too_few_lines`; both are checked again at arm against the rate the
hardware programmed.

**Schedule.** `levels` are dB relative to the reference, in non-decreasing order; the last is
full level. Every level but the last has a `dwell_s`. In `auto`, the run starts at the first
level, holds each for its dwell, holds full level for `full_level_s`, and ends `completed`. In
`manual`, the run starts at the first level and moves only on `random.level`, between the first
and last levels, until stopped; `full_level_s` applies if the ladder is handed back to `auto`.

**Control.** Every field is optional:

| Field | Default | Meaning |
|---|---|---|
| `composite` | `weighted_avg` | How the control channels (the table's enabled `control` rows) combine: `weighted_avg`, `min` or `max` |
| `weights` | 1 each | `[{"channel","weight"}]` for control rows |
| `control_dof` | 120 | Statistical degrees of freedom of the control estimate; higher is steadier and slower to adapt |
| `damping` | 4 | Correction damping, 0 (correct fully every loop) to 10 |
| `max_lines_out` | 10 | Lines outside the abort band in one loop that trip the envelope abort |
| `max_loops_out` | 5 | Consecutive diverging loops that trip the divergence abort |
| `max_sigma` | 5 | Drive clip, in standard deviations; null for no clip |
| `drive_ceiling_vrms` | 10 | The drive is held at or below this rms |
| `drive_abort_vrms` | null | A drive rms at or above this aborts; null for none |
| `coherence_floor` | 0.5 | Band coherence the system identification must reach |
| `review_after_sysid` | true | Hold after system identification for the operator to proceed |
| `pretest` | `{"averages":4,"level_db":-20,"max_vrms":1,"start_vrms":0.01,"shaped":false}` | System identification (below); `shaped` follows the reference's shape instead of a flat band |
| `soft_start_db` | 9 | The run engages this far below the first level and ramps up to it |
| `soft_start_relax_db` | 6 | The bands widen by this much until the first level is reached |
| `level_ramp_s` | 3 | Ramp time of every level change: the drive moves smoothly, output block by output block, while the control loop keeps its own cadence |
| `shutdown_ramp_s` | 3 | Ramp time of a controlled stop: 40 dB down from the level the stop began at, smoothly, block by block |

**The control estimate.** The loop controls to, and the envelope judges, an estimate of the
control spectrum accumulated over loops to `control_dof` degrees of freedom (each loop adds
`2 × n_avg`). A level change keeps it, scaled to the new level; a loop measured while the level
moves is folded in at the level it reached, and is not judged. The new level is also judged for
the envelope abort on its own estimate, from its first loop at rest, so an excursion at a new
level aborts as soon as that estimate holds 16 degrees of freedom (the first loop at `n_avg` 8
or more). No envelope alarm or abort is judged on fewer than 16 degrees of freedom.

**System identification.** It excites relative to the reference. It starts at `start_vrms` and
steps its drive until the control response's rms over the band is `level_db` (-60 to 0) relative
to the reference's rms over the band, within 1 dB, then measures the system over `averages`
control loops at that drive. The drive never exceeds `max_vrms` (null: no ceiling of its own)
or `drive_ceiling_vrms`. When the response cannot reach its level within that, or does not settle
on it, the run ends `sysid_level_unreachable` without driving further. `start_vrms` is greater
than 0 and at most `max_vrms`. The run's aborts (each channel's `abort` limits, and
`drive_abort_vrms`) are armed from the first identification drive, exactly as in the control
loop.

**Limits.** `[{"channel","kind","rows":[{"freq_hz","ceiling"}...]}]`, `kind` `response` or
`force`, `ceiling` a PSD in the channel's unit squared per Hz. The controller lowers the drive
where the channel would pass its ceiling. A limit never aborts.

**Feasibility.** Optional `{"shaker","moving_mass_lb","crest_factor"}`: `shaker` is a shaker
record in the controller's shaker-database form, `moving_mass_lb` is `{"armature","slip_table",
"drive_adapter","fixturing","test_article"}`, `crest_factor` defaults to 3. Checked at arm.
Without it, feasibility is not evaluated and the run says so.

**Result.** `state` is the `random.state` object (`configured`). `derived`:

```
{"grms_g":6.06,"velocity_rms_ips":2.31,"displacement_rms_mil":21.4,"band_hz":[20,2000],
 "grid":{"fs_hz":8192,"n_pts":2048,"df_hz":4,"lines":496,"loop_s":1,"actual":false},
 "control_channels":[1]}
```

`grid` is the requested grid until the run arms (`actual` false); `lines` counts lines in the
band; `loop_s` is the time one control loop takes. A configuration replaces the one before;
it is refused `run_active` while a run is live, and
`config_invalid` or `table_invalid` with the items when it has errors. Configuring checks the
table against the selected station and opens nothing. `dry_run: true` checks without
replacing anything and needs no control; its result is `{"errors","warnings","derived","views"}`
(`derived` null and `views` empty when there are errors).

`views` is `[reference]`, the profile on the grid, so a screen draws it without computing it:
**Reference** (`spectrum`, x `Frequency` Hz log, y `PSD` g²/Hz log, `link` `frequency`), series
in this order: `Reference` (`reference`), `Alarm high` and `Alarm low` (`tolerance`), `Abort high`
and `Abort low` (`abort`), at 0 dB with each row's own bands applied, on the band's lines of the
requested grid, each `shape: "steps"` with its `edges`. A result carrying it is a binary frame.

`random.config` returns what the present run was configured with: its `run_id`, the `config` as
accepted, its `derived` values and the same `views`, so a screen that reloads, or a second
screen, shows the profile and the numbers the run uses. All four are null (`views` empty) when
nothing is configured. Like `random.state` it only reads. `derived.grid` stays the requested
grid; the run state's `grid` is the one the run uses once it arms.

### Running a Random Test

| Method | Params | From | Result |
|---|---|---|---|
| `random.arm` | `{"override_ack"}` (optional) | `configured`, `blocked` | state |
| `random.start` | `{}` | `configured`, `armed` | state |
| `random.proceed` | `{}` | `review` | state |
| `random.level` | `{"delta_db"}` or `{"to_db"}` | `running`, in `manual` | `{"level_db"}` |
| `random.mode` | `{"mode":"auto"|"manual"}` | any configured state | `{"mode"}` |
| `random.stop` | `{}` | any | state |
| `random.abort` | `{}` | any | state |
| `random.reset` | `{}` | any but `running`, `review`, `stopping` | state |
| `random.clear` | `{}` | any but `running`, `review`, `stopping` | state |
| `random.state` | `{}` | any | state |
| `random.log` | `{"since"}` (optional, default 0) | any | `{"entries","next"}` |

A run is live while it is armed, running, holding for review or stopping. "state" in the
Result column is the `random.state` object below.

- **`random.arm`** checks feasibility (when given), opens the station, programs the rate, builds
  the analysis grid at the rate the hardware actually runs at, and passes the controller's arm
  gate (shared sample clock, rate, grid). Nothing is driven. The state becomes `armed`, with
  `grid.actual` true and `clock`. When the profile exceeds the shaker's ratings it is refused
  `feasibility_exceeded` and the state becomes `blocked`; `random.arm` with a non-empty
  `override_ack` then arms past it, and the text is logged verbatim and kept with the run.
  Refusals: `not_controller`, `not_configured`, `run_active`, `run_finished`,
  `feasibility_exceeded`, `override_not_needed`, `bad_params` (an empty acknowledgement),
  `arm_refused`, `hardware_busy`, `devices_absent`, `table_invalid`.
- **`random.start`** from `configured` arms first, with the same refusals. It then runs the
  system identification at `pretest.level_db` and the coherence gate, and either holds for review
  (`review`) or engages: the soft start, then the schedule. It answers at once (`running`).
- **`random.proceed`** releases the review hold and engages.
- **`random.level`**, in `manual` only: `delta_db` steps from the present level (the shell's
  plus and minus 1 dB), held between the first and last levels; `to_db` goes to a level of the
  schedule (`not_a_rung` otherwise). Every change ramps over `level_ramp_s`. Refusals:
  `manual_mode_required`, `not_a_rung`, `wrong_state`, `not_controller`.
- **`random.mode`** hands the ladder over live. `auto` to `manual` holds the present level;
  `manual` to `auto` resumes from the present level with its dwell restarted, and from a level
  between two of the schedule's, it moves to the one above when the dwell ends.
- **`random.stop`** is the controlled stop: while driving, the drive starts down at the next
  output block, not the next control loop, and ramps 40 dB down over `shutdown_ramp_s` (state
  `stopping`); the run ends `operator_stop` at the end of the control loop in which the ramp
  arrives. From `armed` or `review` it ends at once. It answers at once.
- **`random.abort`** ends the run within one control loop (`grid.loop_s`), without the shutdown
  ramp; the station takes the drive down with its own stop. The outcome is `operator_abort`. It
  answers when the loop has ended.
- `random.stop` and `random.abort` are accepted from any connection (see "Controller and
  Observers"); every other verb in this table needs control.
- `random.stop` and `random.abort` on a run that is not live answer with the state, unchanged.
- **`random.reset`** builds a new run from the same configuration (`configured`, a new
  `run_id`). It and `random.configure` are the ways out of `finished`.
- **`random.clear`** drops the configuration and releases the station (`unconfigured`).

### Shutting Down

The control service does not end its process with a run live: a killed process can leave an
output holding its last sample. Told to stop (Ctrl-C, SIGTERM or SIGHUP on Unix; Ctrl-C,
Ctrl-Break or a closed console on Windows; and, started with `--watch-stdin`, the end of its
standard input) it refuses every verb that could start or raise a drive (`shutting_down`; one
already under way finishes first), ends a live run with the controlled stop, the path of
`random.stop` (while driving, the level ramps down over `shutdown_ramp_s`; never an abort), and
exits once the run has ended. The run's outcome is `safe_stop`, and its message says the service
was shut down. Connections are served until then, so every screen sees the run end and keeps
Stop and Abort. A stop that has not ended well after its ramp (three control loops, ten seconds
and twice `shutdown_ramp_s`) is followed by the abort.

`--watch-stdin` is for a parent process that starts the service, such as the desktop app: the
parent stops it by closing the pipe on its standard input, never by killing it, and if the
parent dies the operating system closes the pipe, so the run stops the same way.

### Run State

`random.state` answers with the state object, and the `random.state` text event carries the same
object to every connection on every change of `state`, `phase`, `mode`, `level_db`, `alarm` or
`end`.

```
{"run_id":"r-0007","state":"finished","phase":"aborted","title":"Qualification Random",
 "source":"HARDWARE",
 "station":{"kind":"ni","class":"ni_real","devices":[{"id":"cDAQ1Mod1","serial":"01A2B3C4"}]},
 "grid":{"fs_hz":51200,"n_pts":8192,"df_hz":6.25,"lines":317,"loop_s":0.64,"actual":true},
 "clock":"shared","mode":"auto","levels_db":[-12,-6,0],"level_db":null,"full_level_s":300,
 "elapsed_s":131.2,"at_level_s":42.5,"remaining_at_level_s":null,"alarm":false,
 "feasibility":{"verdict":"not_evaluated","message":"No shaker was given, so the profile was not checked against shaker ratings."},
 "end":{"outcome":"aborted","message":"Envelope abort: 14 lines outside the abort band, worst +7.2 dB, at 0.0 dB.",
        "at_s":131.2,"level_db":0,
        "trip":{"kind":"envelope","abort_lines":14,"worst_db":7.2,"at_level_db":0,"at_loop":212}}}
```

| `state` | `phase` | Meaning |
|---|---|---|
| `unconfigured` | `idle` | No configuration |
| `configured` | `idle` | Configured; nothing open |
| `blocked` | `feasibility_blocked` | Feasibility exceeded; arming needs an acknowledgement |
| `armed` | `armed` | Station open and gated; nothing driven yet |
| `running` | `sysid`, `coherence_gate`, `engaged`, `full_level` | Driving: system identification, its gate, the ladder, full level |
| `review` | `awaiting_review` | System identification passed; nothing drives until `random.proceed` |
| `stopping` | `ramping_down` | A controlled stop or a safe stop is ramping the level down |
| `finished` | `stopped`, `aborted`, `coherence_hold` | Ended; `end` says how |

`phase` is the controller's own phase in snake case, with `armed` (armed, before the first
drive) and `ramping_down` added; `full_level` begins once the drive has reached full level (a
level change toward it reads `engaged` while it ramps). `run_id` is null when unconfigured, and new at every
configure and reset. `source`, `station`, `grid` and `clock` describe the run's station
(`clock`, `shared`, `independent` or `unknown`, is null until the run arms). `level_db` is the
commanded level (`pretest.level_db` during system identification, and the level the
ramp-down is headed for while `stopping`), null when nothing drives. `elapsed_s` counts from `random.start` (null
before). `at_level_s` and `remaining_at_level_s` are the time at the present level and the
time left on it (null in `manual`). `alarm` is true while an alarm condition holds (see
`random.live`). `feasibility` is null until arm, then `{"verdict","message"}` with `verdict`
`pass`, `advisory`, `exceeded` or `not_evaluated`.

`end` is null until the run finishes, then `{"outcome","message","at_s","level_db","trip"}`.
`level_db` and `message` give the level where the end began: where the drive was when the stop
or abort was asked for (a controlled stop's ramp-down starts there), or where the trip came;
`level_db` is null when nothing drove.

| `outcome` | How the run ended |
|---|---|
| `completed` | The schedule finished |
| `operator_stop` | `random.stop` |
| `operator_abort` | `random.abort` |
| `safe_stop` | The service stopped the run with the controlled stop: nobody held control for `timeout_s` (see Controller and Observers), or the service was shut down (see Shutting Down); `message` says which |
| `aborted` | A protection abort tripped; `trip` says which |
| `held` | The coherence gate held: the band coherence stayed below `coherence_floor`, so the run never drove at test level |
| `sysid_level_unreachable` | System identification could not bring the control response to `pretest.level_db` within its drive ceiling, or the response did not settle on it; the run never drove at test level |
| `fault` | The loop or the station failed; `message` is the controller's words |
| `law_fault` | The control law did not produce a usable drive for several loops in a row (3 by default); the run took its controlled stop, with its shutdown ramp. `trip` says why |

The shell shows every outcome but `completed` and `operator_stop` as an abort with `message`.
`trip` is null unless `outcome` is `aborted` or `law_fault`, and then has `at_level_db`, `at_loop`
and its `kind`'s facts. A trip during system identification has `at_loop` 0 and `at_level_db`
`pretest.level_db`, and `message` says it came during system identification:

| `kind` | Facts |
|---|---|
| `envelope` | `abort_lines`, `worst_db` |
| `kinematic` | `channel`, `axis` (`accel`, `velocity` or `displacement`), `value`, `limit` |
| `drive_vrms` | `realized_vrms`, `limit_vrms` |
| `divergence` | `consecutive_loops`, `divergent_bins` |
| `law_fault` | `reason`, `consecutive` (unusable law calls in a row) |

A `law_fault` trip's `reason` is what was wrong with the last unusable law output: `error` (the
law reported one), `panicked` (for a law loaded from a file: it trapped), `over_budget` (the call
took longer than its budget, by default a quarter of one loop's acquisition time), `wrong_length`,
`non_finite`, `negative` (a drive line), `non_finite_level`, or, for a law loaded from a file only,
`over_memory` (it asked for more memory than its cap) or `bad_diagnostics` (its diagnostics broke
the output rules). Its `outcome` is always `law_fault`, never `aborted`: the drive in force was
held through every miss, so the run stops the way an operator stop does.

### Live Display

Event topic `random.live`, a binary frame 5 times a second from the start of system
identification until the run finishes, and once more with the final state:

```
{"run_id","source","class","state","phase","mode","elapsed_s","level_db","commanded_db",
 "at_level_s","remaining_at_level_s","level_s","loop",
 "control_rms_g","reference_rms_g","control_error_db","drive_rms_v",
 "lines":{"in_band","alarm","abort"},
 "channels":[{"index":1,"description":"Control","role":"control","rms":6.02,"unit":"g","overload":false}],
 "alarms":[{"kind":"envelope","lines":2,"worst_db":3.4}],
 "views":[control spectrum, drive spectrum, run readings]}
```

- `level_db` is the level the drive is at now, read for every frame; after a level change it
  walks to the commanded level over `level_ramp_s`, and in a controlled stop it walks down over
  `shutdown_ramp_s`. Null when nothing drives.
- `commanded_db` is the level the run is headed for, the same value as the run state's
  `level_db` (`pretest.level_db` during system identification). While `level_db` and
  `commanded_db` differ, the drive is ramping. `phase` turns `full_level` only once the drive
  has reached full level, so a level change toward full level reads `engaged` while it ramps.
- `level_s` is the time the schedule holds the present level: its `dwell_s`, or `full_level_s`
  at full level. Null in `manual` and while nothing drives. With `at_level_s` and
  `remaining_at_level_s` it is a level's progress.
- `loop` counts control loops since the start. `control_rms_g` is the composite control rms
  over the band (null before the first loop), `reference_rms_g` the reference rms at the
  last control loop's level (the target), `control_error_db` the difference
  `20 log10(control_rms_g / reference_rms_g)` (null until both are known), and `drive_rms_v` the
  drive rms: the last loop's drive spectrum at the level the drive is at now. The Control view's reference and bands are drawn at the last
  control loop's level, the level its measured series belongs to.
- `lines` counts the band's lines, and how many lie outside the alarm and the abort bands in
  the last loop.
- `channels` lists every enabled input row in table order, with its rms over the band in its
  own unit and `overload` true once the row has reached 90 % of `range_max` this run, or the
  station has seen a sample at 98 % of the row's configured range. Every station makes that
  check in software, so it does not depend on the driver reporting overload (the NI 9234's
  driver does not).
- `alarms` lists the conditions holding now: `{"kind":"envelope","lines","worst_db"}` (lines
  outside the alarm band) and `{"kind":"overload","channel"}`. An alarm warns; it does not stop
  the run.

The views are the viz kit's, in this order:

1. **Control** (`spectrum`, x `Frequency` Hz log, y `PSD` g²/Hz log, `link` `frequency`), series
   in this order: `Control` (`measured`; absent until the first loop), `Reference`
   (`reference`), `Alarm high` and `Alarm low` (`tolerance`), `Abort high` and `Abort low`
   (`abort`). Every series is on the band's lines of the grid the run actually uses, with
   `shape: "steps"` and its `edges`, so each line is drawn flat across its own bin (the
   stairstep). The reference and the bands are drawn at the last control loop's level, with
   each row's own bands applied.
2. **Drive** (`spectrum`, y `PSD` V²/Hz log, same `link`): `Drive` (`drive`), stepped on the same
   lines.
3. **Run** (`reading`): the numbers above as rows, for a pane that shows readings.

`random.live` is the **default subscription**: what every connection receives until it asks for
something else (below).

### Live Data Subscriptions

A screen asks for the quantities and channels its plots show, at the rate it wants, and the
service sends that connection only those. On a large channel count this is what keeps a display
feed a display feed: a channel nobody plots is not computed into a frame or sent. A subscription
narrows the display and nothing else; the control loop reads every channel at the station's
rate whatever any screen asks for.

| Method | Params | Result |
|---|---|---|
| `random.subscribe` | `{"rate_hz","plots":[plot...]}`, or `{"default":true}` | `{"default","rate_hz","plots","quantities"}` |

```
{"rate_hz":5,"plots":[{"id":"ctl","quantity":"control"},
                      {"id":"lim","quantity":"response","channels":"limited"},
                      {"id":"t2","quantity":"time","channels":[2,3],"points":1024}]}
```

- Every connection starts on the default subscription and receives `random.live`. A
  `random.subscribe` with `plots` replaces it: from then on the connection receives `random.data`
  (below) at `rate_hz` and no `random.live`. `{"default":true}` returns it to `random.live`.
- Each call replaces the connection's subscription as a whole; a client sends its full list
  whenever a plot is added, removed or changed. The subscription belongs to the connection, lasts
  until it closes, and carries over from run to run, so a screen subscribes once and again only
  when its plots change.
- It only reads, so it is open to every connection, controller or observer (a window on another
  monitor is an observer with a subscription of its own), in any run state, configured or not.
- `rate_hz` is 0.5 to 10, default 5: the most frames per second the connection is sent.
- `plots` holds up to 64 entries, and may be empty (the run's numbers only, for a window that
  shows no plot). Each is `{"id","quantity","channels","points"}`. `id` is the client's own name
  for the plot, 1 to 64 characters and unique in the list; the service returns it with the
  plot's data. `quantity` is one of the table below. `channels` applies to `response` and `time`
  only: a list of channel indexes, or `"all"` (every enabled input, the default), `"control"`
  (the control rows) or `"limited"` (the rows the configuration limits). The service resolves it
  against the run's table on every frame, in table order; an index the run does not have is left
  out, never refused, so a plot kept across a table edit shows fewer channels rather than
  failing. `points` applies to `time` only: the most points sent per channel, 64 to 8192,
  default 1024.
- Refusals: `bad_params` with `param` naming the field (for example `plots[2].quantity`): an
  unknown quantity, `channels` or `points` on a quantity they do not apply to, a duplicate `id`,
  a value out of range.
- The result echoes the subscription with every default filled in (`default` false, `plots` as
  given), or `{"default":true,"rate_hz":5,"plots":null}`, and `quantities`, the list of quantities
  this service offers, so a screen offers only what the service can send.
- When the run has already streamed, the service also sends the connection one `random.data`
  frame at once, so a plot added mid-run, or a window opened after the run ended, fills without
  waiting.

| `quantity` | Views, in order | What they show |
|---|---|---|
| `control` | one `spectrum` | The **Control** view of `random.live`: control against the reference, with the alarm and abort lines |
| `drive` | one `spectrum` | The **Drive** view of `random.live` |
| `response` | one `spectrum` per resolved channel | That channel's PSD over the band, titled `Channel <index>: <description>` (the role when there is no description), y `PSD` in the channel's unit squared per Hz: a series `Channel <index>` (`measured`, stepped on the run's lines; absent until the first loop), and when the configuration limits the channel, a series `Limit` (`limit`, stepped) on the lines its rows cover, each across the same bin as the measured line |
| `time` | one `spectrum` per resolved channel | The last control loop's acquired samples of that channel, same title, x `Time` s linear, counted from the start of the loop's segment, y `Amplitude` in the channel's unit, linear: a series `Channel <index>` (`measured`, shape `line`; absent until the first loop). Decimated to `points` as a minimum and a maximum per interval, in time order, so no peak is lost |
| `coherence` | one `spectrum` | The system identification's ordinary coherence between drive and control on the band's lines, x `Frequency` Hz log, y `Coherence` linear (0 to 1), `measured`, shape `line`. No series until identification has measured it |
| `readings` | one `reading` | The **Run** view of `random.live` |

Event topic `random.data`, a binary frame to one connection, at most `rate_hz` times a second
from the start of system identification until the run finishes, and once more with the final
state. It carries every field of `random.live` except `views`, then `plots`, in the
subscription's order:

```
{..., "plots":[{"id":"lim","quantity":"response","channels":[2,3],"views":[spectrum, spectrum]}]}
```

`channels` is the resolved list (null for the quantities without channels), and for `response`
and `time` the views are in that order, one per channel. A plot whose channels resolve to none
has `channels: []` and no views.

### Run Log

`random.log` returns `{"entries":[entry...],"next"}` from `since`, and the `random.log` text
event carries `{"run_id","entries":[...]}` as entries are written. An entry is
`{"seq","at_unix","kind","text"}`, `kind` one of `state`, `feasibility`, `override`, `station`,
`gate`, `level`, `alarm`, `overload`, `trip`, `note`. The log starts at configure and is the
run's level history and alarm record.

### Run Files (Later)

A run already carries what its `.hforge` file will record: `run_id`, `source` and `station`,
the configuration as accepted, the actual grid, the log, and `end`. When run files are added,
the control service writes the recording under its local data directory while the run goes and
converts it to `.hforge` when the run finishes, and `end` names the file. That layout is
specified with the run-file format, not here.
