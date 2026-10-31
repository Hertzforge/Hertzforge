# Suite Protocol, Version `sds/1`

The one contract between the shell (TypeScript), the analysis service (Python) and the control
service (Rust). `protocol.schema.json` beside this file is the machine-readable form; when this
text and the schema disagree, fix one of them in the same change. Golden frames in `golden/` are
the shared test data every language's codec must reproduce.

`sds` is a working name. Renaming it is a mechanical find and replace.

## Transport

WebSocket, one connection per service, loopback by default.

| Service  | Default address           | Started by                              |
|----------|---------------------------|-----------------------------------------|
| analysis | `ws://127.0.0.1:8791/`    | `uv run --project python sds-analysis --port 8791` |
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
          {"type":"response","id":<int>,"error":{"code":"<code>","message":"<text>"}}
event     {"type":"event","topic":"<name>","seq":<int>,"data":{...}}
```

Error codes: `bad_request`, `unknown_method`, `not_found`, `refused`, `internal`. The message
names the cause in words a user can act on; an error never comes back as an empty result.

## Views: The Viz Kit's Input

Every result and every live event carries `views`, a list the shell renders with its shared viz
kit. Services never send pixels and the shell never computes the numbers.

**`spectrum`** (also used for any x-y line data)

```
{"view":"spectrum","title":"...",
 "x":{"label":"Frequency","unit":"Hz","scale":"log"|"linear"},
 "y":{"label":"PSD","unit":"g²/Hz","scale":"log"|"linear"},
 "series":[{"name":"...","role":"measured"|"reference"|"tolerance"|"abort"|"drive"|"other",
            "x":{"$array":..},"y":{"$array":..},
            "shape":"line"|"steps"|"law", "edges":{"$array":..}}],
 "link":"frequency"}
```

`role` sets the line style so no meaning rests on color alone: measured solid, reference
dashed, tolerance dotted, abort dash-dot, drive thin solid. `link` (optional) names the cursor
group; panes with the same `link` share a cursor position on x.

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
 "modes":[{"index":1,"frequency_hz":12.3,"damping":0.01|null,"shape":{"$array":.. f32 [N,3]}}]}
```

Shapes are real, one translation vector per node, in the same node order as `nodes`.

## Operations: Declarative Panels

The analysis service describes each operation; the shell builds the panel from the description.
A feature that fits this pattern needs no TypeScript.

```
{"id":"random.psd_vs_spec","title":"PSD Against Specification","workspace":"analyze",
 "accepts":"random_run","summary":"one sentence",
 "params":[{"name":"tolerance_db","label":"Tolerance","type":"number","unit":"dB",
            "default":3,"min":0,"max":12,"step":0.5},
           {"name":"channel","label":"Channel","type":"enum",
            "options":[{"value":"ch1","label":"Control 1"}],"default":"ch1"}],
 "outputs":["spectrum","reading"]}
```

Param `type`: `number`, `integer`, `enum`, `boolean`, `string`. An `enum` whose options depend
on the dataset sets `"options_from":"channels"`; the shell fills them from the dataset's
`channels` list returned by `dataset.open`.

`workspace` is one of `acquire`, `analyze`, `modal`, `model`, `specify`, `simulate`, `report`.
`accepts` is a dataset kind (`random_run`, `modal_model`); the shell shows an operation only when
the selected dataset is of that kind ("tools follow the data").

## Methods

**Analysis service**

| Method | Params | Result |
|---|---|---|
| `catalog.list` | `{}` | `{"operations":[descriptor...],"datasets":[{"id","kind","title","description"}]}` |
| `dataset.open` | `{"id"}` | `{"handle","kind","title","summary":{...},"channels":[{"value","label"}]}` |
| `op.run` | `{"op","handle","params":{...}}` | `{"views":[view...]}` |

**Control service**

| Method | Params | Result |
|---|---|---|
| `sim.profiles` | `{}` | `{"profiles":[{"id","title","summary"}]}` |
| `sim.start` | `{"profile","level_db","speed"}` | `{"run_id"}` (`speed` 1 = real time, 0 = as fast as possible) |
| `sim.stop` | `{}` | `{"state"}` |
| `sim.state` | `{}` | `{"state":"idle"|"running"|"stopped","run_id"|null}` |

`speed` is a multiple of real time from 0 to 1000; 0 means as fast as the engine runs.

Event topic `random.display`, about 4 to 10 per second while running:
`{"run_id","source":"SIM","state","elapsed_s","level_db","views":[spectrum, reading],
"phase","outcome","reason"}`. `phase` is the controller's own phase name. Once `state` is
`stopped`, `outcome` is one of `completed`, `operator_stop`, `aborted`, `held`, `fault`, and
`reason` says why in words; the shell shows any outcome other than `completed` or
`operator_stop` as an abort with that reason.
`source` is always present on control data; the shell shows it as a badge. A simulated run is
never presented as a hardware result.
