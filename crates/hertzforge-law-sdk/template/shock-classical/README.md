# A Classical Shock Control Law

This folder is a complete classical shock control law: the frequency-domain correction, written
plainly. Copy it, rename the package in `Cargo.toml`, and change `src/law.rs`.

## What a Classical Law Does

The controller identifies the system, inverts the identified plant, and then calls the law between
pulses: once for the first drive (`seed`), and once after each pulse has been fired, measured and
judged (`iterate`). Each call returns the next full-level drive record in volts. The controller
scales it to the scheduled level, checks it, and brings it to rest at both ends before it plays.

Everything a law sees and returns is at full level, on one grid: `record_len` samples (a power of
two) at `fs_hz`. `rfft_half` and `irfft_half` in `hertzforge_law_sdk::shock` transform records the
same way the controller formed the plant it hands the law.

## Parameters of This Law

| Key | What it does |
|---|---|
| `beta` | The fraction of each pulse's error corrected before the next pulse |
| `correct` | Off: the first drive is held for the whole run |
| `model_error_db` | Scales the law's own copy of the inverse, for seeing how the controller's guards respond |

## Building

```sh
cargo build --target wasm32-wasip2 --release
```

The law file is `target/wasm32-wasip2/release/my_classical_law.wasm`. Import it into the
controller, check it on the simulator, and choose it for a classical shock run.
