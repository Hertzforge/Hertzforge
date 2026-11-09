//! The recorded trace: what the TWR core does play by play with no control law
//! seam (`control: None`), held fixed while the seam is built beside it.
//!
//! Per play it records the SHA-256 of the drive record as played (`f64`
//! little-endian bytes), the metrics, the gate, the clamped bins, the clipped
//! samples, the state after it, the level, the rung and the iteration; per run
//! the identification's coherence and coverage, and the end: the state, the
//! trip by its kind with its numbers, and the abort sentence.
//!
//! Fixtures, on `sim_e2e`'s three-mode plant: (a) `sim_e2e`'s field-like run
//! through the ladder, with its 6 dB backoff; (b) a run whose only rung is
//! 0 dB; (c) MANUAL with three +1 dB steps and a jump to 0 dB, then playback;
//! (d) a run with the per-sample clip engaged; (e) the 6 dB backoff in MANUAL
//! at -12 dB, four corrections; (f) adaptation during three playback plays;
//! (g) a plant whose sign flips after the identification (divergence); (h) a
//! plant whose gain triples after the first correction (the envelope). The
//! fixtures other than (a) and (e) start the seed at the predicted drive (no
//! backoff). The correction's `beta` and growth clamp are the defaults
//! everywhere, so a change to either moves the trace.
//!
//! The trace was recorded once, before the seam changed anything, into
//! `tests/data/twr_trace.txt`. On the platform it was recorded on (its first
//! line says which) the trace must match byte for byte. On another platform
//! the floating-point library may round a last bit differently, so the drive
//! hashes are not compared there and every number is held to a relative 1e-9;
//! everything discrete must still match.
//!
//! `HERTZFORGE_RECORD_TRACE=1` rewrites the file. Only do that when the run
//! without the seam is meant to change, and say so in the commit.
//!
//! Re-recorded once, when the rms and peak errors came to be taken about the
//! record's mean (`metrics`): only the plays' `rms_pct` and `peak_pct` and the
//! one `not_converged` trip's `error_pct` moved (at most 0.034 and 0.29
//! points, and 0.0045), and every drive hash, gate, clamp, clip, state, level
//! and end held.

mod common;

use std::fmt::Write as _;
use std::path::PathBuf;

use common::{fixtures, player, Step};
use hertzforge_hal::Backend;
use hertzforge_twr::service::config::TwrConfig;
use hertzforge_twr::{BackendReferencePlayer, TwrService, TwrState};
use sha2::{Digest, Sha256};

fn sha256_f64(x: &[f64]) -> String {
    let mut h = Sha256::new();
    for v in x {
        h.update(v.to_le_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn opt<T: std::fmt::Debug>(v: Option<T>) -> String {
    match v {
        Some(v) => format!("{v:?}"),
        None => "none".into(),
    }
}

/// A serde value as space-free `key=value` tokens, numbers in shortest
/// round-trip form, object keys sorted (the workspace builds `serde_json`
/// with `preserve_order` and this package alone without, and the trace must
/// read the same either way).
fn flat(key: &str, v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            keys.iter()
                .map(|k| flat(&format!("{key}.{k}"), &m[k.as_str()]))
                .collect::<Vec<_>>()
                .join(" ")
        }
        Value::Number(n) => match n.as_f64() {
            Some(f) if !n.is_u64() && !n.is_i64() => format!("{key}={f:?}"),
            _ => format!("{key}={n}"),
        },
        Value::String(t) => format!("{key}={}", t.replace(' ', "_")),
        other => format!("{key}={other}"),
    }
}

fn run_fixture<B: Backend>(
    out: &mut String,
    tag: &str,
    player: BackendReferencePlayer<B>,
    cfg: TwrConfig,
    script: &[Step],
) {
    let mut svc = TwrService::new(player, cfg).unwrap();
    let mut logged = 0usize;
    let mut sysid_logged = false;
    let mut log = |svc: &TwrService<BackendReferencePlayer<B>>, out: &mut String| {
        let st = svc.status();
        if !sysid_logged {
            if let Some(c) = st.sysid_mean_coherence {
                writeln!(
                    out,
                    "{tag} sysid coherence={c:?} coverage={} lag={} source={}",
                    opt(st.mask_energy_coverage),
                    opt(svc.lag_frames()),
                    opt(svc.lag_provenance())
                )
                .unwrap();
                sysid_logged = true;
            }
        }
        for row in &svc.plays()[logged..] {
            let m = &row.metrics;
            writeln!(
                out,
                "{tag} play={} kind={} level_db={:?} rung={} iteration={} rms_pct={:?} \
                 peak_pct={:?} spectral_db={:?} response_rms={:?} drive_rms={:?} gated={} \
                 clamped={} clipped={} start={} frames={} state={:?} drive_sha={}",
                row.index,
                row.kind.as_str(),
                row.level_db,
                opt(row.rung),
                opt(row.iteration),
                m.rms_error_pct,
                m.peak_error_pct,
                m.spectral_error_db,
                m.response_rms,
                m.drive_rms,
                row.gated,
                row.clamped_bins,
                row.clipped_samples,
                row.start_frame,
                row.frames,
                st.state,
                sha256_f64(svc.drive_time()),
            )
            .unwrap();
        }
        logged = svc.plays().len();
    };
    let terminal = |s: TwrState| {
        matches!(
            s,
            TwrState::Complete | TwrState::Aborted | TwrState::Stopped | TwrState::Holding
        )
    };
    for step in script {
        match *step {
            Step::Advance => {
                let r = svc.advance();
                log(&svc, out);
                if let Err(e) = r {
                    writeln!(out, "{tag} error={}", e.to_string().replace(' ', "_")).unwrap();
                }
            }
            Step::UntilGated(max) => {
                for _ in 0..max {
                    let r = svc.advance();
                    log(&svc, out);
                    r.unwrap();
                    if svc.status().converged_at_level || terminal(svc.state()) {
                        break;
                    }
                }
            }
            Step::ToEnd(max) => {
                for _ in 0..max {
                    if terminal(svc.state()) {
                        break;
                    }
                    let r = svc.advance();
                    log(&svc, out);
                    r.unwrap();
                }
            }
            Step::Up => {
                let r = svc.step_level_up();
                writeln!(
                    out,
                    "{tag} up ok={} level_db={:?}",
                    r.is_ok(),
                    svc.status().level_db
                )
                .unwrap();
            }
            Step::Jump(i) => {
                let r = svc.jump_to_level(i);
                writeln!(
                    out,
                    "{tag} jump={i} ok={} level_db={:?}",
                    r.is_ok(),
                    svc.status().level_db
                )
                .unwrap();
            }
            Step::Play => {
                let r = svc.begin_playback();
                writeln!(out, "{tag} play_verb ok={}", r.is_ok()).unwrap();
            }
        }
    }
    let st = svc.status();
    let trip = st
        .trip
        .as_ref()
        .map(|t| flat("trip", &serde_json::to_value(t).unwrap()))
        .unwrap_or_else(|| "trip=none".into());
    writeln!(
        out,
        "{tag} end state={:?} plays={} station_frames={} {trip} reason={}",
        st.state,
        svc.plays().len(),
        st.station_frames,
        st.abort_reason
            .as_deref()
            .unwrap_or("none")
            .replace(' ', "_")
    )
    .unwrap();
}

fn record_all() -> String {
    let mut out = String::new();
    for f in fixtures() {
        let tag = f.tag;
        run_fixture(&mut out, tag, player(f.station.backend()), f.cfg, &f.script);
    }
    out
}

fn platform() -> String {
    format!(
        "# recorded on {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("twr_trace.txt")
}

/// Two lines agree when every token does: words exactly, numbers to a
/// relative 1e-9, drive hashes not at all.
fn same_within_rounding(want: &str, got: &str) -> Result<(), String> {
    let (wt, gt): (Vec<&str>, Vec<&str>) = (want.split(' ').collect(), got.split(' ').collect());
    if wt.len() != gt.len() {
        return Err("token counts differ".into());
    }
    for (w, g) in wt.iter().zip(&gt) {
        if w.starts_with("drive_sha=") && g.starts_with("drive_sha=") {
            continue;
        }
        if w == g {
            continue;
        }
        let (wk, wv) = w.split_once('=').unwrap_or(("", w));
        let (gk, gv) = g.split_once('=').unwrap_or(("", g));
        if wk != gk {
            return Err(format!("{w} against {g}"));
        }
        let num = |v: &str| {
            v.trim_start_matches("Some(")
                .trim_end_matches(')')
                .parse::<f64>()
        };
        match (num(wv), num(gv)) {
            (Ok(x), Ok(y)) => {
                let tol = 1e-9 * x.abs().max(y.abs()).max(1e-12);
                if (x - y).abs() > tol {
                    return Err(format!("{w} against {g}"));
                }
            }
            _ => return Err(format!("{w} against {g}")),
        }
    }
    Ok(())
}

#[test]
fn the_run_without_the_seam_is_unchanged() {
    let got = format!("{}\n{}", platform(), record_all());
    let path = golden_path();
    if std::env::var_os("HERTZFORGE_RECORD_TRACE").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
        eprintln!("recorded {}", path.display());
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (record it once with HERTZFORGE_RECORD_TRACE=1)",
            path.display()
        )
    });
    if want.lines().next() == Some(platform().as_str()) {
        if want != got {
            for (i, (w, g)) in want.lines().zip(got.lines()).enumerate() {
                if w != g {
                    panic!("the trace moved at line {}:\n  was {w}\n  now {g}", i + 1);
                }
            }
            panic!(
                "the trace moved: {} lines recorded, {} now",
                want.lines().count(),
                got.lines().count()
            );
        }
        return;
    }
    let (wl, gl): (Vec<&str>, Vec<&str>) = (
        want.lines().skip(1).collect(),
        got.lines().skip(1).collect(),
    );
    assert_eq!(
        wl.len(),
        gl.len(),
        "the trace has a different number of lines"
    );
    for (i, (w, g)) in wl.iter().zip(&gl).enumerate() {
        if let Err(e) = same_within_rounding(w, g) {
            panic!(
                "the trace moved at line {}: {e}\n  was {w}\n  now {g}",
                i + 2
            );
        }
    }
}

#[test]
fn the_rounding_comparison_still_sees_a_real_change() {
    let a = "a play=0 rms_pct=5.0237728630191 gated=true drive_sha=00";
    let b = "a play=0 rms_pct=5.023772863019105 gated=true drive_sha=ff";
    same_within_rounding(a, b).expect("a last-bit difference and another hash are rounding");
    let moved = "a play=0 rms_pct=5.0238 gated=true drive_sha=00";
    assert!(same_within_rounding(a, moved).is_err());
    let flipped = "a play=0 rms_pct=5.0237728630191 gated=false drive_sha=00";
    assert!(same_within_rounding(a, flipped).is_err());
}
