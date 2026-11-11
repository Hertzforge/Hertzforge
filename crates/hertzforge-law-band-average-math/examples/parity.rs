//! Runs the law around the same simulated closed loop as
//! `examples/rc_plate_band_average/parity/make_reference.py` and compares each
//! call's drive CPSD with the Python law's. The loop closes on this law's own
//! drive, so an error in one call carries into the next.
//!
//!     cargo run --release -p hertzforge-law-band-average-math --example parity -- DIR [case ...]

use hertzforge_law_band_average_math::{BandAverage, Frame, Params};
use hertzforge_sdm::{HermitianStack, C64};
use std::collections::HashMap;
use std::fs;
use std::time::Instant;

fn read_f64(path: &str) -> Vec<f64> {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    bytes
        .chunks_exact(8)
        .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

fn read_c64(path: &str) -> Vec<C64> {
    read_f64(path)
        .chunks_exact(2)
        .map(|x| C64::new(x[0], x[1]))
        .collect()
}

/// Each line's relative difference, and the worst entry over the grid relative to
/// `sqrt(P_ii P_jj)` of the reference, which weak drives cannot hide behind.
fn compare(rust: &[C64], python: &[C64], d: usize) -> (Vec<f64>, f64) {
    let n2 = d * d;
    let mut worst_entry = 0.0f64;
    let per_line = rust
        .chunks_exact(n2)
        .zip(python.chunks_exact(n2))
        .map(|(a, b)| {
            let diff: f64 = a.iter().zip(b).map(|(x, y)| (*x - *y).norm_sqr()).sum();
            let norm: f64 = b.iter().map(|y| y.norm_sqr()).sum();
            if diff == 0.0 {
                return 0.0;
            }
            for i in 0..d {
                for j in 0..d {
                    let scale = (b[i * d + i].re.abs() * b[j * d + j].re.abs()).sqrt();
                    let e = (a[i * d + j] - b[i * d + j]).norm();
                    worst_entry = worst_entry.max(if scale > 0.0 { e / scale } else { e });
                }
            }
            if norm > 0.0 {
                (diff / norm).sqrt()
            } else {
                f64::INFINITY
            }
        })
        .collect();
    (per_line, worst_entry)
}

/// Every control channel's response ASD to `drive` through `h`, lines x controls.
fn response_asds(drive: &[C64], h: &[C64], controls: usize, d: usize) -> Vec<f64> {
    let mut out = vec![0.0; drive.len() / (d * d) * controls];
    for (k, p) in drive.chunks_exact(d * d).enumerate() {
        if p.iter().all(|x| *x == C64::ZERO) {
            continue;
        }
        for c in 0..controls {
            let row = &h[(k * controls + c) * d..(k * controls + c + 1) * d];
            let mut y = 0.0;
            for i in 0..d {
                for j in 0..d {
                    y += (row[i] * p[i * d + j] * row[j].conj()).re;
                }
            }
            out[k * controls + c] = y;
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("the reference directory").clone();
    let only: Vec<usize> = args[1..].iter().map(|a| a.parse().unwrap()).collect();

    let manifest: HashMap<String, Vec<String>> = fs::read_to_string(format!("{dir}/manifest.txt"))
        .unwrap()
        .lines()
        .map(|l| {
            let mut words = l.split_whitespace().map(String::from);
            (words.next().unwrap(), words.collect())
        })
        .collect();
    let int = |name: &str| manifest[name][0].parse::<usize>().unwrap();
    let (n_lines, n_controls, n_drives, n_calls) = (
        int("n_lines"),
        int("n_controls"),
        int("n_drives"),
        int("n_calls"),
    );
    let total_frames = int("total_frames");
    let fail_amplitude: f64 = manifest["fail_amplitude"][0].parse().unwrap();
    let frames: Vec<usize> = manifest["frames"]
        .iter()
        .map(|x| x.parse().unwrap())
        .collect();

    let h_id = read_c64(&format!("{dir}/h_id.bin"));
    let h_true = read_c64(&format!("{dir}/h_true.bin"));
    // The law reads the upper triangles, as it does of the Python's full arrays.
    let packed = |name: &str| {
        let full = read_c64(&format!("{dir}/{name}.bin"));
        let mut g = HermitianStack::zeros(n_lines, n_controls);
        for k in 0..n_lines {
            for r in 0..n_controls {
                for c in r..n_controls {
                    g.set(k, r, c, full[(k * n_controls + r) * n_controls + c]);
                }
            }
        }
        g
    };
    let spec = packed("spec");
    let sysid = packed("sysid");
    let coherence = read_f64(&format!("{dir}/coherence.bin"));
    let response_jitter = read_f64(&format!("{dir}/response_jitter.bin"));
    let output_jitter = read_f64(&format!("{dir}/output_jitter.bin"));
    let estimates: Vec<Vec<C64>> = (1..n_calls)
        .map(|call| read_c64(&format!("{dir}/estimate_{call}.bin")))
        .collect();
    let frequencies: Vec<f64> = (0..n_lines).map(|k| k as f64).collect();
    let n2 = n_drives * n_drives;
    let mut response = vec![C64::ZERO; n_lines * n_controls * n_controls];

    println!("largest relative difference in the drive CPSD against the Python, per case over {n_calls} calls");
    let mut worst_all = 0.0f64;
    for case in 0..int("n_cases") {
        let case_dir = format!("{dir}/case_{case:02}");
        if !only.is_empty() && !only.contains(&case) || fs::metadata(&case_dir).is_err() {
            continue;
        }
        let text = fs::read_to_string(format!("{case_dir}/params.txt")).unwrap();
        let fail: i64 = fs::read_to_string(format!("{case_dir}/fail.txt"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut law =
            BandAverage::new(Params::parse(&text).unwrap(), n_lines, n_controls, n_drives);
        // As the controller runs it natively: the band solves over the cores.
        law.set_threads(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .min(8),
        );

        let mut times = Vec::new();
        let mut lines = Vec::new();
        let mut entries = Vec::new();
        let mut outside = Vec::new();
        let mut response_db = 0.0f64;
        let mut worst_at = 0;
        let mut output: Vec<C64> = Vec::new();
        let mut log = Vec::new();
        for call in 0..n_calls {
            let asd: Vec<f64> = (0..n_lines)
                .flat_map(|k| (0..n_controls).map(move |i| (k, i)))
                .map(|(k, i)| response[(k * n_controls + i) * n_controls + i].re)
                .collect();
            let frame = Frame {
                frequencies: &frequencies,
                specification: Some(&spec),
                transfer_function: if call == 0 {
                    &h_id
                } else {
                    &estimates[call - 1]
                },
                sysid_response_cpsd: Some(&sysid),
                multiple_coherence: Some(&coherence),
                frames: frames[call],
                total_frames,
                last_response_asd: (call > 0).then_some(asd.as_slice()),
                last_output_cpsd: (call > 0).then_some(output.as_slice()),
            };
            let t0 = Instant::now();
            let drive = law.control(&frame).unwrap();
            times.push(t0.elapsed().as_secs_f64());
            log.extend(law.take_log());

            let reference = read_c64(&format!("{case_dir}/drive_{call}.bin"));
            let (per_line, entry) = compare(&drive, &reference, n_drives);
            let (mut line, mut at) = (0.0, 0);
            for (k, &x) in per_line.iter().enumerate() {
                if x > line {
                    line = x;
                    at = k;
                }
            }
            // The same drive error outside the band that holds the call's worst line.
            let (lb, ub) = law.band_edges().unwrap();
            let f = at as f64;
            let band = lb.iter().zip(ub).find(|(l, u)| **l <= f && f < **u);
            let elsewhere = per_line
                .iter()
                .enumerate()
                .filter(|(k, _)| band.is_none_or(|(l, u)| (*k as f64) < *l || *k as f64 >= *u))
                .map(|x| *x.1)
                .fold(0.0, f64::max);
            if lines.iter().all(|&x: &f64| line > x) {
                worst_at = at;
            }
            lines.push(line);
            entries.push(entry);
            outside.push(elsewhere);

            // What the next call measures, through the softened article. The law reads
            // only the response CPSD's diagonal, so only that is written.
            let asds = response_asds(&drive, &h_true, n_controls, n_drives);
            let reference_asds = response_asds(&reference, &h_true, n_controls, n_drives);
            for (y, r) in asds.iter().zip(&reference_asds) {
                if *y > 0.0 && *r > 0.0 {
                    response_db = response_db.max((10.0 * (y / r).log10()).abs());
                }
            }
            for k in 0..n_lines {
                for c in 0..n_controls {
                    let mut y = asds[k * n_controls + c] * response_jitter[call * n_lines + k];
                    if c as i64 == fail && matches!(call + 1, 3 | 4) {
                        y *= fail_amplitude * fail_amplitude;
                    }
                    response[(k * n_controls + c) * n_controls + c] = C64::new(y, 0.0);
                }
            }
            output.clone_from(&drive);
            for (p, j) in output
                .chunks_exact_mut(n2)
                .zip(&output_jitter[call * n_lines..])
            {
                for x in p {
                    *x = x.scale(*j);
                }
            }
        }

        let py_kappa = read_f64(&format!("{case_dir}/band_kappa.bin"));
        let rs_kappa = law.band_kappa().unwrap_or(&[]);
        let kappa_note = if py_kappa.len() != rs_kappa.len() {
            format!("kappa lengths {} and {}", rs_kappa.len(), py_kappa.len())
        } else {
            let differ = rs_kappa
                .iter()
                .zip(&py_kappa)
                .filter(|(a, b)| ((*a / *b) - 1.0).abs() > 1e-12)
                .count();
            format!(
                "kappa {}/{} bands equal",
                rs_kappa.len() - differ,
                rs_kappa.len()
            )
        };
        let meta: HashMap<String, Vec<String>> = fs::read_to_string(format!("{case_dir}/meta.txt"))
            .unwrap()
            .lines()
            .map(|l| {
                let mut w = l.split_whitespace().map(String::from);
                (w.next().unwrap(), w.collect())
            })
            .collect();
        let py_times: Vec<f64> = meta["times"].iter().map(|x| x.parse().unwrap()).collect();
        let front_note = match law.front_point() {
            Some((i, n)) => format!(
                ", front {i}/{n} (python {}/{})",
                meta["front"][0], meta["front"][1]
            ),
            None => String::new(),
        };

        let py_log = fs::read_to_string(format!("{case_dir}/log.txt")).unwrap();
        let py_log: Vec<&str> = py_log.lines().collect();
        let same = log
            .iter()
            .zip(&py_log)
            .filter(|(a, b)| a.as_str() == **b)
            .count();
        let differing = log.iter().zip(&py_log).find(|(a, b)| a.as_str() != **b);

        let worst_line = lines.iter().copied().fold(0.0, f64::max);
        let worst_entry = entries.iter().copied().fold(0.0, f64::max);
        worst_all = worst_all.max(worst_line);
        println!("\ncase {case:02}: {}", text.replace('\n', " | "));
        println!(
            "  line-norm {worst_line:.2e}, entry/sqrt(PiiPjj) {worst_entry:.2e} (worst at line {worst_at}); {kappa_note}{front_note}; log {same}/{} of {} lines same",
            log.len(),
            py_log.len()
        );
        println!(
            "  outside the worst line's band {:.2e}; the response through the article differs by at most {response_db:.2e} dB",
            outside.iter().copied().fold(0.0, f64::max)
        );
        if let Some((a, b)) = differing {
            println!("  first differing log line\n    rust   {a}\n    python {b}");
        }
        println!(
            "  per call  {}",
            lines
                .iter()
                .map(|x| format!("{x:.1e}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        println!(
            "  rust s    {}",
            times
                .iter()
                .map(|x| format!("{x:.3}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        println!(
            "  python s  {}",
            py_times
                .iter()
                .map(|x| format!("{x:.3}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    println!("\nlargest over all cases: {worst_all:.2e}");
}
