//! Check a multiple-output law file before it drives hardware, from the command line:
//! the same check `laws.check` runs (`sds_control::mimo::law_check`).
//!
//! ```text
//! cargo run --release -p sds-control --example law_check -- <law.wasm> \
//!     [--data-root <folder>] [--param key=value]...
//! ```
//!
//! The file is imported into the data folder's law catalog (the local default unless
//! `--data-root` names another), checked on every simulated test article, and the
//! result recorded against its SHA-256. Exit status 0 when it passed, 1 when it did
//! not, 2 when it could not be checked.

use hertzforge_random_laws::{
    may_arm_on_hardware, mimo_check_record, LawCatalog, LawChoice, LawWorld,
};
use sds_control::mimo::law_check::{control_counts, now_rfc3339, run_check, CheckOptions};
use serde_json::{Map, Value};
use std::path::PathBuf;

const USAGE: &str = "usage: law_check <law.wasm> [--data-root <folder>] [--param key=value]...";

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("law_check: {msg}");
    std::process::exit(2);
}

fn main() {
    let mut file: Option<PathBuf> = None;
    let mut root: Option<PathBuf> = None;
    let mut given = Map::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data-root" => root = Some(args.next().unwrap_or_else(|| fail(USAGE)).into()),
            "--param" => {
                let kv = args.next().unwrap_or_else(|| fail(USAGE));
                let (k, v) = kv.split_once('=').unwrap_or_else(|| fail(USAGE));
                // A number, a boolean, or else the text as given (a choice).
                let value = serde_json::from_str::<Value>(v).unwrap_or(Value::from(v));
                given.insert(k.to_owned(), value);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ if file.is_none() => file = Some(PathBuf::from(&a)),
            _ => fail(USAGE),
        }
    }
    let file = file.unwrap_or_else(|| fail(USAGE));
    let file =
        std::fs::canonicalize(&file).unwrap_or_else(|e| fail(format!("{}: {e}", file.display())));
    let mut cat = match root {
        Some(r) => LawCatalog::open(r),
        None => LawCatalog::open_default(),
    }
    .unwrap_or_else(|e| fail(e));
    let entry = cat.import(&file).unwrap_or_else(|e| fail(e));
    if entry.world != LawWorld::Mimo {
        fail("this is a single-output law; the check runs multiple-output laws");
    }
    let choice = LawChoice::new(
        entry.id.clone(),
        sds_control::mimo::adapter::params_of(&entry.manifest.params, &given),
    );
    let factories: Vec<_> = control_counts()
        .into_iter()
        .map(|n| (n, cat.resolve_mimo(&choice, n).unwrap_or_else(|e| fail(e))))
        .collect();
    println!(
        "{} {} ({})\n",
        entry.identity.name, entry.identity.version, entry.id
    );
    let law = |n: usize| {
        factories
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, f)| f.clone())
            .ok_or_else(|| format!("no law resolved for {n} controls"))
    };
    let cases = run_check(&law, &CheckOptions::default(), &mut |i, case, r| match r {
        None => println!("[{}/6] {} ...", i + 1, case.name()),
        Some(r) => println!(
            "      {}: {}",
            if r.verdict.passed { "PASS" } else { "FAIL" },
            r.verdict.detail
        ),
    });
    let record = mimo_check_record(
        entry.id.clone(),
        now_rfc3339(),
        cases.iter().map(|c| c.verdict.clone()).collect(),
    );
    cat.record_check(&record).unwrap_or_else(|e| fail(e));
    let entry = cat
        .entry(&entry.id)
        .unwrap_or_else(|| fail("the law left the catalog"));
    match may_arm_on_hardware(&entry) {
        Ok(()) => {
            println!("\nPassed: recorded against this file; cleared for hardware.");
        }
        Err(why) => {
            println!("\nNot passed: {why}");
            std::process::exit(1);
        }
    }
}
