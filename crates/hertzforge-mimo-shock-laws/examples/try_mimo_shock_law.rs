//! Run a MIMO Shock law file on the reference 4 x 4 bench beside its baseline
//! built-in law, and print each pulse: the first thing to do with a new idea.
//!
//! ```text
//! bash scripts/build_laws.sh
//! cargo run --release -p hertzforge-mimo-shock-laws --features reference-plant \
//!     --example try_mimo_shock_law -- <law.wasm> [waveform|srs] [name=value ...]
//! ```
//!
//! The bench is the law mathematics' exact reference plant: four drives, four
//! controls, four coupled modes whose frequencies fall 5 % and whose damping
//! triples by full level, identified exactly at -12 dB. The ladder is -12, -9,
//! -6 and -3 dB, then six pulses at full level. The baseline is `inverse` for a
//! waveform target and `shape` for an SRS target, at their defaults.

use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_law_sdk::shock_mimo::math::{inverse::InverseParams, shape::ShapeParams};
use hertzforge_mimo_shock::{MimoShockLaw, MimoShockLawFactory, TargetKind};
use hertzforge_mimo_shock_laws::parity::bench::{play, Bench, BenchRow};
use hertzforge_mimo_shock_laws::{load_law_file, MathLaw, WasmMimoShockLawFactory};
use std::path::PathBuf;
use std::time::Duration;

const LEVELS: [f64; 10] = [-12.0, -9.0, -6.0, -3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

fn param(arg: &str) -> Result<Param, String> {
    let (key, v) = arg
        .split_once('=')
        .ok_or_else(|| format!("`{arg}` is not name=value"))?;
    let value = match v {
        "true" | "on" => ParamValue::Toggle(true),
        "false" | "off" => ParamValue::Toggle(false),
        _ => match (v.parse::<i64>(), v.parse::<f64>()) {
            (Ok(i), _) if !v.contains('.') => ParamValue::Integer(i),
            (_, Ok(x)) => ParamValue::Number(x),
            _ => ParamValue::Choice(v.into()),
        },
    };
    Ok(Param {
        key: key.into(),
        value,
    })
}

fn print(name: &str, rows: &[BenchRow]) {
    println!("{name}");
    println!("  pulse  level dB  waveform err dB  worst SRS dB  drive rms V (full level)");
    for (k, r) in rows.iter().enumerate() {
        let srs = r
            .worst_srs_db
            .map(|v| format!("{v:12.2}"))
            .unwrap_or_else(|| format!("{:>12}", "-"));
        println!(
            "  {k:5}  {:8.1}  {:15.2}  {srs}  {:10.4}",
            r.level_db, r.waveform_error_db, r.drive_rms_full_level
        );
    }
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or("give the law file's path")?);
    let rest: Vec<String> = args.collect();
    let kind = match rest.first().map(String::as_str) {
        Some("srs") => TargetKind::Srs,
        _ => TargetKind::Waveform,
    };
    let params = rest
        .iter()
        .filter(|a| a.contains('='))
        .map(|a| param(a))
        .collect::<Result<Vec<_>, _>>()?;
    let bench = Bench::four_by_four(kind);
    let host = LawHost::new(HostConfig::default()).map_err(|e| e.to_string())?;
    let law = load_law_file(&host, &path).map_err(|e| e.to_string())?;
    let name = format!("{} {}", law.identity().name, law.identity().version);
    let factory = WasmMimoShockLawFactory::new(law, params, Duration::from_secs(60));
    let mut file = factory
        .build(&bench.setup)
        .map_err(|f| format!("the law was not built: {f:?}"))?;
    let mut baseline: Box<dyn MimoShockLaw> = match kind {
        TargetKind::Waveform => Box::new(MathLaw::inverse(&bench.setup, InverseParams::default())?),
        TargetKind::Srs => Box::new(MathLaw::shape(&bench.setup, ShapeParams::default())?),
    };
    println!(
        "reference 4 x 4 bench, {kind:?} target, target rms {:.3} at full level\n",
        bench.target_rms()
    );
    print(&name, &play(file.as_mut(), &bench, &LEVELS)?);
    println!();
    print(
        &format!("baseline: {}", baseline.name()),
        &play(baseline.as_mut(), &bench, &LEVELS)?,
    );
    Ok(())
}
