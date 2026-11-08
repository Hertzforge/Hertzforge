//! TWR references: each format reads to its record (CSV with a time column, TSV,
//! a single column at a given rate, a chosen data column, WAV PCM16 and float32,
//! the JSON record, pasted text), the SHA-256 is of the record (one record pasted
//! or read from a file, two records from two columns or two units), a stored record
//! loads back and survives two imports at once, a file that changed is told apart,
//! and the preparation refuses a record too long by name.

use hertzforge_twr::ReferenceWaveform;
use sds_control::twr::config;
use sds_control::twr::reference::{self, ImportOptions, ImportRequest, Source, MAX_IMPORT_SAMPLES};
use serde_json::json;
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "sds-twr-ref-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn opts(fs_hz: Option<f64>, column: usize) -> ImportOptions {
    ImportOptions {
        fs_hz,
        column,
        units: None,
    }
}

fn import_file(root: &Path, file: &Path, o: ImportOptions) -> reference::Stored {
    reference::import(
        root,
        &ImportRequest {
            source: Source::Path(file.to_str().unwrap().to_owned()),
            options: o,
            name: None,
        },
    )
    .unwrap_or_else(|e| panic!("{}: {}", file.display(), e.message))
}

fn wav(samples: &[f64], fs: u32, float: bool, channels: u16) -> Vec<u8> {
    let bits: u16 = if float { 32 } else { 16 };
    let bytes = (bits / 8) as usize;
    let mut data = Vec::new();
    for &s in samples {
        for ch in 0..channels {
            let v = if ch == 0 { s } else { -s };
            if float {
                data.extend_from_slice(&(v as f32).to_le_bytes());
            } else {
                data.extend_from_slice(&((v * 32768.0) as i16).to_le_bytes());
            }
        }
    }
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&(if float { 3u16 } else { 1u16 }).to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&fs.to_le_bytes());
    b.extend_from_slice(&(fs * channels as u32 * bytes as u32).to_le_bytes());
    b.extend_from_slice(&(channels * bytes as u16).to_le_bytes());
    b.extend_from_slice(&bits.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    b
}

const S: [f64; 8] = [0.0, 0.25, 0.5, 0.25, 0.0, -0.25, -0.5, -0.25];

#[test]
fn each_format_reads_to_its_record() {
    let root = scratch("formats");
    // CSV with a time column and a header.
    let csv = root.join("r.csv");
    let mut text = String::from("# a comment\ntime,accel\n");
    for (i, v) in S.iter().enumerate() {
        text.push_str(&format!("{},{v}\n", i as f64 / 1000.0));
    }
    std::fs::write(&csv, &text).unwrap();
    let a = import_file(&root, &csv, opts(None, 0));
    assert_eq!(
        (a.format, a.fs_hz, a.samples.clone()),
        ("csv", 1000.0, S.to_vec())
    );
    assert_eq!(a.name, "r");
    // TSV, two data columns at a given rate; the second column is a second record.
    let tsv = root.join("r.tsv");
    let rows: String = S.iter().map(|v| format!("{v}\t{}\n", 2.0 * v)).collect();
    std::fs::write(&tsv, &rows).unwrap();
    let b0 = import_file(&root, &tsv, opts(Some(1000.0), 0));
    let b1 = import_file(&root, &tsv, opts(Some(1000.0), 1));
    assert_eq!(b0.format, "tsv");
    assert_eq!(b0.samples, S.to_vec());
    assert_eq!(b1.samples, S.iter().map(|v| 2.0 * v).collect::<Vec<_>>());
    assert_ne!(b0.sha256, b1.sha256, "two columns are two records");
    assert_eq!(
        b0.sha256, a.sha256,
        "the same samples and rate are one record"
    );
    // WAV, PCM16 and float32, a chosen channel.
    for (float, name) in [(false, "p.wav"), (true, "f.wav")] {
        let f = root.join(name);
        std::fs::write(&f, wav(&S, 8000, float, 2)).unwrap();
        let w = import_file(&root, &f, opts(None, 1));
        assert_eq!(w.format, "wav");
        assert_eq!(w.fs_hz, 8000.0);
        let want: Vec<f64> = S.iter().map(|v| -v).collect();
        assert_eq!(w.samples, want, "{name}");
    }
    // The JSON record carries its rate and unit.
    let mut rec = ReferenceWaveform::from_samples("j", 2048.0, S.to_vec()).unwrap();
    rec.eu = "in/s".into();
    let j = root.join("r.json");
    std::fs::write(&j, rec.to_json()).unwrap();
    let js = import_file(&root, &j, opts(None, 0));
    assert_eq!(
        (js.format, js.fs_hz, js.units.as_str()),
        ("json", 2048.0, "in/s")
    );
    // Pasted text, a single column at a given rate.
    let pasted: String = S.iter().map(|v| format!("{v}\n")).collect();
    let p = reference::import(
        &root,
        &ImportRequest {
            source: Source::Text(pasted),
            options: opts(Some(1000.0), 0),
            name: None,
        },
    )
    .unwrap();
    assert_eq!(
        (p.format, p.name.as_str(), p.path.clone()),
        ("text", "pasted", None)
    );
    assert_eq!(
        p.sha256, a.sha256,
        "pasted and read from a file are one record"
    );
    let facts = p.facts_json();
    assert_eq!(facts["samples"], 8);
    assert_eq!(facts["peak"], 0.5);
    // A stored record loads back whole.
    let back = reference::load(&root, &a.sha256).unwrap();
    assert_eq!(back.samples, a.samples);
    assert_eq!(back.fs_hz, a.fs_hz);
    // Every file it was read from is kept with how: a later paste does not change
    // how r.csv (a time column) is read again.
    assert!(back
        .files
        .contains(&(csv.to_str().unwrap().to_owned(), opts(None, 0))));
    reference::still_matches(&back, csv.to_str().unwrap()).unwrap();
    assert!(reference::still_matches(&back, "/elsewhere/r.csv").is_err());
    assert!(reference::load(&root, &"0".repeat(64)).is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn what_is_not_a_record_is_refused_by_name() {
    let root = scratch("refused");
    for (text, o, words) in [
        ("1\n2\n3\n", opts(None, 0), "time column"),
        ("0,1\n1,2\nx,y\n", opts(None, 0), "line 3"),
        ("0,1\n0.1,2\n0.5,3\n", opts(None, 0), "evenly spaced"),
        ("1,2\n3,4\n", opts(Some(100.0), 5), "data column 5"),
        ("5\n", opts(Some(100.0), 0), "fewer than two"),
    ] {
        let e = reference::import(
            &root,
            &ImportRequest {
                source: Source::Text(text.into()),
                options: o,
                name: None,
            },
        )
        .unwrap_err();
        assert_eq!(e.reason, Some("reference_unreadable"), "{text:?}");
        assert!(e.message.contains(words), "{text:?}: {}", e.message);
        if words == "line 3" {
            assert_eq!(e.detail.as_deref().unwrap()["line"], json!(3));
        }
    }
    let missing = root.join("nope.csv");
    let e = reference::import(
        &root,
        &ImportRequest {
            source: Source::Path(missing.to_str().unwrap().into()),
            options: opts(None, 0),
            name: None,
        },
    )
    .unwrap_err();
    assert_eq!(e.reason, Some("reference_unreadable"));
    assert_eq!(
        e.detail.as_deref().unwrap()["path"],
        json!(missing.to_str().unwrap())
    );
    // The parameters' shapes.
    for (p, param) in [
        (json!({}), "path"),
        (json!({"path": "r.csv"}), "path"),
        (json!({"path": "/r.csv", "text": "1"}), "path"),
        (json!({"text": "1", "fs_hz": 0}), "fs_hz"),
        (json!({"text": "1", "column": -1}), "column"),
        (json!({"text": "1", "units": ""}), "units"),
    ] {
        let e = reference::parse_import(p.as_object().unwrap()).unwrap_err();
        assert_eq!(e.reason, Some("bad_params"), "{p}");
        assert_eq!(e.detail.as_deref().unwrap()["param"], json!(param), "{p}");
    }
    const { assert!(MAX_IMPORT_SAMPLES >= hertzforge_twr::control::MAX_RECORD_LEN) };
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_changed_file_is_told_apart_from_the_record_it_was() {
    let root = scratch("changed");
    let f = root.join("r.csv");
    let text: String = S.iter().map(|v| format!("{v}\n")).collect();
    std::fs::write(&f, &text).unwrap();
    let s = import_file(&root, &f, opts(Some(1000.0), 0));
    let p = f.to_str().unwrap();
    reference::still_matches(&s, p).unwrap();
    // Reformatted but the same samples: still the record.
    std::fs::write(&f, text.replace('\n', "\r\n")).unwrap();
    reference::still_matches(&s, p).unwrap();
    std::fs::write(&f, text.replace("0.5", "0.75")).unwrap();
    let why = reference::still_matches(&s, p).unwrap_err();
    assert!(why.contains("import it again"), "{why}");
    std::fs::remove_file(&f).unwrap();
    assert!(reference::still_matches(&s, p).is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn two_imports_of_one_record_at_once_both_keep_it() {
    let root = scratch("concurrent");
    let text: String = (0..4096)
        .map(|i| format!("{}\n", (i % 17) as f64 / 16.0))
        .collect();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let root = root.clone();
            let text = text.clone();
            std::thread::spawn(move || {
                reference::import(
                    &root,
                    &ImportRequest {
                        source: Source::Text(text),
                        options: opts(Some(1000.0), 0),
                        name: None,
                    },
                )
                .map(|s| s.sha256)
                .map_err(|e| e.message)
            })
        })
        .collect();
    let shas: Vec<String> = handles
        .into_iter()
        .map(|h| h.join().unwrap().unwrap())
        .collect();
    assert!(shas.windows(2).all(|w| w[0] == w[1]));
    assert!(reference::load(&root, &shas[0]).is_some());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_record_past_the_cap_is_refused_by_name() {
    let root = scratch("cap");
    let n = 2048;
    let text: String = (0..n)
        .map(|i| format!("{}\n", (i % 32) as f64 / 64.0))
        .collect();
    let s = reference::import(
        &root,
        &ImportRequest {
            source: Source::Text(text),
            options: opts(Some(12_800.0), 0),
            name: None,
        },
    )
    .unwrap();
    let shape = config::parse_preview(
        json!({"reference": {"sha256": s.sha256}, "pads": {"pre": 600_000, "post": 600_000}})
            .as_object()
            .unwrap(),
    )
    .unwrap();
    let item = reference::prepare(&s, &shape, 12_800.0, (20.0, 2000.0)).unwrap_err();
    assert_eq!(item.reason, "record_too_long");
    let shape = config::parse_preview(
        json!({"reference": {"sha256": s.sha256}})
            .as_object()
            .unwrap(),
    )
    .unwrap();
    let p = reference::prepare(&s, &shape, 12_800.0, (20.0, 2000.0)).unwrap();
    assert_eq!(p.grid.n, 4096);
    assert_eq!(p.target.len(), 4096);
    let _ = std::fs::remove_dir_all(root);
}
