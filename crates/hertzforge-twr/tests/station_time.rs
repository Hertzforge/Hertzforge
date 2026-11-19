//! Station time and the plays table on the real player over the simulated
//! station: every play clocks its record AND its exact-zero rest pad, the
//! identification included, and a play a stop cancels clocks the frames it got
//! through and leaves no row.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_twr::player::{PlayObserver, PlayProgress};
use hertzforge_twr::service::config::TwrConfig;
use hertzforge_twr::{
    BackendReferencePlayer, ReferenceWaveform, RunSchedule, TwrError, TwrService, TwrState,
};

const FS: f64 = 4096.0;

fn reference() -> ReferenceWaveform {
    let n = 1024usize;
    let samples: Vec<f64> = (0..n)
        .map(|i| {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            w * (std::f64::consts::TAU * 200.0 * i as f64 / FS).sin()
        })
        .collect();
    ReferenceWaveform::from_samples("burst 200 Hz", FS, samples).unwrap()
}

fn player() -> BackendReferencePlayer<SimBackend> {
    let backend = SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(vec![])
        .feedthrough(1.0)
        .noise_level(1e-6)
        .transport_delay_samples(8)
        .shared_clock(true)
        .seed(0x5747_0001)
        .build();
    let mut table = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    table.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    table.channels.push(drv);
    BackendReferencePlayer::new(
        backend,
        table,
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap()
}

fn config() -> TwrConfig {
    let mut c = TwrConfig::for_reference(reference());
    c.prep.highpass_hz = None;
    c.prep.taper_frac = None;
    c.prep.detrend = None;
    c.pad_pre = 128;
    c.pad_post = 256;
    c.sysid.nfft = 512;
    c.sysid.duration_s = 1.0;
    c.sysid.f_lo_hz = 30.0;
    c.sysid.f_hi_hz = 1500.0;
    c.law.f_lo_hz = 30.0;
    c.law.f_hi_hz = 1500.0;
    c.run = RunSchedule::LoopN(2);
    c
}

/// The pad every play carries on this station, read off an identical player.
fn rest_pad() -> u64 {
    let mut p = player();
    p.open_and_configure().unwrap();
    p.rest_pad_frames() as u64
}

#[test]
fn every_play_clocks_its_record_and_its_rest_pad() {
    let cfg = config();
    let id_len = cfg.sysid.build_excitation(FS).len() as u64;
    let pad = rest_pad();
    assert!(pad > 0);
    let mut svc = TwrService::new(player(), cfg).unwrap();
    assert_eq!(
        svc.status().path_label,
        "host delivery on HERTZFORGE-SIM",
        "the label is the path and the station, nothing else"
    );
    let end = svc.run_to_completion(200).unwrap();
    assert_eq!(end, TwrState::Complete, "events: {:?}", svc.event_log());

    let n = svc.grid().n as u64;
    let rows = svc.plays();
    assert!(rows.len() >= 7, "the ladder and two playback plays");
    assert_eq!(
        rows[0].start_frame,
        id_len + pad,
        "the identification clocks its pad too"
    );
    for w in rows.windows(2) {
        assert_eq!(w[1].start_frame, w[0].start_frame + w[0].frames);
    }
    for r in rows {
        assert_eq!(r.frames, n + pad, "play {}", r.index);
        assert_eq!(r.at_s, r.start_frame as f64 / FS);
    }
    let last = rows.last().unwrap();
    let st = svc.status();
    assert_eq!(st.station_frames, last.start_frame + last.frames);
    assert_eq!(st.station_s, st.station_frames as f64 / FS);
    assert!(st.trip.is_none());
}

#[test]
fn a_cancelled_play_clocks_what_it_got_through_and_has_no_row() {
    let cancel_at = Arc::new(AtomicBool::new(false));
    let p = player();
    let flag = p.cancel_flag();
    let watch = Arc::clone(&cancel_at);
    let observer: PlayObserver = Arc::new(move |prog: PlayProgress| {
        if watch.load(Ordering::Relaxed) && prog.frames_done >= 512 {
            flag.store(true, Ordering::Relaxed);
        }
    });
    let mut cfg = config();
    cfg.schedule.mode = hertzforge_twr::schedule::TwrRunMode::Manual;
    let mut svc = TwrService::new(p.with_play_observer(observer), cfg).unwrap();
    svc.advance().unwrap(); // identification
    svc.advance().unwrap(); // one iteration
    let before = svc.station_frames();
    assert_eq!(svc.plays().len(), 1);

    cancel_at.store(true, Ordering::Relaxed);
    let err = svc.advance().unwrap_err();
    let TwrError::Cancelled {
        frames_done,
        frames_expected,
    } = err
    else {
        panic!("expected a cancel, got {err:?}");
    };
    assert!(frames_done >= 512 && frames_done < frames_expected);
    assert_eq!(svc.state(), TwrState::Stopped);
    assert_eq!(svc.station_frames(), before + frames_done as u64);
    assert_eq!(svc.plays().len(), 1, "a cancelled play is not judged");
    assert!(svc.trip().is_none());
}
