//! The drive file and its checks before arming: the layout, the hash, the zero ends
//! against the shock compensation gate's own integrator, and the setup's refusals.

mod common;

use common::*;
use hertzforge_playback::file::write_folder;
use hertzforge_playback::plan::refusals;
use hertzforge_playback::{check_file, CheckedDrive, DriveFile, StationOffer};

fn offer(rate: Option<f64>, outputs: usize) -> StationOffer {
    StationOffer {
        rate_hz: rate,
        nearest_hz: rate.map(|r| vec![r]).unwrap_or_default(),
        outputs,
    }
}

fn reasons(e: &[hertzforge_playback::FileError]) -> Vec<&'static str> {
    e.iter().map(|x| x.reason).collect()
}

#[test]
fn a_burst_passes_and_its_numbers_are_reported() {
    let dir = temp("pass");
    let folder = write(&dir, 4096.0, &[burst(4096.0, 2.0, 80.0, 0.8)]);
    let file = DriveFile::open(&folder).unwrap();
    assert_eq!(file.samples, 8192);
    let check = check_file(&file, None).unwrap();
    let c = &check.channels[0];
    assert!(c.pass(), "{:?}", c.faults);
    assert!((c.peak_v - 0.8).abs() < 1e-6);
    assert!(c.velocity_end.abs() <= 0.01 * c.velocity_peak);
    assert!(c.displacement_end.abs() <= 0.01 * c.displacement_peak);
    // a few hundred points of envelope, whatever the length
    assert!(c.drive.min.len() <= 200 && c.drive.min.len() >= 100);
}

#[test]
fn the_zero_ends_check_agrees_with_the_shock_gates_integrator() {
    // A half-sine pulse and a burst, short records, through the streaming check and
    // through `pulse_kinematics` taken as g: the same integration, so the residuals and
    // peaks agree once the unit is taken out.
    let fs = 10_240.0;
    let mut pulse = hertzforge_dsp::shock::pulse::half_sine(1.0, 11.0, fs);
    pulse.extend(std::iter::repeat_n(0.0, 400));
    for signal in [pulse, burst(fs, 0.25, 300.0, 1.0)] {
        let dir = temp("agree");
        let folder = write(&dir, fs, std::slice::from_ref(&signal));
        let file = DriveFile::open(&folder).unwrap();
        let c = check_file(&file, None).unwrap().channels.remove(0);
        let k = hertzforge_dsp::shock::pulse_kinematics(&signal, fs);
        let g = hertzforge_dsp::integrate::G_TO_IN_S2;
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * b.abs().max(1.0) + 1e-12;
        assert!(
            close(c.velocity_end, k.residual_velocity_ips / g),
            "{} {}",
            c.velocity_end,
            k.residual_velocity_ips / g
        );
        assert!(close(c.velocity_peak, k.peak_velocity_ips / g));
        assert!(close(c.displacement_end, k.residual_disp_in / g));
        let dpk = k.peak_disp_in.abs().max(k.min_disp_in.abs());
        assert!(close(c.displacement_peak, dpk / g));
        assert!(close(c.peak_v, k.peak_accel_g));
    }
}

#[test]
fn refuses_ends_not_at_zero() {
    // A plain sine of whole cycles starts and ends at 0 V and ends at zero velocity,
    // but its displacement walks away: refused on displacement, with the numbers.
    let fs = 2048.0;
    let sine: Vec<f64> = (0..2048)
        .map(|k| 0.5 * (2.0 * std::f64::consts::PI * 16.0 * k as f64 / fs).sin())
        .collect();
    let step: Vec<f64> = vec![0.2; 2048];
    let dir = temp("ends");
    let folder = write(&dir, fs, &[sine, step]);
    let file = DriveFile::open(&folder).unwrap();
    let check = check_file(&file, None).unwrap();
    let t = table(1, 2, None);
    let r = refusals(
        &file,
        &check,
        &setup(vec![2, 3], 0.0, 5.0),
        &rows(&t),
        &offer(None, 2),
    );
    assert_eq!(reasons(&r), vec!["ends_not_zero"]);
    let words = &r[0].message;
    assert!(
        words.contains("channel 0 (d0)") && words.contains("displacement"),
        "{words}"
    );
    assert!(
        words.contains("channel 1 (d1) starts at 0.200000 V"),
        "{words}"
    );
    let c0: Vec<_> = check.channels[0]
        .faults
        .iter()
        .map(|f| f.reason())
        .collect();
    // its last sample is one step short of the cycle's end, and its displacement walks
    assert_eq!(c0, vec!["end_not_zero", "displacement_not_zero"]);
    // and nothing arms on it
    let e = CheckedDrive::new(
        file,
        check,
        setup(vec![2, 3], 0.0, 5.0),
        &rows(&t),
        &offer(None, 2),
    )
    .unwrap_err();
    assert_eq!(reasons(&e), vec!["ends_not_zero"]);
}

#[test]
fn an_all_zero_channel_passes_with_a_note() {
    let dir = temp("zero");
    let folder = write(
        &dir,
        1024.0,
        &[burst(1024.0, 1.0, 50.0, 0.3), vec![0.0; 1024]],
    );
    let file = DriveFile::open(&folder).unwrap();
    let check = check_file(&file, None).unwrap();
    assert!(check.ends_pass());
    assert!(check.channels[1].note.is_some());
}

#[test]
fn refuses_an_unchecked_or_changed_file() {
    let dir = temp("changed");
    let folder = write(&dir, 1024.0, &[burst(1024.0, 1.0, 50.0, 0.3)]);
    let file = DriveFile::open(&folder).unwrap();
    let check = check_file(&file, None).unwrap();
    let t = table(1, 1, None);
    // A setup that pins another hash is refused before a sample is read.
    let e = check_file(&file, Some(&"0".repeat(64))).unwrap_err();
    assert_eq!(e.reason, "drive_pin_mismatch");
    // A check taken on another file does not arm this one.
    let other = write(&temp("other"), 1024.0, &[burst(1024.0, 1.0, 60.0, 0.3)]);
    let other_check = check_file(&DriveFile::open(&other).unwrap(), None).unwrap();
    let e = CheckedDrive::new(
        file.clone(),
        other_check,
        setup(vec![2], -6.0, 1.0),
        &rows(&t),
        &offer(None, 1),
    )
    .unwrap_err();
    assert_eq!(reasons(&e), vec!["drive_unchecked"]);
    // Samples changed under the sidecar: the hash no longer matches.
    let bin = folder.join("drive.bin");
    let mut bytes = std::fs::read(&bin).unwrap();
    bytes[100] ^= 0x40;
    std::fs::write(&bin, &bytes).unwrap();
    let e = check_file(&DriveFile::open(&folder).unwrap(), None).unwrap_err();
    assert_eq!(e.reason, "drive_hash_mismatch");
    let _ = check;
}

#[test]
fn refuses_a_malformed_file() {
    let dir = temp("bad");
    let folder = dir.join("drive");
    write_folder(
        &folder,
        1000.0,
        &["a", "b"],
        &[0.0; 10],
        serde_json::Map::new(),
    )
    .unwrap();
    // one sample short
    let bin = folder.join("drive.bin");
    let bytes = std::fs::read(&bin).unwrap();
    std::fs::write(&bin, &bytes[..bytes.len() - 8]).unwrap();
    assert_eq!(DriveFile::open(&folder).unwrap_err().reason, "drive_size");
    // a non-finite value
    let mut s = vec![0.0f64; 10];
    s[7] = f64::NAN;
    write_folder(&folder, 1000.0, &["a", "b"], &s, serde_json::Map::new()).unwrap();
    let e = check_file(&DriveFile::open(&folder).unwrap(), None).unwrap_err();
    assert_eq!(e.reason, "drive_not_finite");
    assert!(
        e.message.contains("channel 1") && e.message.contains("sample 3"),
        "{}",
        e.message
    );
    // another unit
    let json = folder.join("drive.json");
    let text = std::fs::read_to_string(&json)
        .unwrap()
        .replacen("\"V\"", "\"g\"", 1);
    std::fs::write(&json, text).unwrap();
    assert_eq!(DriveFile::open(&folder).unwrap_err().reason, "drive_unit");
}

#[test]
fn refuses_a_peak_over_the_ceiling() {
    let dir = temp("peak");
    let folder = write(&dir, 1024.0, &[burst(1024.0, 1.0, 50.0, 2.0)]);
    let file = DriveFile::open(&folder).unwrap();
    let check = check_file(&file, None).unwrap();
    let t = table(1, 1, None);
    // 2 V at -6 dB is 1.0024 V, over a 1 V ceiling
    let r = refusals(
        &file,
        &check,
        &setup(vec![2], -6.0, 1.0),
        &rows(&t),
        &offer(None, 1),
    );
    assert_eq!(reasons(&r), vec!["peak_over_ceiling"]);
    assert!(
        r[0].message.contains("2.000000 V") && r[0].message.contains("1 V"),
        "{}",
        r[0].message
    );
    // at -7 dB it fits
    let r = refusals(
        &file,
        &check,
        &setup(vec![2], -7.0, 1.0),
        &rows(&t),
        &offer(None, 1),
    );
    assert!(r.is_empty(), "{r:?}");
    // a ceiling over the row's range
    let r = refusals(
        &file,
        &check,
        &setup(vec![2], -20.0, 12.0),
        &rows(&t),
        &offer(None, 1),
    );
    assert_eq!(reasons(&r), vec!["ceiling_over_range"]);
}

#[test]
fn refuses_a_map_to_a_missing_or_repeated_drive_row() {
    let dir = temp("map");
    let b = burst(1024.0, 1.0, 50.0, 0.5);
    let folder = write(&dir, 1024.0, &[b.clone(), b]);
    let file = DriveFile::open(&folder).unwrap();
    let check = check_file(&file, None).unwrap();
    let t = table(2, 2, None); // inputs 1, 2; drives 3, 4
    let r = |map: Vec<u32>, outputs| {
        reasons(&refusals(
            &file,
            &check,
            &setup(map, 0.0, 1.0),
            &rows(&t),
            &offer(None, outputs),
        ))
    };
    assert_eq!(r(vec![3, 4], 2), Vec::<&str>::new());
    assert_eq!(r(vec![3, 1], 2), vec!["map_missing_row"]);
    assert_eq!(r(vec![3, 9], 2), vec!["map_missing_row"]);
    assert_eq!(r(vec![4, 4], 2), vec!["map_repeated_row"]);
    assert_eq!(r(vec![3], 2), vec!["map_length"]);
    assert_eq!(r(vec![3, 4], 1), vec!["too_many_drives"]);
}

#[test]
fn refuses_a_rate_mismatch_naming_both_rates() {
    let dir = temp("rate");
    let folder = write(&dir, 25_600.0, &[burst(25_600.0, 0.5, 200.0, 0.5)]);
    let file = DriveFile::open(&folder).unwrap();
    let check = check_file(&file, None).unwrap();
    let t = table(1, 1, None);
    let r = refusals(
        &file,
        &check,
        &setup(vec![2], 0.0, 1.0),
        &rows(&t),
        &offer(Some(25_641.0), 1),
    );
    assert_eq!(reasons(&r), vec!["rate_mismatch"]);
    assert!(
        r[0].message.contains("25600 Hz") && r[0].message.contains("25641 Hz"),
        "{}",
        r[0].message
    );
}

#[test]
fn a_minute_long_tone_of_whole_cycles_passes_the_ends_check() {
    // 60 s at 12 800 Hz, 710 Hz under a whole-record window, made in f64: at rest at both
    // ends. Stored in f32 its rounding, integrated twice over 768 000 samples, ends the
    // displacement at about 15 % of its peak; stored as written it passes.
    let fs = 12_800.0;
    let dir = temp("long-tone");
    let folder = write(&dir, fs, &[burst(fs, 60.0, 710.0, 0.5)]);
    let file = DriveFile::open(&folder).unwrap();
    assert_eq!(file.samples, 768_000);
    let c = check_file(&file, None).unwrap().channels.remove(0);
    assert!(c.pass(), "{}", c.words());
    assert!(c.displacement_end.abs() <= 1e-3 * c.displacement_peak);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_golden_folder_fix_ends_wrote_passes_the_check() {
    // `schema/golden/drive/fixed/`: written by the converter's `fix_ends` from a drive
    // whose ends are not at rest (`python schema/make_golden_drive.py`). What the
    // converter writes, this service passes, and its ends are at rest far inside the
    // allowance.
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden/drive/fixed");
    let file = DriveFile::open(&folder).unwrap();
    let check = check_file(&file, None).unwrap();
    assert_eq!(check.channels.len(), 2);
    assert_eq!(file.sidecar["fixes"][0]["kind"], "fix_ends");
    for c in &check.channels {
        assert!(c.pass(), "{}", c.words());
        assert_eq!((c.start_v, c.end_v), (0.0, 0.0));
        assert!(c.velocity_end.abs() <= 1e-6 * c.velocity_peak, "{c:?}");
        assert!(c.displacement_end.abs() <= 1e-6 * c.displacement_peak, "{c:?}");
    }
}

#[test]
fn the_ceiling_is_judged_on_the_peak_as_the_drive_path_plays_it() {
    // The file holds f64 and the drive path plays f32. A peak that fits the ceiling in
    // f64 but rounds over it in f32 would pass configure and then trip the ceiling at
    // the first sample played; configure refuses it instead.
    let up = 0.5 + 0.75 * f64::from(f32::EPSILON) / 2.0;
    assert!(f64::from(up as f32) > up);
    let ceiling = (up + f64::from(up as f32)) / 2.0;
    let dir = temp("rounds-up");
    let mut signal = burst(1024.0, 1.0, 50.0, up);
    let k = (0..signal.len())
        .max_by(|a, b| signal[*a].abs().total_cmp(&signal[*b].abs()))
        .unwrap();
    signal[k] = up.copysign(signal[k]);
    let folder = write(&dir, 1024.0, &[signal]);
    let file = DriveFile::open(&folder).unwrap();
    let check = check_file(&file, None).unwrap();
    assert_eq!(check.channels[0].peak_v, up);
    let t = table(1, 1, None);
    let r = refusals(
        &file,
        &check,
        &setup(vec![2], 0.0, ceiling),
        &rows(&t),
        &offer(None, 1),
    );
    assert_eq!(reasons(&r), vec!["peak_over_ceiling"], "{r:?}");
}
