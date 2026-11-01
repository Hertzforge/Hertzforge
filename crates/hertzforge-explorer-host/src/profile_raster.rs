//! THE PEAK-LEVEL SWEEP PROFILE: the reference controller's Fn-sweep curve, kept as a
//! SORTED LIST OF VISITED POINTS and rasterized onto Hertzforge's display spectrum grid only at
//! render time.
//!
//! # What the operator asked for
//!
//! While the tone moves - swept OR walked by hand - the response level at each frequency
//! is REMEMBERED, and the remembered curve persists after the tone has gone somewhere
//! else. That is what an operator reads a resonance survey off, and it is the graph that
//! ends up in a trip report. Commercial controllers have done it for decades and the workflow
//! is built around it.
//!
//! # What the reference controller actually draws (the evidence, 2026-08-08)
//!
//! Its setup table gives the graph a frequency axis of its own: `Low`/`High` inside
//! 2-5000 Hz, **"2000 frequency points are provided across the full range"**, with a
//! `Spacing: Log / Linear` radio choosing how those points are distributed, and the graph
//! cursor stepping "one frequency-resolution point" per arrow press
//! (its options manual, sections 2.2 and 2.1-H). That axis is
//! the OSCILLATOR's point list, not an FFT bin grid - so its curve is a value per
//! frequency point along a log axis, which is a sorted point list by construction.
//!
//! The manual is SILENT on the accumulation mechanism itself: nothing in the manual
//! chapters says whether a revisit overwrites, whether anything is held or decayed, or
//! what is drawn between two points the tone did not stop at. Section 5's own
//! open-questions list does not even raise it. So the mechanism below is read off the
//! PLOTS rather than off prose - two screen grabs of the reference controller's
//! Fn sweep from a 2026-08-05 field job: a single continuous
//! polyline spanning the whole surveyed 200-3000 Hz span, no gaps anywhere, and at the
//! sharp 2.3 kHz resonances a NEAR-VERTICAL straight segment between two adjacent points.
//! A straight chord across a steep step is what joining a sorted point list looks like; it
//! is not what a bin raster with unvisited cells looks like.
//!
//! # The four rules, stated once
//!
//! 1. **A visit is a POINT, not a cell.** `(f, level)` goes into a list kept sorted by
//!    frequency. Nothing is quantized on the way in; the display grid enters only at
//!    render.
//! 2. **A revisit OVERWRITES.** A visit within [`ProfileRaster::tolerance_hz`] (half a
//!    display bin) of an existing point replaces it. Not a max-hold - that would weld a
//!    single transient overload into the operator's saved graph for the rest of the run
//!    and make a fixed drive read as a rising one - and not a decay, which would erase the
//!    low end of a slow sweep before the high end was measured.
//! 3. **PASSING THROUGH A REGION REWRITES IT.** The tone travels continuously, so two
//!    consecutively recorded points bound a span the tone just crossed. Every OLDER point
//!    strictly inside that span is REMOVED. This is the operator's "wipe out the old
//!    values at each frequency line as the tone passes it again": a second walk over a
//!    band leaves that band showing the second walk, never a stitch of both.
//! 4. **Between two points, a straight line; outside them, NOTHING.** The render joins
//!    adjacent points with LINEAR INTERPOLATION IN LOG FREQUENCY across a gap of ANY
//!    width - log, because the pane this lands on is a log-x sweep plot and a linear-in-Hz
//!    fill would bend visibly against its own axis. Below the lowest point and above the
//!    highest the raster renders NaN, this display's established "no datum"
//!    (`SpectrumPane`'s `padTo` pads with it and the cursor readout refuses to print it),
//!    so an unsurveyed band reads as absent rather than as a measured zero.
//!
//! # The two defects this model replaced (round 3, 2026-08-08)
//!
//! The first cut remembered per display BIN and interpolated between whichever bins held a
//! number. Both of the operator's complaints fall out of that one choice:
//!
//! * **Coarse clicks left the plot "full of holes."** A coarse walk lands on a handful of
//!   bins and passes OVER the rest, so every bin an earlier pass had written kept its old
//!   value and the curve was stitched THROUGH it. Points written while the drive was still
//!   at the mandatory 0.000 V arm posture, or mid-ramp, are floor-level readings; on a
//!   log-y axis the curve dives off the bottom of the scale at each one and reads as a
//!   hole. Rule 3 removes them as the tone goes past.
//! * **Stale points lingered through re-walks.** A re-walk whose steps land between the
//!   previous pass's bins overwrote none of them. Rule 3 again: the wipe is by TRAVERSAL,
//!   not by whether the new step happened to land on the old one.
//!
//! Fine clicks looked smooth throughout because a ~1% step is smaller than a display bin
//! over most of a survey band, so a fine walk lands on essentially every bin and overwrote
//! the stale values by accident. That is why the defect read as coarse-only.
//!
//! # Lifetime: what clears this, and what does not
//!
//! The profile is owned by the Explorer pump thread, so it lives exactly as long as one
//! RUN. Concretely:
//!
//! * **Stopping a sweep does NOT clear it** - the tone parks and the profile stands. This
//!   is the whole point of "remember until overwritten".
//! * **Continuing to sweep does not clear it** either; a second pass rewrites the span it
//!   crosses (rule 3) and leaves the rest.
//! * **A new Run clears it**, because `ExplorerHost::start` spawns a new pump and the
//!   pump owns it.
//! * **A profile change clears it**, because `ExplorerHost::set_profile` builds a new
//!   service (and refuses while a run is live), so the next Run is a new pump either way.
//!   It has to: `display_nfft` and the adopted rate define the render grid, and a curve
//!   carried across a grid change would be plotted against the wrong frequencies.

/// The peak-level sweep profile: the visited points, and the display grid they are
/// rasterized onto. See the module docs for the four rules and the lifetime.
///
/// The name is historical - this was a bin raster before the point list replaced it, and
/// it is still what rasterizes the curve onto the spectra grid for the wire.
#[derive(Debug, Clone)]
pub struct ProfileRaster {
    f0_hz: f64,
    df_hz: f64,
    n_bins: usize,
    /// The visited `(frequency, level)` points, ASCENDING by frequency and never closer
    /// together than [`Self::tolerance_hz`].
    ///
    /// That spacing invariant is also what BOUNDS this vector, which is the only thing
    /// standing between a long survey and unbounded growth: every insert first removes
    /// every point within a tolerance of itself, so the list can never hold more than
    /// about two points per display bin however long the operator sweeps.
    points: Vec<(f64, f32)>,
    /// The frequency of the previously recorded point - the far end of the traversal the
    /// next visit closes (rule 3). `None` before the first visit of a run.
    last_hz: Option<f64>,
}

impl ProfileRaster {
    /// A profile rendered onto `n_bins` bins starting at `f0_hz`, spaced `df_hz` apart.
    /// For the Explorer that is `f0 = 0`, `df = actual_fs / display_nfft`,
    /// `n_bins = display_nfft / 2 + 1` - the grid `ChannelSpectrum::transform` emits, so
    /// the profile lands on the same x-axis as the spectra it is read against.
    pub fn new(f0_hz: f64, df_hz: f64, n_bins: usize) -> Self {
        ProfileRaster {
            f0_hz,
            df_hz,
            n_bins,
            points: Vec::new(),
            last_hz: None,
        }
    }

    /// The grid this profile renders onto: `(f0_hz, df_hz, n_bins)`.
    pub fn grid(&self) -> (f64, f64, usize) {
        (self.f0_hz, self.df_hz, self.n_bins)
    }

    /// How close two visits have to be to count as the SAME frequency line: half a
    /// display bin, so a revisit that would render into the same bin overwrites rather
    /// than accumulating a second point the render could never separate.
    pub fn tolerance_hz(&self) -> f64 {
        self.df_hz.abs() * 0.5
    }

    /// Has anything been visited yet?
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Forget everything. (Not reached today - the pump owns one profile per run - but the
    /// type would be wrong without it, and a future "clear profile" verb lands here.)
    pub fn clear(&mut self) {
        self.points.clear();
        self.last_hz = None;
    }

    /// Record `amplitude` as the level at `freq_hz`.
    ///
    /// Applies rules 2 and 3 together: everything the tone crossed getting here is
    /// forgotten, and the point for this frequency line is replaced.
    ///
    /// Returns true when the profile changed. Ignored, silently and by design:
    /// * a non-finite frequency or amplitude, and a frequency at or below zero - there is
    ///   no reading for either, and a survey that has not started commands 0 Hz at 0 V;
    /// * a frequency off the render grid, including one inside half a bin of DC. DC has no
    ///   position on a log-frequency axis and no meaning as a survey tone.
    ///
    /// A refused visit does NOT move the traversal anchor: the tone excursion it belongs
    /// to is still open, so coming back onto the grid wipes across the whole excursion.
    pub fn visit(&mut self, freq_hz: f64, amplitude: f64) -> bool {
        if !(freq_hz.is_finite() && freq_hz > 0.0 && amplitude.is_finite()) {
            return false;
        }
        if !(self.df_hz.is_finite() && self.df_hz > 0.0) || self.n_bins == 0 {
            return false;
        }
        let raw = (freq_hz - self.f0_hz) / self.df_hz;
        if !raw.is_finite() || raw < 1.0 || raw.round() as usize >= self.n_bins {
            return false;
        }
        // RULE 3, the traversal the tone just made, and RULE 2, this frequency's own line.
        // One pass, because the two sets overlap: a stale point can be both inside the
        // span and inside the tolerance, and removing it twice is not a thing a `Vec` does.
        let (lo, hi) = match self.last_hz {
            Some(prev) if prev <= freq_hz => (prev, freq_hz),
            Some(prev) => (freq_hz, prev),
            None => (freq_hz, freq_hz),
        };
        let tol = self.tolerance_hz();
        self.points
            .retain(|(f, _)| !((*f > lo && *f < hi) || (*f - freq_hz).abs() <= tol));
        let at = self.points.partition_point(|(f, _)| *f < freq_hz);
        self.points.insert(at, (freq_hz, amplitude as f32));
        self.last_hz = Some(freq_hz);
        true
    }

    /// The rendered curve on the display grid: adjacent visited points joined by
    /// log-frequency interpolation, NaN outside the outermost visited points.
    ///
    /// `None` while nothing has been visited - an empty profile publishes NO trace rather
    /// than an all-NaN one, so the pane's channel list does not gain a permanently blank
    /// entry before the first tone.
    pub fn render(&self) -> Option<Vec<f32>> {
        let first = *self.points.first()?;
        let last = *self.points.last()?;
        let mut out = vec![f32::NAN; self.n_bins];
        if self.points.len() == 1 {
            // A lone point has no chord to draw. It lands on its nearest bin, which is the
            // only place on this grid it can be said to have been measured.
            let b = ((first.0 - self.f0_hz) / self.df_hz).round();
            if b >= 0.0 && (b as usize) < self.n_bins {
                out[b as usize] = first.1;
            }
            return Some(out);
        }
        // Walk the bins upward alongside the point list: `seg` is the segment
        // `[points[seg], points[seg + 1]]` the current bin falls in, and it only ever
        // advances, so the whole render is one pass over each.
        let mut seg = 0usize;
        let start = ((first.0 - self.f0_hz) / self.df_hz).ceil().max(0.0);
        let start = if start < self.n_bins as f64 {
            start as usize
        } else {
            return Some(out);
        };
        for (b, cell) in out.iter_mut().enumerate().skip(start) {
            let fk = self.f0_hz + b as f64 * self.df_hz;
            if fk > last.0 {
                break;
            }
            if fk < first.0 {
                continue;
            }
            while seg + 2 < self.points.len() && self.points[seg + 1].0 < fk {
                seg += 1;
            }
            let (fa, va) = self.points[seg];
            let (fb, vb) = self.points[seg + 1];
            *cell = interpolate_log(fa, va, fb, vb, fk);
        }
        Some(out)
    }
}

/// The value at `fk` on the straight line joining `(fa, va)` to `(fb, vb)` in LOG
/// frequency.
///
/// Falls back to linear-in-frequency when any of the three frequencies is at or below
/// 0 Hz, where a logarithm has no value. That cannot happen on the Explorer's grid
/// (`visit` refuses anything inside half a bin of DC, and `f0` is 0), and it is here so
/// that a grid with a negative or zero `f0` degrades to a straight line rather than to NaN.
fn interpolate_log(fa: f64, va: f32, fb: f64, vb: f32, fk: f64) -> f32 {
    let log_ok = fa > 0.0 && fb > 0.0 && fk > 0.0;
    let (xa, xb, xk) = if log_ok {
        (fa.ln(), fb.ln(), fk.ln())
    } else {
        (fa, fb, fk)
    };
    let span = xb - xa;
    // A degenerate span would divide by zero; two points a tolerance apart on a positive
    // grid cannot produce one, so the guard picks the near endpoint rather than inventing
    // a midpoint. The clamp keeps a bin that floating-point put a hair outside the segment
    // on the segment.
    let t = if span.abs() > f64::EPSILON {
        ((xk - xa) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (f64::from(va) + t * (f64::from(vb) - f64::from(va))) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 Hz grid, so a frequency in Hz is its own bin index and every assertion below
    /// reads as the frequency it is about.
    fn raster() -> ProfileRaster {
        ProfileRaster::new(0.0, 1.0, 1_001)
    }

    /// RULE 1 - a visit is remembered at its own frequency and nowhere else.
    #[test]
    fn a_visit_lands_on_its_bin_and_the_rest_of_the_grid_stays_empty() {
        let mut r = raster();
        assert!(r.is_empty());
        assert!(r.visit(100.0, 0.5));
        assert!(!r.is_empty());
        let out = r.render().expect("a visited raster renders");
        assert_eq!(out.len(), 1_001);
        assert!((out[100] - 0.5).abs() < 1e-6, "{}", out[100]);
        assert!(out[99].is_nan(), "below the span is NaN, not a floor");
        assert!(out[101].is_nan(), "above the span is NaN, not a floor");
    }

    /// RULE 2 - REVISITING OVERWRITES. The distinguishing case: the second visit is
    /// LOWER than the first, so a max-hold would keep 0.9 and this must report 0.2.
    ///
    /// RED-MUTATION: make `visit` keep the larger of the two and this reds - which is the
    /// state where a single transient overload is welded into the operator's saved graph
    /// for the rest of the run.
    #[test]
    fn revisiting_a_frequency_overwrites_it_downward_too() {
        let mut r = raster();
        r.visit(250.0, 0.9);
        r.visit(250.0, 0.2);
        let out = r.render().unwrap();
        assert!(
            (out[250] - 0.2).abs() < 1e-6,
            "remember-until-overwritten, NOT max-hold: {}",
            out[250]
        );
    }

    /// RULE 2 at the TOLERANCE - the operator's frequency line, not an exact float. Two
    /// clicks that land inside half a display bin of each other are the same line and must
    /// leave ONE point, or the render would carry two values it can never separate.
    ///
    /// RED-MUTATION: drop the tolerance to an exact compare and this reds with two points.
    #[test]
    fn a_revisit_within_half_a_bin_is_the_same_frequency_line() {
        let df = 2.0;
        let mut r = ProfileRaster::new(0.0, df, 1_001);
        assert!((r.tolerance_hz() - 1.0).abs() < 1e-12);
        r.visit(300.0, 1.0);
        r.visit(300.6, 4.0); // inside half a bin: the same line
        assert_eq!(r.points.len(), 1, "{:?}", r.points);
        assert!((r.points[0].1 - 4.0).abs() < 1e-6);
        r.visit(303.0, 2.0); // a bin and a half away: a DIFFERENT line
        assert_eq!(r.points.len(), 2, "{:?}", r.points);
    }

    /// RULE 4 - a coarse jump joins up, and the join is LINEAR IN LOG FREQUENCY.
    ///
    /// 100 Hz -> 400 Hz is two octaves; the geometric midpoint is 200 Hz, so a log fill
    /// puts the half-way VALUE there. A linear-in-Hz fill would put it at 250 Hz instead,
    /// which is what this asserts against.
    #[test]
    fn a_coarse_jump_fills_in_by_log_frequency_interpolation() {
        let mut r = raster();
        r.visit(100.0, 1.0);
        r.visit(400.0, 3.0);
        let out = r.render().unwrap();
        assert!((out[100] - 1.0).abs() < 1e-6);
        assert!((out[400] - 3.0).abs() < 1e-6);
        // Geometric midpoint: value half way between 1.0 and 3.0.
        assert!(
            (out[200] - 2.0).abs() < 1e-3,
            "log-f midpoint is 200 Hz -> 2.0, got {}",
            out[200]
        );
        // The linear-in-Hz midpoint (250 Hz) must NOT be 2.0 - that is the whole
        // difference between the two fills.
        assert!(
            (out[250] - 2.0).abs() > 0.1,
            "a linear-in-Hz fill would put 2.0 at 250 Hz: {}",
            out[250]
        );
        // Monotonic between the endpoints, and no holes.
        let span = &out[100..=400];
        for (i, v) in span.iter().enumerate() {
            assert!(v.is_finite(), "bin {} left unfilled", 100 + i);
        }
        for (i, w) in span.windows(2).enumerate() {
            assert!(w[1] >= w[0], "fill must not wobble at bin {}", 101 + i);
        }
    }

    /// THE COARSE-WALK ACCEPTANCE CASE (operator, 2026-08-08: coarse clicks left the plot
    /// "full of holes"). Three hand clicks, nothing between them, and the curve must be
    /// CONTINUOUS from the first to the last - the reference controller's screen grabs draw a chord
    /// across a two-octave step exactly as they draw one across a fine one.
    ///
    /// RED-MUTATION: render only the bins that were visited and this reds on the very
    /// first unvisited bin, which is the shipped defect.
    #[test]
    fn a_coarse_three_point_walk_renders_gap_free_between_its_endpoints() {
        let mut r = raster();
        for (f, a) in [(100.0, 1.0), (400.0, 4.0), (900.0, 2.0)] {
            assert!(r.visit(f, a));
        }
        let out = r.render().unwrap();
        assert_eq!(r.points.len(), 3, "three clicks, three points");
        for (b, v) in out.iter().enumerate().take(901).skip(100) {
            assert!(v.is_finite(), "bin {b} is a hole in a coarse walk");
        }
        assert!((out[100] - 1.0).abs() < 1e-6);
        assert!((out[400] - 4.0).abs() < 1e-6);
        assert!((out[900] - 2.0).abs() < 1e-6);
        // Rule 4's other half: still nothing outside the walked span.
        assert!(out[99].is_nan());
        assert!(out[901].is_nan());
        // And the chords bracket their endpoints - a bin between two points can only hold
        // a value between their values.
        assert!(out[200] > 1.0 && out[200] < 4.0, "{}", out[200]);
        assert!(out[600] > 2.0 && out[600] < 4.0, "{}", out[600]);
    }

    /// RULE 3 - THE TRAVERSAL WIPE, and the operator's exact complaint. A first pass up
    /// the band leaves points every 100 Hz. The operator then goes BACK to 350 Hz and
    /// walks up again in steps that land nowhere near the first pass's frequencies. The
    /// re-walked span must end up holding only the second walk, and the span the tone
    /// never crossed must be untouched.
    ///
    /// The intermediate assertion is the rule in its starkest form and is stated rather
    /// than hidden: the RETURN to 350 Hz is itself a traversal, so it wipes 400-800 on the
    /// way down before a single new point has been measured. That is the cost of "passing
    /// through a region rewrites it" - a long jump backwards discards the span it crosses
    /// instead of stitching the next pass onto it.
    ///
    /// RED-MUTATION: remove the span half of `visit`'s `retain` (keep only the tolerance
    /// half) and this reds with the first pass's 400/500/600/700/800 Hz points still
    /// there - which is the shipped defect, where a re-walk whose steps miss the old ones
    /// stitches both passes into one curve.
    #[test]
    fn re_walking_a_sub_span_replaces_it_and_leaves_the_rest_alone() {
        let mut r = raster();
        for i in 1..=9 {
            assert!(r.visit(100.0 * i as f64, 1.0));
        }
        assert_eq!(r.points.len(), 9);
        // Back down to 350 Hz: the tone crosses 800, 700, 600, 500, 400 to get there.
        assert!(r.visit(350.0, 5.0));
        let after_return: Vec<f64> = r.points.iter().map(|(f, _)| *f).collect();
        assert_eq!(
            after_return,
            vec![100.0, 200.0, 300.0, 350.0, 900.0],
            "the return trip wipes every line it passes"
        );
        // Then up again, in steps that land between the first pass's points.
        for f in [430.0, 530.0, 650.0] {
            assert!(r.visit(f, 5.0));
        }
        let freqs: Vec<f64> = r.points.iter().map(|(f, _)| *f).collect();
        assert_eq!(
            freqs,
            vec![100.0, 200.0, 300.0, 350.0, 430.0, 530.0, 650.0, 900.0],
            "the crossed span holds ONLY the second walk"
        );
        for (f, v) in r.points.iter() {
            let want = if (350.0..=650.0).contains(f) {
                5.0
            } else {
                1.0
            };
            assert!((v - want).abs() < 1e-6, "{f} Hz holds {v}, wanted {want}");
        }
        // The rendered curve is still gap-free across the whole surveyed span - the
        // 650 -> 900 Hz stretch the second walk has not reached yet is a chord, not a hole.
        let out = r.render().unwrap();
        for (b, v) in out.iter().enumerate().take(901).skip(100) {
            assert!(v.is_finite(), "bin {b} is a hole after a re-walk");
        }
    }

    /// RULE 3, the other direction: walking BACK DOWN through a span wipes it just the
    /// same. The traversal is bounded by two consecutive points whichever way the tone
    /// went, so a downward re-walk is not a special case in the code and must not be one
    /// in the behaviour.
    #[test]
    fn a_downward_re_walk_wipes_the_span_it_crosses_too() {
        let mut r = raster();
        for i in 1..=5 {
            r.visit(100.0 * i as f64, 1.0);
        }
        r.visit(470.0, 7.0); // lands between 400 and 500
        r.visit(230.0, 7.0); // travels DOWN across 400 and 300
        let freqs: Vec<f64> = r.points.iter().map(|(f, _)| *f).collect();
        assert_eq!(freqs, vec![100.0, 200.0, 230.0, 470.0, 500.0], "{freqs:?}");
    }

    /// RULE 4 - outside the visited points, NaN. The span is the survey's extent, not the
    /// grid: a survey that only ever ran 100-400 Hz must not paint a curve over the rest
    /// of the band.
    #[test]
    fn outside_the_visited_span_is_nan_not_a_floor() {
        let mut r = raster();
        r.visit(100.0, 1.0);
        r.visit(400.0, 3.0);
        let out = r.render().unwrap();
        assert!(out[0].is_nan());
        assert!(out[99].is_nan());
        assert!(out[401].is_nan());
        assert!(out[1_000].is_nan());
    }

    /// A VISIT INSIDE A SURVEYED SPAN re-anchors the curve on both sides. This is the
    /// manual-walk case that makes the profile useful: the operator goes back to a
    /// frequency they jumped over, and the chord between the neighbours is replaced by two
    /// shorter chords through the new measurement.
    #[test]
    fn a_visit_inside_a_filled_span_re_anchors_the_interpolation() {
        let mut r = raster();
        r.visit(100.0, 1.0);
        r.visit(400.0, 1.0);
        let flat = r.render().unwrap();
        assert!((flat[200] - 1.0).abs() < 1e-6, "a flat fill between equals");
        r.visit(200.0, 5.0); // the resonance the operator went back for
        let peaked = r.render().unwrap();
        assert!((peaked[200] - 5.0).abs() < 1e-6);
        assert!(peaked[150] > 1.0 && peaked[150] < 5.0, "{}", peaked[150]);
        assert!(peaked[300] > 1.0 && peaked[300] < 5.0, "{}", peaked[300]);
    }

    /// Nothing visited publishes NOTHING - never an all-NaN trace that would sit in the
    /// pane's channel list as a permanently blank entry.
    #[test]
    fn an_empty_raster_renders_no_trace_at_all() {
        assert!(raster().render().is_none());
        // And the inputs that CANNOT be a survey tone leave it empty.
        let mut r = raster();
        assert!(!r.visit(0.0, 1.0), "0 Hz is a parked oscillator");
        assert!(!r.visit(-5.0, 1.0));
        assert!(!r.visit(f64::NAN, 1.0));
        assert!(
            !r.visit(100.0, f64::NAN),
            "a NaN amplitude is not a reading"
        );
        assert!(!r.visit(1e9, 1.0), "past the top bin");
        assert!(!r.visit(0.4, 1.0), "rounds into bin 0, which is DC");
        assert!(r.is_empty());
        assert!(r.render().is_none());
    }

    /// The grid is carried, not assumed: a non-unity `df` maps frequencies to the right
    /// bins. 2.083 Hz bins are what 8533.33 Hz / 4096 actually produces.
    #[test]
    fn a_real_display_grid_maps_frequencies_to_the_right_bins() {
        let df = 8_533.333_333 / 4_096.0;
        let mut r = ProfileRaster::new(0.0, df, 4_096 / 2 + 1);
        r.visit(100.0, 2.0);
        let out = r.render().unwrap();
        let bin = (100.0 / df).round() as usize;
        assert_eq!(bin, 48);
        assert!((out[bin] - 2.0).abs() < 1e-6);
        assert_eq!(r.grid(), (0.0, df, 2_049));
    }

    /// A survey on a REAL grid, walked coarsely, renders every bin between its endpoints -
    /// the 1 Hz test grid could hide an off-by-one that a 2.083 Hz one cannot.
    #[test]
    fn a_coarse_walk_on_a_real_grid_has_no_holes_either() {
        let df = 8_533.333_333 / 4_096.0;
        let mut r = ProfileRaster::new(0.0, df, 4_096 / 2 + 1);
        // Ten percent per click, 200 Hz to about 2 kHz: a reference-controller COARSE walk.
        let mut f = 200.0;
        while f < 2_000.0 {
            assert!(r.visit(f, 1.0 + f / 1_000.0));
            f *= 1.1;
        }
        let out = r.render().unwrap();
        let lo = (200.0 / df).ceil() as usize;
        let hi = (f / 1.1 / df).floor() as usize;
        assert!(
            hi > lo + 500,
            "the walk has to cover a real span: {lo}..{hi}"
        );
        for (b, v) in out.iter().enumerate().take(hi + 1).skip(lo) {
            assert!(v.is_finite(), "bin {b} is a hole in a coarse walk");
        }
        assert!(out[lo - 1].is_nan(), "still nothing below the walk");
        assert!(out[hi + 1].is_nan(), "still nothing above the walk");
    }

    /// The point list cannot grow without bound however long the survey runs: every point
    /// is at least a tolerance from its neighbours, so a full-band walk at a step far finer
    /// than the grid still ends up with about one point per bin.
    #[test]
    fn a_long_survey_cannot_grow_the_point_list_without_bound() {
        let df = 2.0;
        let mut r = ProfileRaster::new(0.0, df, 1_001);
        // Fifty passes over the band at a tenth of a bin per step.
        for _ in 0..50 {
            let mut f = 100.0;
            while f < 900.0 {
                r.visit(f, 1.0);
                f += df / 10.0;
            }
        }
        assert!(
            r.points.len() <= 2 * r.grid().2,
            "{} points on a {}-bin grid",
            r.points.len(),
            r.grid().2
        );
        for w in r.points.windows(2) {
            assert!(
                w[1].0 - w[0].0 > r.tolerance_hz(),
                "{:?} and {:?} are the same frequency line",
                w[0],
                w[1]
            );
        }
    }

    /// Clearing forgets the traversal anchor as well as the points - a profile that kept
    /// the anchor would wipe from the old tone's frequency on the first visit after.
    #[test]
    fn clearing_forgets_the_span_too() {
        let mut r = raster();
        r.visit(100.0, 1.0);
        r.visit(500.0, 1.0);
        r.clear();
        assert!(r.is_empty());
        assert!(r.render().is_none());
        r.visit(300.0, 1.0);
        r.visit(200.0, 1.0);
        assert_eq!(
            r.points.len(),
            2,
            "a cleared profile starts a new traversal"
        );
    }
}
