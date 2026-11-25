//! A MIMO Sine run's record at the largest setup the engine accepts, 24
//! controls, 8 limit channels and 12 drives: `channel_levels` and
//! `control_trace` carry every channel of every update, each value where the
//! reader looks for it.

use hertzforge_mimo_sine::records::TickRecord;
use hertzforge_mimo_sine::{MAX_CONTROLS, MAX_DRIVES, MAX_LIMITS};
use hertzforge_sdm::C64;
use sds_control::mimo_sine::record::{levels_result, trace_result, Kept, TICK_BLOCK};
use sds_control::record::{ArrayValues, ResultArray};

const NC: usize = 24;
const NL: usize = 8;
const ND: usize = 12;

/// A value no two channels or updates share: update `t`, channel `c`.
fn tag(t: usize, c: usize) -> f64 {
    1.0 + t as f64 + 0.01 * c as f64
}

fn kept() -> Kept {
    let mut k = Kept::new();
    for t in 0..TICK_BLOCK {
        let mut r = TickRecord::empty(NC, ND, NL);
        r.index = t as u64;
        r.time_s = 0.01 * t as f64;
        for c in 0..NC {
            r.control[c] = C64::new(tag(t, c), 0.0);
            r.reference[c] = C64::new(2.0 * tag(t, c), 0.0);
            r.judged_reference[c] = r.reference[c];
            r.error_db[c] = -tag(t, c);
        }
        for j in 0..NL {
            r.limit[j] = C64::new(tag(t, NC + j), 0.0);
            r.limit_bound[j] = 3.0 * tag(t, NC + j);
        }
        for j in 0..ND {
            r.drive[j] = C64::new(tag(t, NC + NL + j), 0.0);
            r.drive_asked[j] = C64::new(0.0, tag(t, NC + NL + j));
            r.drive_in_force[j] = C64::new(-tag(t, NC + NL + j), 0.0);
        }
        k.ticks.push(r);
        k.pass.push(0.0);
    }
    k
}

fn array<'a>(arrays: &'a [ResultArray], name: &str) -> (&'a [usize], &'a [f64]) {
    let a = arrays
        .iter()
        .find(|a| a.name == name)
        .unwrap_or_else(|| panic!("no array {name}"));
    match &a.values {
        ArrayValues::F64(v) => (&a.shape, v),
        ArrayValues::C128(_) => panic!("{name} is complex"),
    }
}

#[test]
fn mimo_sine_the_record_carries_24_controls_8_limits_and_12_drives() {
    assert_eq!((MAX_CONTROLS, MAX_LIMITS, MAX_DRIVES), (NC, NL, ND));
    let k = kept();
    let n = TICK_BLOCK;
    let channels: Vec<(usize, String)> = (1..=NC + NL + ND).map(|i| (i, "g".to_owned())).collect();
    let (_, d, arrays) = levels_result(&k, &channels, NC, NL, 0.01);
    assert_eq!(
        d["roles"],
        serde_json::json!({"controls": NC, "limits": NL, "drives": ND})
    );
    let (shape, values) = array(&arrays, "values");
    assert_eq!(shape, &[NC + NL + ND, n]);
    for c in 0..NC + NL + ND {
        for t in 0..n {
            assert_eq!(values[c * n + t], tag(t, c), "channel {c}, update {t}");
        }
    }
    let (shape, reference) = array(&arrays, "reference");
    assert_eq!(shape, &[NC, n]);
    let (_, error) = array(&arrays, "error_db");
    for c in 0..NC {
        assert_eq!(reference[c * n + 5], 2.0 * tag(5, c));
        assert_eq!(error[c * n + 5], -tag(5, c));
    }
    let (shape, bound) = array(&arrays, "limit_bound");
    assert_eq!(shape, &[NL, n]);
    assert_eq!(bound[(NL - 1) * n + 3], 3.0 * tag(3, NC + NL - 1));

    let drives: Vec<usize> = (NC + NL + 1..=NC + NL + ND).collect();
    let (_, _, arrays) = trace_result(&k, &drives, &[]);
    for (name, part) in [
        ("drive_re", 1.0),
        ("drive_asked_im", 1.0),
        ("drive_in_force_re", -1.0),
    ] {
        let (shape, v) = array(&arrays, name);
        assert_eq!(shape, &[ND, n], "{name}");
        for j in 0..ND {
            for t in 0..n {
                assert_eq!(v[j * n + t], part * tag(t, NC + NL + j), "{name} drive {j}");
            }
        }
    }
}
