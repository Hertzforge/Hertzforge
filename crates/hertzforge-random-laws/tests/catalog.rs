//! The catalog end to end on a scratch data root: what it lists and in what
//! order, importing a law file by path, files in the laws folder it cannot
//! use, resolving a choice of law and its parameters, and check records with
//! the rule the arming gate applies.

mod common;

use common::*;
use hertzforge_law_host::{law_file_sha256, HostConfig, LawHost, Param, ParamValue};
use hertzforge_random::HinvConfig;
use hertzforge_random_laws::{
    may_arm_on_hardware, ArmRefusal, BuiltinLaw, CatalogError, CheckCase, CheckRecord, CheckStatus,
    ChoiceError, LawCatalog, LawChoice, LawWorld, DEFAULT_LAW_ID, LAWS_DIR,
};
use std::path::Path;

fn open(root: &Path) -> LawCatalog {
    let host = LawHost::new(HostConfig::default()).expect("the host starts");
    LawCatalog::with_host(host, root).expect("the catalog opens")
}

fn wasm_files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".wasm"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn built_in_laws_come_first_then_imported_laws_by_name() {
    let scratch = Scratch::new("catalog-list");
    let mut cat = open(scratch.path());
    let ids: Vec<String> = cat.entries().into_iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        ["hinv", "ratio"],
        "a fresh data root lists the built-ins"
    );
    assert_eq!(DEFAULT_LAW_ID, "hinv");
    for e in cat.entries() {
        assert!(e.builtin);
        assert_eq!(e.check, CheckStatus::BuiltIn);
        assert!(e.identity.sha256.is_empty());
        assert_eq!(e.manifest, BuiltinLaw::from_id(&e.id).unwrap().manifest());
    }
    assert!(
        !cat.laws_dir().exists(),
        "listing creates no folder in the data root"
    );

    let ratio_bytes = std::fs::read(law_file(RATIO)).unwrap();
    let ratio = cat.import(&law_file(RATIO)).expect("ratio imports");
    let fixture = cat
        .import(&law_file(MISBEHAVE))
        .expect("the fixture imports");
    assert_eq!(ratio.id, law_file_sha256(&ratio_bytes));
    assert_eq!(ratio.identity.sha256, ratio.id);
    assert!(!ratio.builtin);
    assert_eq!(ratio.check, CheckStatus::Unchecked);
    assert_eq!(cat.laws_dir(), scratch.path().join(LAWS_DIR));
    let stored = cat.laws_dir().join(format!("{}.wasm", ratio.id));
    assert_eq!(
        std::fs::read(&stored).unwrap(),
        ratio_bytes,
        "copied byte for byte"
    );

    let names: Vec<(String, bool)> = cat
        .entries()
        .into_iter()
        .map(|e| (e.manifest.name, e.builtin))
        .collect();
    assert_eq!(
        names,
        [
            ("H-inverse".to_string(), true),
            ("per-line ratio".to_string(), true),
            ("misbehaving fixture".to_string(), false),
            ("per-line ratio".to_string(), false),
        ]
    );

    // Importing the same file again copies nothing and returns the same entry.
    let before = wasm_files(cat.laws_dir());
    assert_eq!(cat.import(&law_file(RATIO)).unwrap(), ratio);
    assert_eq!(wasm_files(cat.laws_dir()), before);
    assert_eq!(before.len(), 2);

    // Another catalog on the same data root finds both.
    let again = open(scratch.path());
    assert_eq!(again.entries(), cat.entries());
    assert!(again.problems().is_empty());
    assert_eq!(again.entry(&fixture.id), Some(fixture));
}

#[test]
fn what_is_not_a_law_is_not_imported() {
    let scratch = Scratch::new("catalog-refuse");
    let mut cat = open(scratch.path());
    let text = scratch.path().join("notes.wasm");
    std::fs::write(&text, b"not a component").unwrap();
    for path in [law_file(BAD_MANIFEST), text] {
        match cat.import(&path) {
            Err(CatalogError::NotALaw { .. }) => {}
            other => panic!("{}: {other:?}", path.display()),
        }
    }
    assert!(matches!(
        cat.import(scratch.path()),
        Err(CatalogError::Io { .. })
    ));
    assert!(matches!(
        cat.import(&scratch.path().join("missing.wasm")),
        Err(CatalogError::Io { .. })
    ));
    assert!(
        wasm_files(cat.laws_dir()).is_empty(),
        "nothing was copied in"
    );
    assert_eq!(cat.entries().len(), 2);
}

#[test]
fn files_in_the_laws_folder_that_cannot_be_used_are_named() {
    let scratch = Scratch::new("catalog-problems");
    let dir = scratch.path().join(LAWS_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    let ratio = std::fs::read(law_file(RATIO)).unwrap();
    let fixture = std::fs::read(law_file(MISBEHAVE)).unwrap();
    let bad = std::fs::read(law_file(BAD_MANIFEST)).unwrap();
    // A law under its own hash: listed.
    std::fs::write(
        dir.join(format!("{}.wasm", law_file_sha256(&ratio))),
        &ratio,
    )
    .unwrap();
    // A law under another name: left out.
    std::fs::write(dir.join("my_law.wasm"), &fixture).unwrap();
    // A law under a hash that is not its own: left out.
    let wrong = "f".repeat(64);
    std::fs::write(dir.join(format!("{wrong}.wasm")), &fixture).unwrap();
    // A file named by its own hash that is not a usable law: left out.
    std::fs::write(dir.join(format!("{}.wasm", law_file_sha256(&bad))), &bad).unwrap();
    // Not a law file at all (a check record, an interrupted copy): ignored.
    std::fs::write(dir.join("notes.txt"), b"x").unwrap();
    std::fs::write(dir.join(".abc.wasm.1.part"), b"x").unwrap();

    let cat = open(scratch.path());
    let imported: Vec<String> = cat
        .entries()
        .into_iter()
        .filter(|e| !e.builtin)
        .map(|e| e.id)
        .collect();
    assert_eq!(imported, [law_file_sha256(&ratio)]);
    let problems = cat.problems();
    assert_eq!(problems.len(), 3, "{problems:#?}");
    let why = |name: &str| {
        problems
            .iter()
            .find(|p| {
                p.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(name)
            })
            .map(|p| p.reason.clone())
            .unwrap_or_else(|| panic!("no problem named for {name}: {problems:#?}"))
    };
    assert!(why("my_law").contains("not named by the SHA-256"));
    assert!(why(&wrong).contains("not the one it is named by"));
    assert!(why(&law_file_sha256(&bad)).contains("manifest"));
}

#[test]
fn a_choice_resolves_to_a_factory_or_says_why_not() {
    let scratch = Scratch::new("catalog-choice");
    let mut cat = open(scratch.path());
    let ratio = cat.import(&law_file(RATIO)).unwrap();
    let num = |k: &str, v: f64| Param::new(k, ParamValue::Number(v));

    // The default law with its defaults leaves the default settings as they are.
    let mut cfg = manual_cfg();
    cat.resolve(&LawChoice::default(), &mut cfg)
        .expect("the default resolves");
    assert_eq!(cfg.hinv, HinvConfig::default());

    // hinv's values land on the run's hinv settings, which the core reads too.
    let mut cfg = manual_cfg();
    let choice = LawChoice::new(
        "hinv",
        vec![
            num("damping", 2.0),
            num("lambda", 5.0),
            Param::new("divergence_n", ParamValue::Integer(12)),
        ],
    );
    cat.resolve(&choice, &mut cfg).expect("hinv resolves");
    assert_eq!(cfg.hinv.damping, 2.0);
    assert_eq!(cfg.hinv.lambda, Some(5.0));
    assert_eq!(cfg.hinv.divergence_n, 12);
    let mut zero = cfg.clone();
    cat.resolve(&LawChoice::new("hinv", vec![num("lambda", 0.0)]), &mut zero)
        .unwrap();
    assert_eq!(zero.hinv.lambda, None, "0 is the suggest policy");
    assert_eq!(
        zero.hinv.damping, 4.0,
        "a value left out is the declared default"
    );

    // Other laws leave the run's settings alone.
    for choice in [
        LawChoice::new("ratio", vec![num("gain", 0.25)]),
        LawChoice::new(ratio.id.clone(), vec![num("max_step_db", 3.0)]),
    ] {
        let mut c = cfg.clone();
        cat.resolve(&choice, &mut c).expect("resolves");
        assert_eq!(c, cfg, "{}", choice.id);
    }

    // Refusals, each typed; nothing in the config changes.
    let refusals: Vec<(LawChoice, &str)> = vec![
        (LawChoice::new("nonesuch", vec![]), "unknown"),
        (
            LawChoice::new("hinv", vec![num("surprise", 1.0)]),
            "surprise",
        ),
        (
            LawChoice::new("hinv", vec![num("damping", 11.0)]),
            "damping",
        ),
        (
            LawChoice::new(
                "hinv",
                vec![Param::new("damping", ParamValue::Toggle(true))],
            ),
            "damping",
        ),
        (
            LawChoice::new(
                "hinv",
                vec![Param::new("divergence_n", ParamValue::Integer(0))],
            ),
            "divergence_n",
        ),
        (LawChoice::new("ratio", vec![num("gain", 2.0)]), "gain"),
        (
            LawChoice::new(ratio.id.clone(), vec![num("gain", -1.0)]),
            "gain",
        ),
    ];
    for (choice, key) in refusals {
        let mut c = cfg.clone();
        let e = match cat.resolve(&choice, &mut c) {
            Err(e) => e,
            Ok(_) => panic!("{choice:?} resolved"),
        };
        match (&e, key) {
            (ChoiceError::UnknownLaw(id), "unknown") => assert_eq!(id, "nonesuch"),
            (ChoiceError::BadParam { key: k, .. }, key) => assert_eq!(k, key, "{e}"),
            _ => panic!("{choice:?}: {e:?}"),
        }
        assert_eq!(c, cfg, "a refused choice changed the config: {e}");
    }
}

#[test]
fn hardware_arms_only_a_built_in_law_or_one_that_passed_its_check() {
    let scratch = Scratch::new("catalog-check");
    let mut cat = open(scratch.path());
    let law = cat.import(&law_file(MISBEHAVE)).unwrap();
    for e in cat.entries().iter().filter(|e| e.builtin) {
        assert_eq!(may_arm_on_hardware(e), Ok(()), "{}", e.id);
    }
    assert_eq!(may_arm_on_hardware(&law), Err(ArmRefusal::Unchecked));

    let case = |name: &str, passed: bool| CheckCase {
        name: name.into(),
        passed,
        detail: String::new(),
    };
    let failed = CheckRecord::new(
        law.id.clone(),
        "0.1.0",
        "2026-11-02T17:00:00Z",
        vec![case("one mode", true), case("two close modes", false)],
    );
    assert!(!failed.passed);
    cat.record_check(&failed).unwrap();
    let entry = cat.entry(&law.id).unwrap();
    assert_eq!(entry.check, CheckStatus::Checked(failed.clone()));
    match may_arm_on_hardware(&entry) {
        Err(ArmRefusal::CheckFailed { failed_cases, .. }) => {
            assert_eq!(failed_cases, ["two close modes"])
        }
        other => panic!("{other:?}"),
    }

    let passed = CheckRecord::new(
        law.id.clone(),
        "0.1.0",
        "2026-11-02T18:00:00Z",
        vec![case("one mode", true), case("two close modes", true)],
    );
    cat.record_check(&passed).unwrap();
    assert_eq!(may_arm_on_hardware(&cat.entry(&law.id).unwrap()), Ok(()));
    // The record is found again from the file alone.
    assert_eq!(
        open(scratch.path()).entry(&law.id).unwrap().check,
        CheckStatus::Checked(passed.clone())
    );

    // A record that says passed while a case failed is refused, on write and on
    // read: a hand-edited verdict cannot clear a law.
    let mut forged = passed.clone();
    forged.cases[1].passed = false;
    assert!(cat.record_check(&forged).is_err());
    let path = CheckRecord::path(cat.laws_dir(), &law.id);
    std::fs::write(&path, serde_json_text(&forged)).unwrap();
    cat.rescan().unwrap();
    let entry = cat.entry(&law.id).unwrap();
    assert!(
        matches!(entry.check, CheckStatus::Unreadable(_)),
        "{:?}",
        entry.check
    );
    assert!(matches!(
        may_arm_on_hardware(&entry),
        Err(ArmRefusal::CheckUnreadable(_))
    ));

    // A passing record for another file does not clear this one.
    let mut other = passed.clone();
    other.law_sha256 = "0".repeat(64);
    std::fs::write(&path, serde_json_text(&other)).unwrap();
    cat.rescan().unwrap();
    assert!(matches!(
        may_arm_on_hardware(&cat.entry(&law.id).unwrap()),
        Err(ArmRefusal::CheckUnreadable(_))
    ));

    // No cases is not a pass.
    assert!(!CheckRecord::new(law.id.clone(), "0.1.0", "2026-11-02T19:00:00Z", vec![]).passed);
}

#[test]
fn a_check_record_is_plain_json_beside_its_law_file() {
    let scratch = Scratch::new("catalog-record");
    let mut cat = open(scratch.path());
    let law = cat.import(&law_file(RATIO)).unwrap();
    let record = CheckRecord::new(
        law.id.clone(),
        "0.1.0",
        "2026-11-02T17:00:00Z",
        vec![CheckCase {
            name: "one mode".into(),
            passed: true,
            detail: "settled in 4 loops".into(),
        }],
    );
    cat.record_check(&record).unwrap();
    let path = scratch
        .path()
        .join(LAWS_DIR)
        .join(format!("{}.check.json", law.id));
    let text = std::fs::read_to_string(&path).expect("the record is beside the law file");
    for key in [
        "\"format\": 1",
        "\"law_sha256\"",
        "\"tool_version\": \"0.1.0\"",
        "\"date\": \"2026-11-02T17:00:00Z\"",
        "\"passed\": true",
        "\"cases\"",
        "\"settled in 4 loops\"",
    ] {
        assert!(text.contains(key), "{key} in {text}");
    }
    assert_eq!(CheckRecord::read(cat.laws_dir(), &law.id), Ok(Some(record)));
    assert_eq!(CheckRecord::read(cat.laws_dir(), &"0".repeat(64)), Ok(None));
}

/// A record as JSON, bypassing the checks `CheckRecord::write` makes.
fn serde_json_text(r: &CheckRecord) -> String {
    let cases: Vec<String> = r
        .cases
        .iter()
        .map(|c| {
            format!(
                r#"{{"name":"{}","passed":{},"detail":"{}"}}"#,
                c.name, c.passed, c.detail
            )
        })
        .collect();
    format!(
        r#"{{"format":{},"law_sha256":"{}","tool_version":"{}","date":"{}","passed":{},"cases":[{}]}}"#,
        r.format,
        r.law_sha256,
        r.tool_version,
        r.date,
        r.passed,
        cases.join(",")
    )
}

#[test]
fn classical_shock_laws_have_their_own_list_the_built_in_first() {
    let root = std::env::temp_dir().join(format!(
        "hertzforge-random-laws-classical-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let mut cat = open(&root);
    let list = cat.shock_classical_entries();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "fd_correct");
    assert_eq!(list[0].world, LawWorld::ShockClassical);
    assert_eq!(
        list[0].manifest,
        hertzforge_random_laws::classical_builtin_manifest()
    );
    assert_eq!(
        cat.entry("fd_correct").unwrap().world,
        LawWorld::ShockClassical
    );
    assert_eq!(LawWorld::ShockClassical.as_str(), "shock_classical");

    // An imported classical law file lists with the classical laws only.
    let entry = cat
        .import(&law_file("hertzforge_law_shock_fdc"))
        .expect("the classical law file imports");
    assert_eq!(entry.world, LawWorld::ShockClassical);
    let ids = |v: Vec<hertzforge_random_laws::CatalogEntry>| -> Vec<String> {
        v.into_iter().map(|e| e.id).collect()
    };
    assert_eq!(
        ids(cat.shock_classical_entries()),
        ["fd_correct".to_string(), entry.id.clone()]
    );
    assert!(!ids(cat.entries()).contains(&entry.id));
    assert!(!ids(cat.mimo_entries()).contains(&entry.id));
    assert!(cat.classical_law(&entry.id).is_some());
    // A Random choice of it is refused by its world.
    let mut cfg = ladder_cfg();
    assert!(matches!(
        cat.resolve(&LawChoice::new(entry.id.clone(), Vec::new()), &mut cfg),
        Err(ChoiceError::WrongTestType {
            law_is_for: "classical shock",
            ..
        })
    ));
    let _ = std::fs::remove_dir_all(root);
}
