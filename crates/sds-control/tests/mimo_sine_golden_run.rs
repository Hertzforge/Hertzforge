//! The golden MIMO Sine run folder, `schema/golden/runs/mimo_sine/`, against
//! today's code: a short sweep on the simulator's linear 4 by 4, unpaced, with
//! the simulator's and the identification's fixed seeds, gives the golden
//! folder once it is normalized (`support::msine_golden::normalized` says how).
//! The analysis side converts the same folder in
//! `tests/test_run_folder_golden_mimo_sine.py`.
//!
//! `scripts/make_golden_runs.sh` rewrites the folder (this test with
//! `HERTZFORGE_WRITE_GOLDEN_RUNS=1`); otherwise the test only compares.
//!
//! The run is SIMULATED; nothing here is evidence about hardware.

mod support;

use support::golden_runs::{compare, write_golden};
use support::msine_golden::{golden_dir, golden_run, normalized, MOST_BYTES};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_sine_todays_code_writes_the_golden_run_folder() {
    let (folder, _end) = golden_run().await;
    let got = normalized(&folder, false);
    let total: usize = got.values().map(Vec::len).sum();
    if std::env::var_os("HERTZFORGE_SHOW_GOLDEN_SIZES").is_some() {
        for (name, bytes) in &got {
            println!("{name}: {}", bytes.len());
        }
    }
    assert!(
        total <= MOST_BYTES,
        "the golden folder holds {total} bytes, over {MOST_BYTES}"
    );
    if std::env::var_os("HERTZFORGE_WRITE_GOLDEN_RUNS").is_some() {
        write_golden(&golden_dir(), &got);
        println!("wrote {} ({total} bytes)", golden_dir().display());
        return;
    }
    let want = normalized(&golden_dir(), true);
    compare(&got, &want);
}

/// The run is the same run every time: two runs of the golden setup give the
/// same folder, so the golden folder is a fact about the code.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_sine_the_golden_run_repeats_exactly() {
    let a = normalized(&golden_run().await.0, false);
    let b = normalized(&golden_run().await.0, false);
    for (name, bytes) in &a {
        assert_eq!(Some(bytes), b.get(name), "{name} differs between two runs");
    }
    assert_eq!(a.len(), b.len());
}
