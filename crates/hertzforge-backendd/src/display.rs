//! The SHARED display packers: the time-domain batches every module host publishes on
//! the one display bus.
//!
//! # Why this module exists
//!
//! These two functions and their two constants lived as private items inside
//! [`crate::random`], which made them Random's by accident of where they were written
//! rather than by anything about what they do: neither one knows what a PSD is, what a
//! level ladder is, or which module produced the samples. They take an interleaved
//! block, a channel count, a rate and a run-relative frame clock, and they answer with
//! the decimated shapes the wire packer understands.
//!
//! The Explorer host needs the same shapes on the same bus. Copying them would have
//! produced two decimators that drift - and the specific thing they would drift on is
//! the `t_end_s` clock contract, which is exactly the frozen-clock bug the Random path
//! once had (48/48 wire frames carrying a constant `tEnd`, every remote time pane's
//! follow window frozen). One copy, one contract.
//!
//! The move is MECHANICAL: the bodies, the constants and the pinning test are
//! byte-identical to what `random.rs` carried.
//!
//! # [`TimeTap`] arrived the same way
//!
//! It was written inside [`crate::explorer`] and moved here when the Sine host
//! needed the same accumulator: buffer drained acquisition blocks, pack once
//! [`TAP_MIN_SPAN`] of signal is held, publish with the service mutex RELEASED. Not
//! one line of it is Explorer-specific - it knows about frames, a stride, a rate and a
//! run-relative clock, and nothing about a survey.
//!
//! The move was mechanical in the same sense: the struct, its impl and its doc comments
//! are BYTE-IDENTICAL to what `explorer.rs` carried, with only the visibility words
//! changed, and the moved body was byte-diffed against the pre-move bytes rather than
//! eyeballed. The reason to move it rather than copy it is the reason above, sharpened:
//! a second tap would drift on the `cum_frames` contract, and a drifted `cum_frames` is
//! the frozen-clock bug again - a time pane whose follow window never advances, which
//! looks like a dead stream rather than like a bug in a packer.
//!
//! # The `t_end_s` contract, stated once
//!
//! `cum_frames` is the RUN-RELATIVE frame count through the END of the block being
//! packed - never the block's own length. A packer handed the block length stamps the
//! same `t_end_s` on every batch, which reads to a follow-window as a time base that
//! never advances.

use std::time::Duration;

use crate::recorder::{DisplayBatch, ScopeBatch, WaveBatch};

/// Envelope columns per second on the remote scope stream (mirrors the local
/// scope's decimation density).
pub const SCOPE_COLS_PER_SEC: f64 = 240.0;
/// Decimated waveform sample rate target, Hz. Display-grade: boxcar-averaged
/// (see [`wave_batch`]) - honest for a time-pane polyline, NOT measurement data.
pub const WAVE_RATE_HZ: f64 = 2000.0;

/// Min/max envelope columns for one interleaved segment.
///
/// `cum_frames` = run-relative frames through the END of this segment; it is
/// the batch's time base (`t_end_s`), NOT the segment length - see the module
/// docs.
pub fn scope_batch(
    raw: &[f32],
    n_ch: usize,
    fs: f64,
    run_id: u64,
    cum_frames: u64,
) -> Option<ScopeBatch> {
    let frames = raw.len() / n_ch.max(1);
    if frames == 0 || fs <= 0.0 {
        return None;
    }
    let per_col = ((fs / SCOPE_COLS_PER_SEC).round() as usize).max(1);
    let n_cols = frames.div_ceil(per_col);
    let mut channels = Vec::with_capacity(n_ch);
    for c in 0..n_ch {
        let mut mins = Vec::with_capacity(n_cols);
        let mut maxs = Vec::with_capacity(n_cols);
        let mut sq = 0.0f64;
        let mut peak = 0.0f32;
        for col in 0..n_cols {
            let lo = col * per_col;
            let hi = ((col + 1) * per_col).min(frames);
            let mut mn = f32::INFINITY;
            let mut mx = f32::NEG_INFINITY;
            for f in lo..hi {
                let v = raw[f * n_ch + c];
                mn = mn.min(v);
                mx = mx.max(v);
                sq += (v as f64) * (v as f64);
                peak = peak.max(v.abs());
            }
            mins.push(mn);
            maxs.push(mx);
        }
        let rms = (sq / frames as f64).sqrt() as f32;
        channels.push(((c as u16) + 1, rms, peak, mins, maxs));
    }
    Some(ScopeBatch {
        dt_s: (per_col as f64 / fs) as f32,
        t_end_s: cum_frames as f64 / fs,
        run_id,
        channels,
    })
}

/// Boxcar-decimated samples for one interleaved segment. DISPLAY-GRADE: a
/// boxcar is a weak anti-alias filter; this feeds a time-pane polyline, never
/// analysis. (A shared proper decimator with the Tauri layer is the upgrade
/// path.)
pub fn wave_batch(
    raw: &[f32],
    n_ch: usize,
    fs: f64,
    run_id: u64,
    cum_frames: u64,
) -> Option<WaveBatch> {
    let dec = ((fs / WAVE_RATE_HZ).round() as usize).max(1);
    wave_batch_dec(raw, n_ch, fs, run_id, cum_frames, dec)
}

/// [`wave_batch`] at an EXPLICIT decimation factor.
///
/// # Why a transient producer needs this
///
/// [`WAVE_RATE_HZ`]'s 2 kHz target is right for a continuous producer and hostile to a
/// transient one. At 2 kHz a 6 ms pulse is TWELVE points and an 11 ms half-sine is 22,
/// so the peak the whole shock test is about lands between samples and the pane draws a
/// triangle. Inside a pulse window the shock hosts pass `dec = 1`: at the 51 200 Hz this
/// chassis runs, the same 6 ms pulse is 307 points and the 11 ms one is 563. That is
/// affordable precisely BECAUSE the window is bounded by the pulse - a transient
/// producer publishes tens of milliseconds and then stops, where a continuous one would
/// be putting 51 200 samples per second per channel on the wire forever.
///
/// `wave_batch` is now this function at the 2 kHz-derived factor, so there is still ONE
/// decimator and ONE `t_end_s` contract (the module docs' whole point).
pub fn wave_batch_dec(
    raw: &[f32],
    n_ch: usize,
    fs: f64,
    run_id: u64,
    cum_frames: u64,
    dec: usize,
) -> Option<WaveBatch> {
    let frames = raw.len() / n_ch.max(1);
    if frames == 0 || fs <= 0.0 {
        return None;
    }
    let dec = dec.max(1);
    let n_out = frames / dec;
    if n_out == 0 {
        return None;
    }
    let mut channels = Vec::with_capacity(n_ch);
    for c in 0..n_ch {
        let mut out = Vec::with_capacity(n_out);
        for k in 0..n_out {
            let lo = k * dec;
            let hi = (lo + dec).min(frames);
            let mut acc = 0.0f32;
            for f in lo..hi {
                acc += raw[f * n_ch + c];
            }
            out.push(acc / (hi - lo) as f32);
        }
        channels.push(((c as u16) + 1, out));
    }
    // The newest EMITTED sample: the boxcar drops the segment's tail
    // remainder (frames % dec), so the batch's clock ends there, aligned to
    // the same run-relative frame count the scope batch rides.
    let tail = (frames - n_out * dec) as u64;
    Some(WaveBatch {
        dt_s: (dec as f64 / fs) as f32,
        t_end_s: cum_frames.saturating_sub(tail) as f64 / fs,
        run_id,
        channels,
    })
}

/// The TIME-DOMAIN tap's minimum publish span, stated in TIME.
///
/// The scope/wave batches are packed from whatever acquisition blocks the pass drained,
/// and one iteration on the chassis is one ~30 ms block - so on hardware this floor is
/// already met by the first block and the tap publishes EVERY iteration, which is the live
/// tap's whole point (the display must not wait a segment's worth of samples). It exists
/// for the other end of the range: an HERTZFORGE-SIM station's natural block is ONE FRAME, and
/// publishing per iteration there would put ~1000 single-column batches per second on the
/// wire. 15 ms matches the frame span the Random path measured after its live tap landed
/// (79 frames/s of 15 ms each), so the two modules' time streams beat at the same rate.
///
/// It is a FLOOR, never a ceiling: nothing is held back once the span is reached.
pub const TAP_MIN_SPAN: Duration = Duration::from_millis(15);

/// THE TIME-DOMAIN DISPLAY TAP (feature: "Time is a must, highest priority").
///
/// # What this fixes
///
/// The Explorer daemon published SPECTRA and nothing else, so every remote time pane sat
/// blank for the whole run while the spectrum panes filled - a deferral, taken because
/// there was no non-competing raw tap to read. There is one now: each host's
/// `service_analysis_tapped` (`ExplorerHostService`'s, and `SineService`'s since the Sine
/// host landed) hands over the blocks it already drained, so this reads no second copy of
/// the device task and competes with nothing.
///
/// (The link on that method name was resolved from `explorer.rs` and is a plain code
/// span here, because this module deliberately depends on neither host - which is the
/// property that let it be shared in the first place. The only edit the move made to
/// this doc comment.)
///
/// # The clock contract (the constant-clock trap, and why this is not the block length)
///
/// `cum_frames` is the RUN-RELATIVE frame count through the END of what is buffered, and
/// it is what [`crate::display::scope_batch`] / [`crate::display::wave_batch`] stamp
/// `t_end_s` from. Handing them the block length instead stamps a CONSTANT `t_end_s` on
/// every batch, which reads to a follow window as a time base that never advances - 48/48
/// wire frames measured that way before the Random path was fixed. Monotonic by
/// construction: it only ever adds.
///
/// # Cadence
///
/// The pump iterates once per bounded tick batch and this publishes on that beat once
/// [`TAP_MIN_SPAN`] of signal is held - never a segment's worth. On the chassis a
/// single ~30 ms block clears the floor, so the beat IS the block rate.
pub struct TimeTap {
    run_id: u64,
    fs_hz: f64,
    /// Interleave width of the buffered frames. `0` until the first block arrives.
    n_ch: usize,
    /// Interleaved frames accumulated since the last publish.
    buf: Vec<f32>,
    /// Run-relative frames through the END of `buf`. See the clock contract above.
    cum_frames: u64,
    /// Frames one publish must hold ([`TAP_MIN_SPAN`] at `fs_hz`).
    min_frames: usize,
    /// Packed batches waiting for the pump to publish them. Packing happens under the
    /// service mutex (it is inside the analysis pass); SENDING happens after the pump
    /// releases it, so a slow broadcast can never lengthen the hold.
    out: Vec<DisplayBatch>,
}

impl TimeTap {
    /// A tap for one run at `fs_hz`. A rate that does not exist publishes nothing
    /// rather than garbage - see the `min_frames` arm below.
    ///
    /// (Doc comment ADDED by the move here: the original was a private method and
    /// `missing_docs` does not reach those. The BODY is byte-identical.)
    pub fn new(run_id: u64, fs_hz: f64) -> Self {
        let min_frames = if fs_hz.is_finite() && fs_hz > 0.0 {
            (TAP_MIN_SPAN.as_secs_f64() * fs_hz).ceil().max(1.0) as usize
        } else {
            usize::MAX // a rate that does not exist publishes nothing, rather than garbage
        };
        TimeTap {
            run_id,
            fs_hz,
            n_ch: 0,
            buf: Vec::new(),
            cum_frames: 0,
            min_frames,
            out: Vec::new(),
        }
    }

    /// Accept one drained acquisition block. Packs when the buffered span is due.
    pub fn push(&mut self, block: &[f32], stride: usize) {
        let stride = stride.max(1);
        if block.is_empty() || !(self.fs_hz.is_finite() && self.fs_hz > 0.0) {
            return;
        }
        // A stride change means the acquired frame geometry changed under us, which no
        // longer describes the samples already buffered. Drop them rather than pack a
        // batch that interleaves two shapes.
        if stride != self.n_ch {
            self.buf.clear();
            self.n_ch = stride;
        }
        self.buf.extend_from_slice(block);
        self.cum_frames += (block.len() / stride) as u64;
        if self.frames() >= self.min_frames {
            self.pack();
        }
    }

    /// Account for frames the analysis ring DROPPED (drop-oldest overflow) without
    /// inventing samples for them.
    ///
    /// The clock advances past the hole, so the loss surfaces as a GAP in the time pane
    /// (the wave ring breaks the trace on a forward jump) instead of as silently
    /// compressed time. Drop-oldest means the lost blocks are older than everything still
    /// in the ring, so whatever is buffered is packed FIRST - it predates the hole.
    pub fn skip(&mut self, frames: u64) {
        if frames == 0 {
            return;
        }
        self.pack();
        self.cum_frames += frames;
    }

    /// Pack whatever is buffered, whether or not the span floor is met (a run that ends
    /// must not strand its last samples).
    pub fn flush(&mut self) {
        self.pack();
    }

    /// Frames currently buffered.
    ///
    /// (Doc comment ADDED by the move here, as for [`TimeTap::new`]. The BODY is
    /// byte-identical.)
    pub fn frames(&self) -> usize {
        // `checked_div` rather than a guarded `/`: `n_ch` is 0 until the first block
        // arrives, and 0 frames is the right answer then.
        self.buf.len().checked_div(self.n_ch).unwrap_or(0)
    }

    fn pack(&mut self) {
        if self.frames() == 0 {
            return;
        }
        if let Some(sb) = crate::display::scope_batch(
            &self.buf,
            self.n_ch,
            self.fs_hz,
            self.run_id,
            self.cum_frames,
        ) {
            self.out.push(DisplayBatch::Scope(sb));
        }
        // `wave_batch` answers `None` when the buffer holds fewer frames than the
        // decimation factor - honest, and unreachable at the shipped floor (15 ms of
        // signal is many multiples of `fs / 2000`).
        if let Some(wb) = crate::display::wave_batch(
            &self.buf,
            self.n_ch,
            self.fs_hz,
            self.run_id,
            self.cum_frames,
        ) {
            self.out.push(DisplayBatch::Wave(wb));
        }
        self.buf.clear();
    }

    /// Publish everything packed since the last call. Called with the service mutex
    /// RELEASED.
    pub fn publish(&mut self, tx: &tokio::sync::broadcast::Sender<DisplayBatch>) {
        for b in self.out.drain(..) {
            // Non-blocking: a lagging subscriber drops frames, it never stalls the pump.
            let _ = tx.send(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clock-contract pin: batch t_end_s is RUN-relative and
    /// advances across ticks - never the constant segment length. 48/48 wire
    /// frames measured carrying the same tEnd before this was fixed.
    ///
    /// Moved here with the functions: the contract belongs beside the
    /// packers, not beside one of the two modules that call them.
    #[test]
    fn batch_t_end_is_cumulative_and_advances() {
        let fs = 1000.0;
        let seg: Vec<f32> = vec![0.0; 500]; // 500 frames @ 1 ch = 0.5 s
        let a = scope_batch(&seg, 1, fs, 1, 500).unwrap();
        let b = scope_batch(&seg, 1, fs, 1, 1000).unwrap();
        assert!(
            (a.t_end_s - 0.5).abs() < 1e-9,
            "first tick ends at 0.5 s: {}",
            a.t_end_s
        );
        assert!(
            (b.t_end_s - 1.0).abs() < 1e-9,
            "second tick ends at 1.0 s: {}",
            b.t_end_s
        );
        assert!(b.t_end_s > a.t_end_s, "t_end must ADVANCE");
        // Waveform: dec = 1000/2000 -> 1 (clamped); no tail at dec=1.
        let wa = wave_batch(&seg, 1, fs, 1, 500).unwrap();
        let wb = wave_batch(&seg, 1, fs, 1, 1000).unwrap();
        assert!(wb.t_end_s > wa.t_end_s, "wave t_end must ADVANCE");
        // Tail alignment: fs 10k -> dec 5; 503 frames -> n_out 100, tail 3.
        let seg2: Vec<f32> = vec![0.0; 503];
        let wt = wave_batch(&seg2, 1, 10_000.0, 1, 503).unwrap();
        assert!(
            (wt.t_end_s - 0.05).abs() < 1e-9,
            "tail-trimmed clock: {}",
            wt.t_end_s
        );
    }

    /// **THE DISCONTIGUOUS-PRODUCER ROW**, added when the shock hosts landed.
    ///
    /// A transient producer's `cum_frames` does NOT advance by the packed segment
    /// length: the silence between two pulses is real elapsed time nobody sampled, and
    /// the shock tap adds it to the clock without packing samples for it. So the packers
    /// have to be correct for a `cum_frames` that JUMPED - which is the case the
    /// clock-contract row above never exercised, because the Random and Explorer taps
    /// only ever add what they packed.
    ///
    /// What this pins: the packers denominate `t_end_s` from `cum_frames` alone and
    /// never from the buffer they were handed, so a jump appears as a jump rather than
    /// being silently compressed out. A wave ring breaks its trace on that forward jump,
    /// which is the honest rendering of "nothing was driving here".
    ///
    /// RED-MUTATION: stamp `t_end_s` from the segment length instead and both the jump
    /// and the clock-contract row above red.
    #[test]
    fn a_discontiguous_producer_clock_jumps_by_the_gap_it_was_given() {
        let fs = 51_200.0;
        // Pulse 1: 2304 frames, published as one segment ending at frame 2304.
        let pulse: Vec<f32> = vec![0.5; 2304];
        let a = scope_batch(&pulse, 1, fs, 3, 2304).unwrap();
        // 2.0 s of SILENCE nobody sampled: the clock advances by 102 400 frames and the
        // next pulse's segment is the same length as the first.
        let gap_frames = (2.0 * fs) as u64;
        let b = scope_batch(&pulse, 1, fs, 3, 2304 + gap_frames + 2304).unwrap();
        let elapsed = b.t_end_s - a.t_end_s;
        assert!(
            (elapsed - (2.0 + 2304.0 / fs)).abs() < 1e-9,
            "the gap must appear in the clock: {elapsed} s between two pulses 2.0 s apart"
        );
        // The BATCH is still only the pulse - the gap is not packed as samples.
        assert_eq!(b.channels[0].3.len(), a.channels[0].3.len());
        // And the raw-rate wave path carries the same jump.
        let wa = wave_batch_dec(&pulse, 1, fs, 3, 2304, 1).unwrap();
        let wb = wave_batch_dec(&pulse, 1, fs, 3, 2304 + gap_frames + 2304, 1).unwrap();
        assert!((wb.t_end_s - wa.t_end_s - (2.0 + 2304.0 / fs)).abs() < 1e-9);
        // dec = 1 means NO tail trim and one output sample per input frame - the
        // property the shock pane depends on for a 6 ms pulse to be 307 points and not
        // 12 (see `wave_batch_dec`).
        assert_eq!(wa.channels[0].1.len(), 2304);
        assert!((wa.dt_s as f64 - 1.0 / fs).abs() < 1e-12);
        // ANTI-VACUITY: the shipped 2 kHz factor really does decimate this record, so
        // the assertion above is about dec = 1 and not about the record being short.
        let shipped = wave_batch(&pulse, 1, fs, 3, 2304).unwrap();
        assert!(
            shipped.channels[0].1.len() < 200,
            "at 2 kHz a 45 ms record is ~90 points: {}",
            shipped.channels[0].1.len()
        );
    }
}
