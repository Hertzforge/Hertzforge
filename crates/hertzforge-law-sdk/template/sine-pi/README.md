# A Sine Control Law

This folder is a complete single-output Sine control law: a proportional-integral loop on the
error in dB, written plainly. Copy it, rename the package in `Cargo.toml`, and change
`src/law.rs`.

## What This Law Does

It starts at the identification's prediction (moved by `start_offset_db`) and holds that drive
until the measurement has come up: at least five ticks, then until the measured amplitude is
within 2 dB of what the identification predicts for that drive (or over it), and at most a second.
Without a trusted prediction it holds the second. Then each tick it moves the drive by the PI step
on the error in dB plus the prediction's change, at most `max_step_db`. A change of level is taken
by the loop, with no hold: the measurement already reads the drive in force.

The hold is there because a drive's response reaches the measurement late. On hardware the
converters and the output buffer delay it, and the controller's tracking filter, whose corner
scales with the frequency, then takes a third of a second to read it at 20 Hz. On the way up the
reading can stall and dip for several ticks, so a rise that has stopped is no sign that it has
come up; what the identification predicts for the drive in force is. An error read before the
response has arrived is one the law did not cause: a law that integrates it climbs on a reading of
nothing, and by the time the response arrives the drive is several dB high.

## What a Sine Law Does

The controller plays one tone and decides its frequency, its sweep, its phase and the level
schedule. The law decides one number every control tick (10 ms by default): the tone's amplitude,
in drive units peak (volts peak on hardware). It is built once per run after the system has been
identified, called once for the starting drive (`engage`) and then once a tick (`tick`). Each call
has a time budget of a quarter of the tick.

Each tick the law sees the frequency, the target at the present level, the measured control
amplitude, the drive in force and the identification's prediction of the drive at this frequency
(`prediction_db`, dB relative to one drive unit at full level; absent where the identification is
not trusted). `hold` says why the controller holds the drive: `notch` (a limit channel holds it
down; do not raise it) or `probe` (a resonance search; the drive returned is not used).

The controller applies its own guards to whatever the law returns: a drive that is not a finite
number at least zero is refused, the change per tick and the rise per second are capped, the first
drive is bounded near the prediction, and the limit channels, the drive ceiling and the drive abort
apply. A law needs none of those for safety.

## Parameters of This Law

| Key | What it does |
|---|---|
| `kp` | Proportional gain on the change of the error, per dB |
| `ki` | Integral gain on the error, per second; can be changed while the test runs |
| `follow` | How much of the prediction's change from tick to tick the drive follows (0 to 1) |
| `max_step_db` | The most the law moves the drive in one tick |
| `start_offset_db` | Scales the law's own first drive, for seeing how the controller's guards respond |

## Building

```sh
cargo build --target wasm32-wasip2 --release
```

The law file is `target/wasm32-wasip2/release/my_sine_law.wasm`. Import it into the controller,
check it on the simulator, and choose it for a Sine run.
