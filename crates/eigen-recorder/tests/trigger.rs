//! Gate tests: exact-frame level triggering, hysteresis no-retrigger on
//! seeded noise, gap-free pre-trigger/live contiguity, rearm
//! holdoff, Falling slope, and pre-trigger clamping.

use eigen_recorder::{LevelTrigger, PreTriggerRing, Slope, TriggerEngine, TriggerMode};

/// Deterministic xorshift64 PRNG for the noise fixtures (no rand dep).
struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in [0, 1).
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn level_mode(channel: u32, threshold: f64, slope: Slope, hysteresis: f64) -> TriggerMode {
    TriggerMode::Level(LevelTrigger {
        channel_index: channel,
        threshold_eu: threshold,
        slope,
        hysteresis_eu: hysteresis,
        holdoff_s: 0.0,
    })
}

// ---------------------------------------------------------------- ring basics

#[test]
fn ring_snapshot_is_oldest_to_newest_across_wrap() {
    let mut ring = PreTriggerRing::new(2, 4);
    assert_eq!(ring.len_frames(), 0);
    assert!(ring.snapshot().is_empty());
    // Push 6 frames of 2 channels: values (i, -i).
    for i in 0..6 {
        ring.push_frames(&[i as f32, -(i as f32)]);
    }
    assert_eq!(ring.len_frames(), 4);
    // Newest 4 frames survive: 2, 3, 4, 5.
    assert_eq!(
        ring.snapshot(),
        vec![2.0, -2.0, 3.0, -3.0, 4.0, -4.0, 5.0, -5.0]
    );
}

#[test]
fn ring_push_longer_than_capacity_keeps_the_newest_frames() {
    let mut ring = PreTriggerRing::new(1, 3);
    let big: Vec<f32> = (0..10).map(|i| i as f32).collect();
    ring.push_frames(&big);
    assert_eq!(ring.len_frames(), 3);
    assert_eq!(ring.snapshot(), vec![7.0, 8.0, 9.0]);
}

// ------------------------------------------------------------- exact framing

#[test]
fn level_trigger_fires_at_the_exact_step_frame() {
    // 1 channel, fs 1000 Hz, small pre-trigger. Zeros arm the detector
    // (below threshold - hysteresis), a step at a known global frame fires.
    let mode = level_mode(0, 1.0, Slope::Rising, 0.2);
    let mut eng = TriggerEngine::new(mode, 1, 1000.0, 0.05, 0);
    const STEP_AT: usize = 237;
    let signal: Vec<f32> = (0..400)
        .map(|i| if i < STEP_AT { 0.0 } else { 5.0 })
        .collect();
    // Feed in odd-sized blocks so the step crosses a block boundary path.
    let mut fired = None;
    for block in signal.chunks(17) {
        if let Some(ev) = eng.feed(block, block.len()) {
            assert!(fired.is_none(), "must fire exactly once");
            fired = Some(ev);
        }
    }
    let ev = fired.expect("the step must fire the trigger");
    assert_eq!(ev.trigger_frame_global, STEP_AT as u64);
    // Pre-window is full (50 frames at 1000 Hz * 0.05 s) and all zeros.
    assert_eq!(ev.pretrigger_frames, 50);
    assert!(ev.pretrigger.iter().all(|&v| v == 0.0));
}

#[test]
fn trigger_channel_selection_uses_the_frame_index() {
    // 2 channels; the burst is on channel 1 only. Trigger on channel 1.
    let mode = level_mode(1, 1.0, Slope::Rising, 0.1);
    let mut eng = TriggerEngine::new(mode, 2, 1000.0, 0.01, 0);
    let mut frames = Vec::new();
    for i in 0..100 {
        frames.push(9.0); // channel 0 always high - must not fire
        frames.push(if i < 60 { 0.0 } else { 2.0 }); // channel 1 steps at 60
    }
    let ev = eng.feed(&frames, 100).expect("channel-1 step fires");
    assert_eq!(ev.trigger_frame_global, 60);
}

// ------------------------------------------------------ hysteresis / chatter

#[test]
fn hysteresis_prevents_retrigger_on_noise_around_the_threshold() {
    // Threshold 1.0, hysteresis 0.2. After the first fire the signal chatters
    // in [0.85, 1.15] - crossing the threshold repeatedly but never dropping
    // below the arming bound 0.8 - so the armed detector never re-arms and no
    // second event may fire, even with rearm budget available.
    let mode = level_mode(0, 1.0, Slope::Rising, 0.2);
    // Tiny pre-trigger so the ring-depth holdoff floor expires quickly.
    let mut eng = TriggerEngine::new(mode, 1, 1000.0, 0.002, 5);
    let mut rng = XorShift64::new(0x9E3779B97F4A7C15);

    let mut signal: Vec<f32> = vec![0.0; 50]; // arms
    signal.push(5.0); // fires at frame 50
    for _ in 0..5000 {
        // Noise in [0.85, 1.15]: chatter around the threshold inside the band.
        signal.push((0.85 + 0.30 * rng.next_f64()) as f32);
    }

    let mut events = Vec::new();
    for block in signal.chunks(64) {
        if let Some(ev) = eng.feed(block, block.len()) {
            events.push(ev);
        }
    }
    assert_eq!(
        events.len(),
        1,
        "noise inside the hysteresis band must not retrigger"
    );
    assert_eq!(events[0].trigger_frame_global, 50);
    assert!(
        eng.rearms_remaining() > 0,
        "budget was available; hysteresis did the work"
    );
}

// ------------------------------------------------- pre-trigger/live contiguity

#[test]
fn pretrigger_snapshot_joins_gap_free_with_the_live_continuation() {
    // 2 channels carrying a known ramp: frame i = (i, i + 0.5). Trigger on
    // channel 0 crossing 100. The joined pre-window + live-from-trigger
    // sequence must be gapless across the boundary.
    let mode = level_mode(0, 100.0, Slope::Rising, 1.0);
    let mut eng = TriggerEngine::new(mode, 2, 1000.0, 0.05, 0); // ring = 50 frames
    let n_ch = 2;

    let mut joined: Vec<f32> = Vec::new();
    let mut live = false;
    for i in 0..300u64 {
        let frame = [i as f32, i as f32 + 0.5];
        if let Some(ev) = eng.feed(&frame, 1) {
            assert_eq!(ev.trigger_frame_global, 100);
            assert_eq!(ev.pretrigger_frames, 50);
            joined.extend_from_slice(&ev.pretrigger);
            live = true;
        }
        if live {
            // The trigger frame itself is the first live frame.
            joined.extend_from_slice(&frame);
        }
    }
    assert!(live, "ramp must fire");
    // Joined = frames 50..300, interleaved, gapless.
    assert_eq!(joined.len(), (300 - 50) * n_ch);
    for (k, frame) in joined.chunks_exact(n_ch).enumerate() {
        let i = (50 + k) as f32;
        assert_eq!(frame[0], i, "channel 0 gap at joined frame {k}");
        assert_eq!(frame[1], i + 0.5, "channel 1 gap at joined frame {k}");
    }
}

// ------------------------------------------------------------ holdoff / rearm

#[test]
fn rearm_holdoff_is_at_least_the_ring_depth_and_the_configured_holdoff() {
    // fs 1000 Hz, ring 100 frames (0.1 s), holdoff 0.3 s -> 300 frames.
    let mode = TriggerMode::Level(LevelTrigger {
        channel_index: 0,
        threshold_eu: 1.0,
        slope: Slope::Rising,
        hysteresis_eu: 0.1,
        holdoff_s: 0.3,
    });
    let mut eng = TriggerEngine::new(mode, 1, 1000.0, 0.1, 3);
    assert_eq!(eng.ring_capacity_frames(), 100);

    // Pulse train: arm-fire pulses at frames 100, 250, 500.
    // Fire 1 at 100. The pulse at 250 falls INSIDE the 300-frame holdoff
    // (100 + 300 = 400) and must not fire. The pulse at 500 fires.
    let mut signal = vec![0.0f32; 1000];
    for &at in &[100usize, 250, 500] {
        signal[at] = 5.0; // one-frame pulse; surrounding zeros re-arm
    }
    let mut fires = Vec::new();
    for (i, block) in signal.chunks(10).enumerate() {
        if let Some(ev) = eng.feed(block, block.len()) {
            fires.push((i, ev.trigger_frame_global));
        }
    }
    let frames: Vec<u64> = fires.iter().map(|(_, f)| *f).collect();
    assert_eq!(
        frames,
        vec![100, 500],
        "the in-holdoff pulse must be swallowed"
    );
}

#[test]
fn ring_depth_floors_the_holdoff_when_no_holdoff_is_configured() {
    // Zero configured holdoff, ring 200 frames: a second pulse 150 frames
    // after the first fire is inside the ring-depth floor (its pre-window
    // would duplicate the prior run's tail) and must be swallowed.
    let mode = level_mode(0, 1.0, Slope::Rising, 0.1);
    let mut eng = TriggerEngine::new(mode, 1, 1000.0, 0.2, 3);
    assert_eq!(eng.ring_capacity_frames(), 200);

    let mut signal = vec![0.0f32; 1000];
    signal[300] = 5.0;
    signal[450] = 5.0; // 150 < 200 after fire 1 -> swallowed
    signal[600] = 5.0; // 300 >= 200 after fire 1 -> fires
    let mut frames = Vec::new();
    for block in signal.chunks(32) {
        if let Some(ev) = eng.feed(block, block.len()) {
            frames.push(ev.trigger_frame_global);
        }
    }
    assert_eq!(frames, vec![300, 600]);
}

#[test]
fn auto_rearm_counts_down_and_exhausts() {
    // auto_rearm = 1 -> two fires total, then exhausted.
    let mode = level_mode(0, 1.0, Slope::Rising, 0.1);
    let mut eng = TriggerEngine::new(mode, 1, 1000.0, 0.0, 1); // no ring, no holdoff
    let mut signal = vec![0.0f32; 500];
    signal[50] = 5.0;
    signal[150] = 5.0;
    signal[250] = 5.0; // exhausted - must not fire
    let mut frames = Vec::new();
    for block in signal.chunks(16) {
        if let Some(ev) = eng.feed(block, block.len()) {
            frames.push(ev.trigger_frame_global);
        }
    }
    assert_eq!(frames, vec![50, 150]);
    assert_eq!(eng.rearms_remaining(), 0);

    // Manual rearm restores the budget.
    eng.rearm(1);
    let mut tail = vec![0.0f32; 100];
    tail[60] = 5.0;
    let ev = eng.feed(&tail, 100).expect("rearmed engine fires again");
    assert_eq!(ev.trigger_frame_global, 500 + 60);
}

// ------------------------------------------------------------- falling slope

#[test]
fn falling_slope_fires_on_the_exact_downward_crossing() {
    // Threshold -1.0, hysteresis 0.2: arms while the signal sits above -0.8,
    // fires on the first sample <= -1.0.
    let mode = level_mode(0, -1.0, Slope::Falling, 0.2);
    let mut eng = TriggerEngine::new(mode, 1, 1000.0, 0.01, 0);
    const DROP_AT: usize = 123;
    let signal: Vec<f32> = (0..300)
        .map(|i| if i < DROP_AT { 0.0 } else { -3.0 })
        .collect();
    let mut fired = None;
    for block in signal.chunks(23) {
        if let Some(ev) = eng.feed(block, block.len()) {
            fired = Some(ev);
        }
    }
    assert_eq!(
        fired.expect("must fire").trigger_frame_global,
        DROP_AT as u64
    );
}

// -------------------------------------------------------------- manual / free

#[test]
fn free_run_and_manual_modes_never_level_fire() {
    for mode in [TriggerMode::FreeRun, TriggerMode::Manual] {
        let mut eng = TriggerEngine::new(mode, 1, 1000.0, 0.01, 0);
        let loud = vec![100.0f32; 500];
        assert!(eng.feed(&loud, 500).is_none());
        assert_eq!(eng.global_frame(), 500);
    }
}

#[test]
fn manual_fire_snapshots_the_ring_at_the_current_frame() {
    let mut eng = TriggerEngine::new(TriggerMode::Manual, 1, 1000.0, 0.005, 0); // ring 5
    let ramp: Vec<f32> = (0..20).map(|i| i as f32).collect();
    assert!(eng.feed(&ramp, 20).is_none());
    let ev = eng.manual_fire();
    assert_eq!(ev.trigger_frame_global, 20);
    assert_eq!(ev.pretrigger_frames, 5);
    assert_eq!(ev.pretrigger, vec![15.0, 16.0, 17.0, 18.0, 19.0]);
}

// ------------------------------------------------------------------ clamping

#[test]
fn pretrigger_seconds_clamp_to_the_bounded_max() {
    // 100 s requested at 1000 Hz clamps to 10 s = 10,000 frames.
    let eng = TriggerEngine::new(TriggerMode::Manual, 1, 1000.0, 100.0, 0);
    assert_eq!(eng.ring_capacity_frames(), 10_000);
    // Negative and NaN clamp to zero.
    let eng = TriggerEngine::new(TriggerMode::Manual, 1, 1000.0, -1.0, 0);
    assert_eq!(eng.ring_capacity_frames(), 0);
    let eng = TriggerEngine::new(TriggerMode::Manual, 1, 1000.0, f64::NAN, 0);
    assert_eq!(eng.ring_capacity_frames(), 0);
    // The default 1.0 s at 1000 Hz is 1000 frames.
    let eng = TriggerEngine::new(
        TriggerMode::Manual,
        1,
        1000.0,
        eigen_recorder::trigger::DEFAULT_PRETRIGGER_S,
        0,
    );
    assert_eq!(eng.ring_capacity_frames(), 1000);
}

#[test]
fn default_hysteresis_is_one_percent_of_range() {
    assert_eq!(LevelTrigger::default_hysteresis_eu(100.0), 1.0);
    let lt = LevelTrigger::new(0, 5.0, Slope::Rising, 50.0);
    assert_eq!(lt.hysteresis_eu, 0.5);
}
