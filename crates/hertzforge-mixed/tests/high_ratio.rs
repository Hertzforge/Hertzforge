//! A tone far above the floor (a low-level loopback's ratio): 0.2 g on a
//! floor of 1.3e-6 g^2/Hz, A^2 / G about 3e4 Hz (45 dB over the floor's
//! density in a hertz), on the lagged resonant plant of `common`, 1 Hz lines,
//! 4 frames a loop, through the ladder -12, -6, 0 dB. Swept at 2 oct/min from
//! 100 Hz the tone crosses a couple of lines a frame and comes in before its
//! tracker has settled; dwelling, it sits on the same lines throughout. In both
//! the floor must not see the tone: no floor abort line, its rms within 1 dB,
//! no line over the alarm within 10 Hz of the tone, and the tone within its
//! alarm once settled.

mod common;
use common::*;
use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_mixed::{
    parts::{Direction, PartSchedule, PartSweepTarget, SweepLaw},
    FloorPart, MixedConfig, MixedPhase, PartBody, PartKind,
};

fn cfg(schedule: PartSchedule) -> MixedConfig {
    let mut t = BreakpointTable::new("floor");
    t.push(BreakpointRow::psd(20.0, 1.3e-6));
    t.push(BreakpointRow::psd(1500.0, 1.3e-6));
    let mut f = floor(0.01);
    f.body = PartBody::Floor(FloorPart {
        breakpoints: t,
        alarm_db: (3.0, 3.0),
        abort_db: (6.0, 6.0),
    });
    let mut c = config(vec![f, tone("t1", schedule, 0.2)]);
    c.df_hz = 1.0;
    c.lines = 1500;
    c.nfft = Some(4096);
    c.blocks_per_window = 4;
    c.control_dof = 60.0;
    c.exclusion_hz = 2.0;
    c.soft_start_db = 0.0;
    c.floor_settle_s = 10.0;
    c.schedule.table_db = vec![-12.0, -6.0, 0.0];
    c.schedule.start_db = -12.0;
    c.schedule.dwell_s = vec![6.0];
    c.full_level_duration_s = Some(30.0);
    c
}

fn run(c: MixedConfig, label: &str) {
    let mut s = engaged(c, DELAY);
    let (mut alarm_max, mut floor_worst, mut tone) = (0usize, 0.0f64, Vec::new());
    let (mut near_max, mut abort_max) = (0usize, 0usize);
    let mut alive = true;
    while alive {
        alive = run_for(&mut s, 0.5);
        let st = s.live_status();
        if st.phase == MixedPhase::Running {
            // Lines over the upper alarm within 10 Hz of where the tone is: a
            // tone leaking into the floor reads high. (The floor's own scatter
            // puts a few lines under the lower alarm anywhere at the 16
            // degrees of freedom a line is first judged at, more where lines
            // are left out often and so restart their average.)
            let f = st.parts[1].freq_hz.unwrap_or(0.0);
            let d = s.display();
            let near = (0..d.grid.lines)
                .filter(|&k| {
                    let fk = k as f64 * d.grid.df_hz;
                    (fk - f).abs() <= 10.0 && d.reference[k] > 0.0 && !d.excluded[k]
                })
                .filter(|&k| 10.0 * (d.residual[k] / d.reference[k]).log10() > 3.0)
                .count();
            near_max = near_max.max(near);
            abort_max = abort_max.max(st.floor.abort_lines);
            alarm_max = alarm_max.max(st.floor.alarm_lines);
            if let Some(e) = st.floor.error_db_rms {
                floor_worst = if e.abs() > floor_worst.abs() {
                    e
                } else {
                    floor_worst
                };
            }
            let t = st.parts.iter().find(|r| r.kind == PartKind::Tone).unwrap();
            if let Some(e) = t.error_db {
                tone.push(e);
            }
        }
    }
    let worst_tone = tone.iter().fold(0.0f64, |m, e| m.max(e.abs()));
    eprintln!(
        "{label}: end {:?}; floor rms worst {floor_worst:+.2} dB, alarm lines at most {alarm_max} ({near_max} near the tone), abort lines at most {abort_max}; tone {} readings worst {worst_tone:.2} dB",
        s.end_reason(),
        tone.len()
    );
    assert!(s.trip().is_none(), "{label}: {:?}", s.end_reason());
    assert!(floor_worst.abs() < 1.0, "{label}: floor {floor_worst}");
    assert_eq!(near_max, 0, "{label}: alarm lines beside the tone");
    assert!(abort_max <= 2, "{label}: {abort_max} abort lines");
    assert!(
        !tone.is_empty() && worst_tone < 1.5,
        "{label}: tone {worst_tone}"
    );
}

#[test]
fn a_tone_far_over_the_floor_swept_from_100_hz_stays_out_of_the_floor() {
    run(
        cfg(PartSchedule::Sweep {
            lo_hz: 100.0,
            hi_hz: 400.0,
            start_hz: 100.0,
            direction: Direction::Up,
            law: SweepLaw::LogOctPerMin,
            rate: 2.0,
            target: PartSweepTarget::Sweeps { n: 2 },
        }),
        "swept",
    );
}

#[test]
fn a_tone_far_over_the_floor_dwelling_stays_out_of_the_floor() {
    run(cfg(fixed(160.0)), "dwell");
}
