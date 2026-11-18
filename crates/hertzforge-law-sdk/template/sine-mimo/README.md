# A MIMO Sine Control Law

This folder is a complete MIMO Sine control law: each control paired with one drive, each
paired drive's amplitude following its control's ratio of wanted to measured amplitude. Copy
it, rename the package in `Cargo.toml`, and change `src/law.rs`.

## What This Law Does

Control `i` is paired with drive `i`, for as many pairs as there are controls or drives,
whichever is fewer. At engage each paired drive starts at its control's reference over the
identification's path from that drive to that control. Each update after it, each paired drive
in force is multiplied by the ratio of wanted to measured amplitude at its control, raised to
`gain`, at most `max_step_db` either way. While the controller holds the drive (the backstop
notch, a probe, or the settle window after engage) the law returns the drive in force.

It knows nothing of the coupling between pairs. On a structure where one drive moves several
controls the pairs work against each other, which is why its manifest accepts dwells and stepped
sines and not sweeps. The built-in `matrix` law is the one for coupled structures.

## What a MIMO Sine Law Does

The controller plays one tone on every drive, each drive with its own amplitude and phase, and
decides the tone's frequency, the sweep, the tone's phase and the level schedule. The law decides
one complex amplitude per drive every control update (10 ms by default), in volts peak at the
scheduled level. It is built once per run after the structure has been identified, called once
for the starting drive (`engage`) and then once an update (`tick`). Each update has a time budget
of a quarter of the update; building the law has a longer one, for a law that fits a model.

Each update the law sees the frequency and the sweep rate, the scheduled level, every control's
reference at this frequency and level, every control's and limit channel's tracked amplitude, the
limit bounds, and the drive in force as it was played. A channel's tracked amplitude is its
response's phase minus the tone's, so a linear structure gives `C = H D` with `H` the
identification. `hold` says why the controller holds the drive: `notch` (a limit channel holds it
down), `held` (a hold the law asked for was granted), `probe` (the drive returned is not used) or
`engage` (the settle window after engage).

The controller applies its own guards to whatever the law returns: one finite drive per drive,
the change per update and the rise per second capped on each drive, the whole drive vector scaled
down for the limit channels and for a drive over its ceiling, a bound on the first drive, and the
aborts. A law needs none of those for safety. The drive in force can differ from the drive the
law asked for, so a law that integrates should start each update from the drive in force.

A law may also return a request to hold or slow the sweep (honoured only when the run allows it)
and a lowered reference for a limit channel; this law does neither.

## What the Manifest States

| Field | This law |
|---|---|
| Shapes | square, more controls than drives, fewer controls than drives |
| Test types | dwell, stepped |
| Amplitude-only reference | accepted (the law reads amplitudes only) |

The controller refuses a run whose shape, test type or reference the law does not accept.

## Parameters of This Law

| Key | What it does |
|---|---|
| `gain` | The power the ratio of wanted to measured amplitude is raised to each update |
| `max_step_db` | The most a drive's amplitude moves in one update |

## Building

```sh
cargo build --target wasm32-wasip2 --release
```

The law file is `target/wasm32-wasip2/release/my_sine_mimo_law.wasm`. Import it into the
controller's law catalog and choose it for a MIMO Sine run.
