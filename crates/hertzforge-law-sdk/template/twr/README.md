# A Time Waveform Replication Control Law

This folder is a complete time waveform replication (TWR) control law: the frequency-domain
correction, written plainly. Copy it, rename the package in `Cargo.toml`, and change `src/law.rs`.

## What a TWR Law Does

The controller identifies the system, inverts the identified plant, and holds for review. When the
run proceeds it builds the law and calls it between plays: once for the first drive (`seed`), and
after each play it wants corrected (`iterate`): a correction play that did not meet the gate, and
a playback play when adaptation during playback is on. Each call returns the next full-level drive
record in volts. The controller scales it to the commanded level, checks it, brings it to rest at
both ends and plays it.

Everything a law sees and returns is at full level, on one grid: `record_len` samples (a power of
two) at `fs_hz`, a quiet lead-in, the replicated section and a quiet lead-out. `rfft_half` and
`irfft_half` in `hertzforge_law_sdk::twr` transform records the same way the controller formed the
plant it hands the law. When the controller clipped the last drive, `limited` is set and `drive` is
the drive that played.

## Parameters of This Law

| Key | What it does |
|---|---|
| `beta` | The fraction of each play's error corrected before the next play |
| `correct` | Off: the first drive is held for the whole run |
| `model_error_db` | Scales the law's own copy of the inverse, for seeing how the controller's guards respond |

## Building

```sh
cargo build --target wasm32-wasip2 --release
```

The law file is `target/wasm32-wasip2/release/my_twr_law.wasm`. Import it into the controller,
check it on the simulator, and choose it for a TWR run.
