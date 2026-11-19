//! The summed drive ceiling is a limit on the drive as played. On a wire-like
//! plant (gain 1, a pipeline lag of about 0.12 s), a 0.25 g tone under a
//! 0.15 V ceiling is not touched at -12 dB, where it needs 0.063 V; as the
//! ladder climbs the ceiling holds it (the tone is `limiting`), the summed
//! drive never passes the ceiling, and the run ends on the drive peak's abort
//! or the tone's own abort naming the ceiling, never on a divergence. The
//! divergence guard still trips on a law that keeps asking for more against
//! its rate cap with nothing holding it.

mod common;
use common::*;
use hertzforge_hal_sim::SimConfig;
use hertzforge_mixed::{
    parts::MixedLawGuards, MixedConfig, MixedLawFactories, MixedService, MixedState, MixedTripKind,
    PartKind,
};
use hertzforge_sine::law::{
    LawDescriptor, LawFault, LawGuards, LawValues, SineControlLaw, SineLawFactory, SineLawSetup,
    TickInputs, TickOutputs,
};
use std::sync::Arc;

/// Frames of transport delay: with the station's own pipeline, about 0.12 s
/// of lag at 4096 Hz (the bench's 1577 frames at 12 800 Hz).
const WIRE_DELAY: usize = 440;

/// The bench's grid scaled to 4096 Hz: frames ten times the lag (1 Hz
/// lines), four frames a loop.
fn wire_grid(mut c: MixedConfig) -> MixedConfig {
    c.df_hz = 1.0;
    c.lines = 1024;
    c.nfft = Some(4096);
    c.blocks_per_window = 4;
    c.exclusion_hz = 2.0;
    c
}

fn wire_engaged(cfg: MixedConfig, laws: MixedLawFactories) -> MixedService {
    let cfg = wire_grid(cfg);
    let sim = SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(Vec::new())
        .feedthrough(1.0)
        .noise_level(1e-5)
        .transport_delay_samples(WIRE_DELAY)
        .shared_clock(true)
        .seed(0x4345_494C)
        .build();
    let mut s = MixedService::new(
        cfg,
        table(),
        Box::new(sim),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        1,
        None,
    )
    .unwrap()
    .with_laws(laws)
    .with_block_frames(BLOCK);
    s.arm().unwrap();
    s.run_sysid(0).unwrap();
    assert!(s.coherence_gate().unwrap());
    s.proceed_after_review().unwrap();
    s
}

/// The bench row's case: a low floor, a 0.25 g tone at 160 Hz, the ladder
/// -12, -6, 0 dB, the ceiling at 0.15 V and its abort at `abort_vpk`.
fn ceiling_cfg(abort_vpk: f64) -> MixedConfig {
    let mut c = config(vec![floor(1e-7), tone("t1", fixed(160.0), 0.25)]);
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    c.identification.sysid_level_db = -20.0;
    c.schedule.table_db = vec![-12.0, -6.0, 0.0];
    c.schedule.start_db = -12.0;
    c.schedule.dwell_s = vec![8.0];
    c.full_level_duration_s = Some(10.0);
    c.limits.drive_vpk_ceiling = 0.15;
    c.limits.drive_vpk_abort = Some(abort_vpk);
    c
}

/// Every quarter second until the run ends (at most `secs`): level, the
/// tone's row and the summed drive's peak.
struct Tick {
    level_db: f64,
    error_db: Option<f64>,
    capped: u64,
    limiting: bool,
    drive_vpk: f64,
    clamped: bool,
}

fn watch(s: &mut MixedService, secs: f64) -> Vec<Tick> {
    let mut out = Vec::new();
    for _ in 0..(secs / 0.25) as usize {
        let alive = run_for(s, 0.25);
        let st = s.live_status();
        let t = st.parts.iter().find(|r| r.kind == PartKind::Tone).unwrap();
        out.push(Tick {
            level_db: st.level_db,
            error_db: t.error_db,
            capped: t.capped,
            limiting: t.limiting,
            drive_vpk: st.totals.drive_vpk,
            clamped: st.totals.clamped,
        });
        if !alive {
            break;
        }
    }
    out
}

#[test]
fn a_ceiling_tone_is_untouched_at_minus_12_db_and_held_at_the_ceiling_as_the_ladder_climbs() {
    let mut s = wire_engaged(ceiling_cfg(0.15), MixedLawFactories::builtin());
    let tr = watch(&mut s, 80.0);
    let reason = s.end_reason();
    // At -12 dB nothing binds: the tone reaches its target with no capped
    // call and is not limiting.
    let low: Vec<&Tick> = tr
        .iter()
        .filter(|t| (t.level_db + 12.0).abs() < 0.01 && t.error_db.is_some())
        .collect();
    assert!(
        low.len() >= 8,
        "{} readings at -12 dB; end {reason:?}",
        low.len()
    );
    let last = low.last().unwrap();
    assert!(
        last.error_db.unwrap().abs() < 0.5,
        "tone at -12 dB reads {:?}",
        last.error_db
    );
    assert!(low.iter().all(|t| t.capped == 0 && !t.limiting));
    // The summed drive never passes the ceiling.
    let peak = tr.iter().map(|t| t.drive_vpk).fold(0.0, f64::max);
    assert!(peak <= 0.15 + 1e-6, "peak {peak}");
    // Climbing, the ceiling holds the tone or the drive's abort ends the run;
    // never a divergence.
    let trip = s.trip().cloned();
    let held = tr.iter().any(|t| t.limiting);
    for lv in [-12.0, -6.0, 0.0] {
        let at: Vec<&Tick> = tr
            .iter()
            .filter(|t| (t.level_db - lv).abs() < 0.01)
            .collect();
        if let Some(l) = at.last() {
            eprintln!(
                "{lv} dB: {} readings, last error {:?} dB, limiting {}, capped {}, peak {:.4} V",
                at.len(),
                l.error_db,
                at.iter().any(|t| t.limiting),
                l.capped,
                at.iter().map(|t| t.drive_vpk).fold(0.0, f64::max)
            );
        }
    }
    eprintln!(
        "peak {peak:.4} V; limiting seen {held}; clamp seen {}; end {reason:?}",
        tr.iter().any(|t| t.clamped)
    );
    assert!(
        !matches!(
            trip.as_ref().map(|t| &t.kind),
            Some(MixedTripKind::Divergence)
        ),
        "{reason:?}"
    );
    assert!(
        held || matches!(
            trip.as_ref().map(|t| &t.kind),
            Some(MixedTripKind::DriveVpk { .. })
        ),
        "{reason:?}"
    );
    // Held at the ceiling, the tone plays at what the ceiling leaves it.
    if held {
        let tone_vpk = s
            .live_status()
            .parts
            .iter()
            .find(|r| r.kind == PartKind::Tone)
            .unwrap()
            .drive;
        assert!(tone_vpk <= 0.15 + 1e-9, "tone {tone_vpk}");
    }
}

#[test]
fn a_tone_the_ceiling_holds_under_its_abort_ends_on_its_own_abort_naming_the_ceiling() {
    let mut s = wire_engaged(ceiling_cfg(0.30), MixedLawFactories::builtin());
    let tr = watch(&mut s, 80.0);
    assert!(tr.iter().any(|t| t.limiting), "{:?}", s.end_reason());
    assert_eq!(s.state(), MixedState::Aborted, "{:?}", s.end_reason());
    let t = s.trip().unwrap();
    assert!(
        matches!(
            t.kind,
            MixedTripKind::ToneAbort {
                held_at_vpk: Some(v),
                error_db
            } if (v - 0.15).abs() < 1e-12 && error_db < 0.0
        ),
        "{t:?}"
    );
    assert_eq!(t.part.as_deref(), Some("t1"));
    let words = s.end_reason().unwrap();
    assert!(
        words.starts_with("Tone t1 abort") && words.contains("0.150 V drive ceiling"),
        "{words}"
    );
    // The tone was held at -6 dB or above, never at -12 dB.
    assert!(tr
        .iter()
        .filter(|t| t.limiting)
        .all(|t| t.level_db > -12.0 + 1e-6));
}

/// A law that always asks for twice the drive in force.
struct Greedy;
impl SineControlLaw for Greedy {
    fn name(&self) -> &str {
        "greedy"
    }
    fn engage(&mut self, i: &TickInputs<'_>, _: &mut LawValues) -> Result<TickOutputs, LawFault> {
        Ok(TickOutputs {
            drive: 10f64.powf(i.prediction_db.unwrap_or(-20.0) / 20.0),
            faulted: false,
        })
    }
    fn tick(&mut self, i: &TickInputs<'_>, _: &mut LawValues) -> Result<TickOutputs, LawFault> {
        Ok(TickOutputs {
            drive: 2.0 * i.drive,
            faulted: false,
        })
    }
    fn adjust(&mut self, _: &str, _: f64) -> Result<(), LawFault> {
        Ok(())
    }
}
struct GreedyFactory;
impl SineLawFactory for GreedyFactory {
    fn build(&self, _: &SineLawSetup<'_>) -> Result<Box<dyn SineControlLaw + Send>, LawFault> {
        Ok(Box::new(Greedy))
    }
    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "greedy".into(),
            name: "greedy".into(),
            version: "0".into(),
            sha256: None,
            params: Vec::new(),
            diagnostics: Vec::new(),
            live: Vec::new(),
        }
    }
}

#[test]
fn a_law_pushing_against_its_rate_cap_with_nothing_holding_it_still_trips_divergence() {
    let mut laws = MixedLawFactories::builtin();
    laws.tones = Arc::new(GreedyFactory);
    let mut c = config(vec![floor(1e-7), tone("t1", fixed(160.0), 0.25)]);
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    c.identification.sysid_level_db = -20.0;
    // The rise cap at 10 dB/s: a second of asking climbs 10 dB, far under
    // the 20 V ceiling.
    c.law_guards = Some(MixedLawGuards {
        floor: Default::default(),
        tones: LawGuards {
            max_rise_db_per_s: 10.0,
            ..LawGuards::default()
        },
    });
    let mut s = wire_engaged(c, laws);
    let tr = watch(&mut s, 30.0);
    assert!(tr.iter().all(|t| !t.limiting));
    let t = s.trip().expect("tripped");
    assert!(matches!(t.kind, MixedTripKind::Divergence), "{t:?}");
    assert_eq!(t.part.as_deref(), Some("t1"));
}

#[test]
fn a_band_the_random_ceiling_holds_does_not_wind_its_gain_up() {
    // The floor wants about 1.1 V rms at the plant's gain; the random part's
    // rms ceiling holds it at 0.5 V, so the band reads low the whole run.
    let mut c = config(vec![floor(0.01), band("b1", fixed(600.0), 0.05, 40.0)]);
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    c.limits.drive_ceiling_vrms = Some(0.5);
    c.limits.max_lines_out = usize::MAX;
    if let hertzforge_mixed::PartBody::Floor(f) = &mut c.parts[0].body {
        f.abort_db = (60.0, 60.0);
    }
    if let hertzforge_mixed::PartBody::Narrowband(b) = &mut c.parts[1].body {
        b.abort_db = (60.0, 60.0);
    }
    let mut s = engaged(c, DELAY);
    let mut worst_gain = f64::NEG_INFINITY;
    let mut band_limiting = false;
    let mut floor_limiting = false;
    for _ in 0..80 {
        assert!(run_for(&mut s, 0.25), "{:?}", s.end_reason());
        let st = s.live_status();
        let b = st
            .parts
            .iter()
            .find(|r| r.kind == PartKind::Narrowband)
            .unwrap();
        if b.on {
            worst_gain = worst_gain.max(b.gain_db.unwrap_or(0.0));
            band_limiting |= b.limiting;
        }
        floor_limiting |= st.parts[0].limiting;
    }
    eprintln!("band gain at most {worst_gain:+.2} dB under the random part's rms ceiling");
    assert!(band_limiting && floor_limiting);
    assert!(worst_gain <= 1.0 + 1e-9, "band gain {worst_gain:+.2} dB");
}
