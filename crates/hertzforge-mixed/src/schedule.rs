//! The clock that moves every part: each part's frequency for a step of test
//! time, over Sine's bouncing sweep ([`BidirSweep`]) and dwell ([`DwellRun`]).
//!
//! Rules (each is a unit test below):
//!
//! - **One test clock.** Test time advances by the step given, holds excluded.
//!   A part's window opens and closes on it.
//! - **A part's own clock runs only while it plays.** A sweep or a timed dwell
//!   moves only for the share of a step its window is open and it is switched
//!   on, so a part switched on at 10 s starts its sweep at 10 s.
//! - **`follow` is resolved in dependency order**, every part after the part it
//!   follows: its frequency is that part's times the ratio, every step.
//! - **A hold stops every clock together:** the test clock, every sweep, every
//!   dwell timer and every window. Resume restarts them together.
//! - **A part ends** when its sweep meets its target, its timed dwell runs out,
//!   or its window closes; a follower ends with the part it follows. An ended
//!   part stays at the frequency it ended at (its level is ramped out by the
//!   core, not here).

use crate::parts::{ConfigError, Direction, MixedConfig, Part, PartSchedule};
use hertzforge_sine::scheduler::{BidirSweep, DwellFor, DwellPlan, DwellRun};

/// One part's motion engine.
#[derive(Debug, Clone)]
enum Engine {
    /// The floor (no frequency).
    None,
    /// A dwell; a timed one runs a [`DwellRun`].
    Fixed { hz: f64, run: Option<DwellRun> },
    /// A bouncing sweep.
    Sweep(BidirSweep),
    /// Another part's frequency times `ratio`.
    Follow { of: usize, ratio: f64 },
}

/// One part as the clock has it after the last step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartNow {
    /// The part's frequency (a tone) or center (a band), Hz; `None` for the
    /// floor.
    pub freq_hz: Option<f64>,
    /// Whether the part plays now: switched on, inside its window, not ended.
    pub on: bool,
    /// Whether the part has ended (its target met, its timed dwell run out, its
    /// window closed, or the part it follows ended).
    pub done: bool,
    /// Completed one-direction passes of a sweep; 0 otherwise.
    pub sweeps_done: u32,
    /// A sweep's direction now; `None` otherwise.
    pub direction: Option<Direction>,
    /// Seconds this part's own clock has run (its sweep or dwell time).
    pub part_s: f64,
}

/// The clock of every part of a run.
#[derive(Debug, Clone)]
pub struct PartClock {
    engines: Vec<Engine>,
    windows: Vec<crate::parts::PartWindow>,
    enabled: Vec<bool>,
    order: Vec<usize>,
    now: Vec<PartNow>,
    t_s: f64,
    held: bool,
}

impl PartClock {
    /// Build the clock for `cfg`'s parts. The configuration must already have
    /// passed [`MixedConfig::validate`]; a schedule the clock cannot build is
    /// refused as validation refuses it.
    pub fn new(cfg: &MixedConfig) -> Result<Self, ConfigError> {
        let order = cfg.follow_order()?;
        let mut engines = Vec::with_capacity(cfg.parts.len());
        for p in &cfg.parts {
            engines.push(engine_for(cfg, p)?);
        }
        let mut clock = PartClock {
            now: vec![
                PartNow {
                    freq_hz: None,
                    on: false,
                    done: false,
                    sweeps_done: 0,
                    direction: None,
                    part_s: 0.0,
                };
                cfg.parts.len()
            ],
            windows: cfg.parts.iter().map(|p| p.window).collect(),
            enabled: cfg.parts.iter().map(|p| p.enabled).collect(),
            engines,
            order,
            t_s: 0.0,
            held: false,
        };
        clock.refresh();
        Ok(clock)
    }

    /// Advance test time by `dt` seconds (nothing moves while held, or for a
    /// step that is not positive) and return every part as it is after the
    /// step, indexed as the parts list.
    pub fn tick(&mut self, dt: f64) -> &[PartNow] {
        if self.held || dt.is_nan() || dt <= 0.0 {
            return &self.now;
        }
        let (t0, t1) = (self.t_s, self.t_s + dt);
        for i in 0..self.engines.len() {
            if !self.enabled[i] || self.now[i].done {
                continue;
            }
            let w = self.windows[i];
            // The share of [t0, t1) inside the window.
            let start = t0.max(w.on_s);
            let end = w.off_s.map_or(t1, |off| t1.min(off));
            let run = (end - start).max(0.0);
            // A sweep or timed dwell keeps its own time, which stops the
            // moment its target is met inside the step.
            self.now[i].part_s = match &mut self.engines[i] {
                Engine::Fixed { run: Some(d), .. } => {
                    d.tick(run);
                    d.elapsed_s()
                }
                Engine::Sweep(s) => {
                    s.tick(run);
                    s.elapsed_test_s()
                }
                _ => self.now[i].part_s + run,
            };
        }
        self.t_s = t1;
        self.refresh();
        &self.now
    }

    /// Every part as the clock has it, indexed as the parts list.
    pub fn now(&self) -> &[PartNow] {
        &self.now
    }

    /// Hold every clock: the test clock, every sweep, every dwell timer and
    /// every window.
    pub fn hold(&mut self) {
        self.held = true;
        for e in &mut self.engines {
            match e {
                Engine::Sweep(s) => s.hold(),
                Engine::Fixed { run: Some(d), .. } => d.hold(),
                _ => {}
            }
        }
    }

    /// Resume every clock together.
    pub fn resume(&mut self) {
        self.held = false;
        for e in &mut self.engines {
            match e {
                Engine::Sweep(s) => s.resume(),
                Engine::Fixed { run: Some(d), .. } => d.resume(),
                _ => {}
            }
        }
    }

    /// Whether the clocks are held.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Test time, seconds, holds excluded.
    pub fn t_s(&self) -> f64 {
        self.t_s
    }

    /// Switch part `index` on or off from now (the operator's live switch). A
    /// part switched off keeps its clock where it is; switched on again it
    /// goes on from there.
    pub fn set_enabled(&mut self, index: usize, on: bool) {
        if let Some(e) = self.enabled.get_mut(index) {
            *e = on;
            self.refresh();
        }
    }

    /// Whether every tone and band has ended (the floor never ends).
    pub fn all_parts_done(&self) -> bool {
        self.engines
            .iter()
            .zip(&self.now)
            .filter(|(e, _)| !matches!(e, Engine::None))
            .all(|(_, n)| n.done)
    }

    /// Recompute every part's frequency and state, in dependency order.
    fn refresh(&mut self) {
        for k in 0..self.order.len() {
            let i = self.order[k];
            let window_done = self.windows[i].off_s.is_some_and(|off| self.t_s >= off);
            let (freq, own_done, sweeps, dir) = match &self.engines[i] {
                Engine::None => (None, false, 0, None),
                Engine::Fixed { hz, run } => {
                    (Some(*hz), run.as_ref().is_some_and(DwellRun::done), 0, None)
                }
                Engine::Sweep(s) => (
                    Some(s.freq_hz()),
                    s.done(),
                    s.sweeps_done(),
                    Some(Direction::from(s.direction())),
                ),
                Engine::Follow { of, ratio } => {
                    let parent = self.now[*of];
                    (parent.freq_hz.map(|f| f * ratio), parent.done, 0, None)
                }
            };
            let done = self.now[i].done || own_done || window_done;
            let n = &mut self.now[i];
            n.freq_hz = freq;
            n.done = done;
            n.sweeps_done = sweeps;
            n.direction = dir;
            n.on = self.enabled[i] && !done && self.windows[i].contains(self.t_s);
        }
    }
}

fn engine_for(cfg: &MixedConfig, p: &Part) -> Result<Engine, ConfigError> {
    let bad = |e: hertzforge_sine::ControlError| ConfigError::BadPart {
        part: p.id.clone(),
        field: "schedule",
        why: e.to_string(),
    };
    Ok(match p.schedule() {
        None => Engine::None,
        Some(PartSchedule::Fixed { hz, for_s }) => Engine::Fixed {
            hz: *hz,
            run: match for_s {
                Some(s) => Some(
                    DwellRun::new(DwellPlan {
                        freq_hz: *hz,
                        for_: DwellFor::Seconds(*s),
                    })
                    .map_err(bad)?,
                ),
                None => None,
            },
        },
        Some(s @ PartSchedule::Sweep { .. }) => {
            Engine::Sweep(BidirSweep::new(s.sweep_config().expect("a sweep")).map_err(bad)?)
        }
        Some(PartSchedule::Follow { of, ratio }) => Engine::Follow {
            of: cfg
                .index_of(of)
                .ok_or_else(|| ConfigError::FollowUnknownPart {
                    part: p.id.clone(),
                    of: of.clone(),
                })?,
            ratio: *ratio,
        },
        Some(PartSchedule::Segments { .. }) => {
            return Err(ConfigError::SegmentsNotBuilt { part: p.id.clone() })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::tests_support::*;
    use crate::parts::{PartSweepTarget, PartWindow, SweepLaw};

    fn sweep_cfg(law: SweepLaw, rate: f64, target: PartSweepTarget) -> MixedConfig {
        config(vec![
            floor("floor"),
            tone(
                "s",
                PartSchedule::Sweep {
                    lo_hz: 100.0,
                    hi_hz: 400.0,
                    start_hz: 100.0,
                    direction: Direction::Up,
                    law,
                    rate,
                    target,
                },
            ),
        ])
    }

    /// Step `clock` to `t` seconds in steps of `dt` and return part `i`.
    fn run_to(clock: &mut PartClock, t: f64, dt: f64, i: usize) -> PartNow {
        while clock.t_s() + 1e-12 < t {
            let step = dt.min(t - clock.t_s());
            clock.tick(step);
        }
        clock.now()[i]
    }

    #[test]
    fn a_linear_sweep_is_at_its_closed_form_frequency() {
        let c = sweep_cfg(
            SweepLaw::LinearHzPerS,
            5.0,
            PartSweepTarget::Sweeps { n: 2 },
        );
        let mut k = PartClock::new(&c).unwrap();
        // f(t) = 100 + 5 t.
        for t in [1.0, 10.0, 37.5] {
            let f = run_to(&mut k, t, 0.01, 1).freq_hz.unwrap();
            assert!((f - (100.0 + 5.0 * t)).abs() < 1e-9, "t {t}: {f}");
        }
        assert_eq!(k.now()[0].freq_hz, None, "the floor has no frequency");
        assert!(k.now()[0].on);
    }

    #[test]
    fn a_log_sweep_is_at_its_closed_form_frequency() {
        // 1 oct/min from 100 Hz: f(t) = 100 * 2^(t/60), 2 octaves to 400 Hz.
        let c = sweep_cfg(
            SweepLaw::LogOctPerMin,
            1.0,
            PartSweepTarget::Sweeps { n: 2 },
        );
        let mut k = PartClock::new(&c).unwrap();
        for t in [6.0, 60.0, 90.0] {
            let f = run_to(&mut k, t, 0.01, 1).freq_hz.unwrap();
            let want = 100.0 * 2f64.powf(t / 60.0);
            assert!((f - want).abs() < 1e-9 * want, "t {t}: {f} against {want}");
        }
    }

    #[test]
    fn a_sweep_bounces_at_its_limit_and_counts_one_pass() {
        let c = sweep_cfg(
            SweepLaw::LinearHzPerS,
            10.0,
            PartSweepTarget::Sweeps { n: 3 },
        );
        let mut k = PartClock::new(&c).unwrap();
        // 30 s to 400 Hz, then 5 s back down to 350 Hz, in one step.
        let n = k.tick(35.0)[1];
        assert!((n.freq_hz.unwrap() - 350.0).abs() < 1e-9);
        assert_eq!(n.sweeps_done, 1);
        assert_eq!(n.direction, Some(Direction::Down));
        assert!(n.on && !n.done);
    }

    #[test]
    fn a_count_target_ends_the_part_at_the_limit() {
        let c = sweep_cfg(
            SweepLaw::LinearHzPerS,
            10.0,
            PartSweepTarget::Sweeps { n: 2 },
        );
        let mut k = PartClock::new(&c).unwrap();
        let n = run_to(&mut k, 59.0, 0.5, 1);
        assert!(!n.done);
        let n = run_to(&mut k, 61.0, 0.5, 1);
        assert!(n.done && !n.on);
        assert_eq!(n.sweeps_done, 2);
        assert_eq!(n.freq_hz, Some(100.0));
        assert!(k.all_parts_done());
    }

    #[test]
    fn a_duration_target_ends_the_part_on_time() {
        let c = sweep_cfg(
            SweepLaw::LinearHzPerS,
            10.0,
            PartSweepTarget::Duration { s: 12.0 },
        );
        let mut k = PartClock::new(&c).unwrap();
        let n = run_to(&mut k, 11.9, 0.1, 1);
        assert!(!n.done);
        let n = run_to(&mut k, 12.5, 0.1, 1);
        assert!(n.done);
        assert!((n.freq_hz.unwrap() - 220.0).abs() < 1e-9);
        assert!(
            (n.part_s - 12.0).abs() < 1e-6,
            "its clock stopped with it: {}",
            n.part_s
        );
        // A timed dwell ends too.
        let c = config(vec![
            floor("floor"),
            tone(
                "d",
                PartSchedule::Fixed {
                    hz: 100.0,
                    for_s: Some(3.0),
                },
            ),
        ]);
        let mut k = PartClock::new(&c).unwrap();
        assert!(!run_to(&mut k, 2.9, 0.1, 1).done);
        assert!(run_to(&mut k, 3.1, 0.1, 1).done);
    }

    #[test]
    fn a_follower_holds_its_ratio_through_a_bounce() {
        let mut c = sweep_cfg(
            SweepLaw::LinearHzPerS,
            10.0,
            PartSweepTarget::Sweeps { n: 4 },
        );
        c.parts.push(tone(
            "h",
            PartSchedule::Follow {
                of: "s".into(),
                ratio: 2.5,
            },
        ));
        let mut k = PartClock::new(&c).unwrap();
        let mut bounced = false;
        while k.t_s() < 70.0 {
            let now = k.tick(0.37).to_vec();
            let (f, h) = (now[1].freq_hz.unwrap(), now[2].freq_hz.unwrap());
            assert!((h - 2.5 * f).abs() < 1e-9 * h, "{h} against 2.5 x {f}");
            bounced |= now[1].sweeps_done >= 1;
        }
        assert!(bounced);
        // It ends with the part it follows.
        run_to(&mut k, 200.0, 1.0, 1);
        assert!(k.now()[1].done && k.now()[2].done);
    }

    #[test]
    fn a_hold_stops_every_clock_and_resume_restarts_them_together() {
        let mut c = sweep_cfg(
            SweepLaw::LinearHzPerS,
            10.0,
            PartSweepTarget::Sweeps { n: 2 },
        );
        c.parts.push(tone(
            "d",
            PartSchedule::Fixed {
                hz: 50.0,
                for_s: Some(5.0),
            },
        ));
        // A part whose window opens at 4 s.
        let mut late = tone(
            "late",
            PartSchedule::Fixed {
                hz: 600.0,
                for_s: None,
            },
        );
        late.window = PartWindow {
            on_s: 4.0,
            off_s: None,
        };
        c.parts.push(late);
        let mut k = PartClock::new(&c).unwrap();
        k.tick(2.0);
        let before = k.now().to_vec();
        k.hold();
        for _ in 0..100 {
            k.tick(0.1);
        }
        assert_eq!(k.t_s(), 2.0, "the test clock stopped");
        assert_eq!(k.now(), &before[..], "no part moved");
        assert!(!k.now()[3].on, "a window did not open while held");
        k.resume();
        k.tick(2.5);
        assert!((k.now()[1].freq_hz.unwrap() - 145.0).abs() < 1e-9);
        assert!(!k.now()[2].done, "the dwell has run 4.5 of its 5 s");
        assert!(k.now()[3].on);
        assert!((k.now()[3].part_s - 0.5).abs() < 1e-12);
    }

    #[test]
    fn a_part_plays_only_inside_its_window_and_its_clock_starts_there() {
        let mut c = sweep_cfg(
            SweepLaw::LinearHzPerS,
            10.0,
            PartSweepTarget::Sweeps { n: 2 },
        );
        c.parts[1].window = PartWindow {
            on_s: 10.0,
            off_s: Some(20.0),
        };
        let mut k = PartClock::new(&c).unwrap();
        let n = k.tick(5.0)[1];
        assert!(!n.on && !n.done);
        assert_eq!(n.freq_hz, Some(100.0), "its sweep has not started");
        // 15 s: 5 s into its window.
        let n = k.tick(10.0)[1];
        assert!(n.on);
        assert!((n.freq_hz.unwrap() - 150.0).abs() < 1e-9);
        // 25 s: the window closed at 20 s, after 10 s of sweep.
        let n = k.tick(10.0)[1];
        assert!(!n.on && n.done);
        assert!((n.freq_hz.unwrap() - 200.0).abs() < 1e-9);
    }

    #[test]
    fn a_part_switched_off_live_keeps_its_place() {
        let c = sweep_cfg(
            SweepLaw::LinearHzPerS,
            10.0,
            PartSweepTarget::Sweeps { n: 2 },
        );
        let mut k = PartClock::new(&c).unwrap();
        k.tick(3.0);
        k.set_enabled(1, false);
        assert!(!k.now()[1].on);
        k.tick(5.0);
        assert!((k.now()[1].freq_hz.unwrap() - 130.0).abs() < 1e-9);
        k.set_enabled(1, true);
        k.tick(1.0);
        assert!((k.now()[1].freq_hz.unwrap() - 140.0).abs() < 1e-9);
    }
}
