# An SRS Shock Control Law

This folder is a complete SRS shock control law: the per-band ratio update, written plainly. Copy
it, rename the package in `Cargo.toml`, and change `src/law.rs`.

## What an SRS Law Does

The controller synthesizes every pulse from a table of components (odd-half-cycle wavelets or
damped sines) and identifies the system before the first pulse. It calls the law between pulses:
once for the starting gains (`seed`), and once after each pulse has been fired, measured and
judged (`iterate`). Each call returns one multiplicative gain per component. The controller applies
the gains to its table, caps them by how far any gain may rise in one pulse and by what its limit
channels can take, and checks the drive in volts before it plays.

Every spectrum is a shock response spectrum on the setup's band grid (`band_hz`). The components
are on their own grid (`component_hz`); `nearest_band` in `hertzforge_law_sdk::srs` maps a component
to the band nearest it. The target a law sees on each call is at the scale the last pulse was
commanded at.

## Parameters of This Law

| Key | What it does |
|---|---|
| `max_db_per_pulse` | The most any component may rise or fall from one pulse to the next |
| `correct` | Off: the starting gains are held for the whole run |
| `seed_error_db` | Scales the law's starting gains, for seeing how the controller's guards respond |

## Building

```sh
cargo build --target wasm32-wasip2 --release
```

The law file is `target/wasm32-wasip2/release/my_srs_law.wasm`. Import it into the controller,
check it on the simulator, and choose it for an SRS shock run.
