//! The bench's passive 4 x 4 network: its design figures and its computed transfer
//! matrix, from the component values in `hertzforge_hal_sim::bench_network`.
//!
//! ```text
//! cargo run -p hertzforge-hal-sim --example bench_network -- [--csv <file>] \
//!     [--fs <Hz>] [--nfft <points>]
//! ```
//!
//! Prints the corners, the mode, the worst input voltage and output current at
//! 10 V, the worst condition number in band, and the matrix (dB and degrees) at a
//! few frequencies. With `--csv`, writes every line of the matrix on the bench's
//! grid (default 12.8 kHz and 4096 points, 3.125 Hz lines) as
//! `freq_hz,input,output,re,im`, the analytic matrix a bench identification is
//! compared with. The same stack is what `BenchNetwork::plant` plays on the
//! simulator.

use hertzforge_hal_sim::bench_network::{BenchNetwork, BAND_HZ};
use std::io::Write;

fn main() {
    let mut csv: Option<String> = None;
    let (mut fs, mut nfft) = (12_800.0f64, 4096usize);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut next = || args.next().expect("a value after the flag");
        match a.as_str() {
            "--csv" => csv = Some(next()),
            "--fs" => fs = next().parse().expect("--fs is a number"),
            "--nfft" => nfft = next().parse().expect("--nfft is a whole number"),
            other => panic!("unknown argument {other}"),
        }
    }
    let n = BenchNetwork::nominal();
    let (lo, hi) = BAND_HZ;
    let freqs: Vec<f64> = (0..400)
        .map(|k| lo * (hi / lo).powf(k as f64 / 399.0))
        .collect();
    let (f0, q) = n.tank_resonance();
    println!("Path corners (Hz):");
    for j in 0..4 {
        match n.corner_hz(j) {
            Some(c) if n.c_series[j].is_some() => println!("  output {j}: {c:.1} (high-pass)"),
            Some(c) => println!("  output {j}: {c:.1} (low-pass)"),
            None => println!("  output {j}: none"),
        }
    }
    println!("Mode on output {}: {f0:.1} Hz, Q {q:.2}", n.tank.drive);
    println!(
        "At 10 V on every output: worst input {:.3} V, worst output current {:.3} mA",
        n.max_input_voltage(10.0, &freqs),
        n.max_output_current(10.0, &freqs) * 1e3
    );
    let mut worst = 0.0f64;
    for &f in &freqs {
        let h = n.frf_at(f);
        let mut s = hertzforge_sdm::MatrixStack::zeros(1, 4, 4);
        for (i, row) in h.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                s.set(0, i, j, *v);
            }
        }
        worst = worst.max(hertzforge_sdm::linalg::condition_numbers(&s).unwrap()[0]);
    }
    println!("Worst condition number in band: {worst:.1}\n");
    for f in [20.0, 100.0, 300.0, 1000.0, 2000.0, f0, 5000.0] {
        println!("{f:.1} Hz (rows: inputs 0-3; columns: outputs 0-3; dB / degrees)");
        for row in n.frf_at(f) {
            let cells: Vec<String> = row
                .iter()
                .map(|v| {
                    format!(
                        "{:7.2} / {:7.1}",
                        20.0 * v.norm().log10(),
                        v.arg().to_degrees()
                    )
                })
                .collect();
            println!("  {}", cells.join("   "));
        }
    }
    if let Some(path) = csv {
        let (lines, df) = (nfft / 2 + 1, fs / nfft as f64);
        let s = n.frf_stack(lines, df);
        let mut out =
            std::io::BufWriter::new(std::fs::File::create(&path).expect("the file opens"));
        writeln!(out, "freq_hz,input,output,re,im").unwrap();
        for line in 0..lines {
            for i in 0..4 {
                for j in 0..4 {
                    let v = s.get(line, i, j);
                    writeln!(out, "{},{i},{j},{:e},{:e}", line as f64 * df, v.re, v.im).unwrap();
                }
            }
        }
        println!("\nWrote {lines} lines at {df} Hz to {path}");
    }
}
