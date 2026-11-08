/**
 * The words of a TWR run: the level card's status line in each state and phase, the play in flight
 * and its progress through the record, the iteration against the level's budget, the playback
 * against its schedule, the last play against the gate, a play's alignment, and a trip's facts.
 */
import { describe, expect, it } from 'vitest';
import type { TwrPlayRow, TwrPlaysCount, TwrTrip } from '$lib/protocol/twr';
import {
	driftWords,
	errorAgainstGate,
	iterationWords,
	lastPlayWords,
	lawCallWords,
	levelWords,
	playbackWords,
	playProgress,
	playWords,
	twrLevelStatus,
	twrTripFacts,
	type TwrRunNow
} from './twrWords';

const LEVELS = [-12, -9, -6, -3, 0];
const plays = (over: Partial<TwrPlaysCount> = {}): TwrPlaysCount => ({ done: 3, at_level: 1, of_level: 8, playback: 0, of_playback: 15, ...over });

function now(over: Partial<TwrRunNow> = {}): TwrRunNow {
	return { state: 'running', phase: 'waiting', mode: 'auto', levels_db: LEVELS, level_db: -6, rung: 2, plays: plays(), identification: null, end: null, frozen: false, play: null, ...over };
}

const row = (over: Partial<TwrPlayRow> = {}): TwrPlayRow => ({
	index: 2,
	kind: 'iteration',
	level_db: -6,
	rung: 2,
	iteration: 2,
	rms_error_pct: 4.2,
	peak_error_pct: 9,
	spectral_error_db: 0.3,
	gated: true,
	clipped_samples: 0,
	clamped_bins: 0,
	drive_vpk: 0.1,
	response_peak: 0.1,
	lag_frames: 41.02,
	drift_frames: 0.02,
	start_frame: 0,
	frames: 65536,
	at_s: 12,
	law: null,
	...over
});

describe('the TWR level card', () => {
	it('says what the run is doing, in the plan words', () => {
		const at = (o: Partial<TwrRunNow>) => twrLevelStatus(now(o)).text;
		expect(at({ phase: 'iterating', play: { index: 3, kind: 'iteration', iteration: 2, frames_done: 0, frames: 65536 } })).toBe('Correcting at -6.0 dB, iteration 2 of 8');
		expect(at({ phase: 'playing', level_db: 0, rung: 4, plays: plays({ playback: 2 }) })).toBe('Playing 3 of 15 at full level');
		expect(at({ state: 'held', phase: 'coherence_hold' })).toBe('Holding: the identification did not reach its coherence floor');
		expect(at({ phase: 'sysid' })).toBe('Identifying the system with band-limited random');
		expect(twrLevelStatus(now({ phase: 'sysid' }), 'log_chirp').text).toBe('Identifying the system with a log chirp');
		expect(at({ mode: 'manual' })).toBe('Waiting for Iterate or Play at -6.0 dB');
		expect(at({})).toBe('Between plays at -6.0 dB');
		expect(at({ phase: 'holding', plays: plays({ at_level: 8 }) })).toBe('Holding at -6.0 dB: the level did not meet its gate in 8 plays');
		expect(at({ state: 'review', phase: 'awaiting_review' })).toBe('Holding for review');
		expect(at({ state: 'stopping', phase: 'stopping' })).toBe('Stopping: the play in flight comes to rest');
	});

	it('the iteration counts the play in flight; between plays, the plays done', () => {
		expect(iterationWords(plays({ at_level: 1 }), { index: 3, kind: 'iteration', iteration: 2, frames_done: 10, frames: 100 })).toBe('Iteration 2 of 8 at this level');
		expect(iterationWords(plays({ at_level: 1 }), null)).toBe('1 of 8 correction plays at this level');
	});

	it('the playback counts only once playback has begun', () => {
		expect(playbackWords(plays(), null)).toBeNull();
		expect(playbackWords(plays(), { index: 7, kind: 'playback', iteration: null, frames_done: 0, frames: 100 })).toBe('Playback 1 of 15');
		expect(playbackWords(plays({ playback: 4 }), null)).toBe('Playback 4 of 15');
	});

	it('the play in flight, its kind, level and progress through the record', () => {
		const p = { index: 4, kind: 'iteration' as const, iteration: 2, frames_done: 16384, frames: 65536 };
		expect(playWords(now({ play: p }))).toBe('Play 5, correction 2 at -6.0 dB');
		expect(playWords(now({ play: { ...p, kind: 'playback', iteration: null }, level_db: 0 }))).toBe('Play 5, playback at full level');
		expect(playProgress(p)).toBe(0.25);
		expect(playProgress(null)).toBeNull();
		expect(playProgress({ ...p, frames: 0 })).toBeNull();
	});

	it('the last play against the gate, and in a sentence', () => {
		expect(errorAgainstGate({ rms_error_pct: 4.2, peak_error_pct: 9, spectral_error_db: 0.3, gated: true }, 10)).toEqual({ text: '4.2% rms error, gate 10%: met', tone: 'pass' });
		expect(errorAgainstGate({ rms_error_pct: 18, peak_error_pct: 30, spectral_error_db: 1, gated: false }, 10)?.tone).toBe('warn');
		expect(lastPlayWords(row(), LEVELS)?.text).toBe('Play 3 met the gate at -6.0 dB, 4.2% rms error');
		expect(lastPlayWords(row({ gated: false, rms_error_pct: 18 }), LEVELS)?.text).toBe('Play 3 missed the gate at -6.0 dB, 18% rms error; correcting');
	});

	it('full level is the table top at 0 dB; another top level is its dB', () => {
		expect(levelWords(0, LEVELS)).toBe('full level');
		expect(levelWords(3, [-3, 0, 3])).toBe('+3.0 dB');
		expect(levelWords(null, LEVELS)).toBe('-');
	});

	it("a play's alignment, never a 0 that was not measured", () => {
		expect(driftWords(row())).toBe('0.02 frames');
		expect(driftWords(row({ lag_frames: null, drift_frames: null }))).toBe('Not yet measured');
	});

	it('the law call behind a play', () => {
		expect(lawCallWords(row())).toBe('In force');
		expect(lawCallWords(row({ law: { used: true, miss: null, call_ms: 3.2, peak_rise_db: 0.4, bin_rise_db: 1, predicted_peak_ratio: 1, values: {} } }))).toBe('3.2 ms');
		expect(lawCallWords(row({ law: { used: false, miss: 'over_budget', call_ms: 1800, peak_rise_db: null, bin_rise_db: null, predicted_peak_ratio: null, values: {} } }))).toBe('Missed: The control law took longer than its time budget');
	});
});

describe("a TWR trip's facts", () => {
	it('names where and what for every kind', () => {
		const trips: TwrTrip[] = [
			{ kind: 'envelope', play: 3, at_level_db: -6, in_play: true, worst_frac: 0.61, abort_frac: 0.5, samples: 8 },
			{ kind: 'drive_vpk', play: null, at_level_db: 0, vpk: 0.2, ceiling_vpk: 0.15, predicted: true },
			{ kind: 'drive_rms', play: 1, at_level_db: -12, vrms: 0.4, limit_vrms: 0.3 },
			{ kind: 'response_rms', play: 1, at_level_db: -12, rms: 2.1, limit: 2 },
			{ kind: 'alignment', play: 2, at_level_db: -9, lag_frames: 43.1, identification_lag_frames: 41, drift_frames: 2.1, limit_frames: 1 },
			{ kind: 'coverage', play: null, at_level_db: null, coverage: 0.82, min_band_energy: 0.9, error_floor_pct: 42 },
			{ kind: 'divergence', play: 5, at_level_db: -3, plays: 3, error_pct: 30 },
			{ kind: 'not_converged', play: 8, at_level_db: -12, iterations: 8, error_pct: 50.1, gate_pct: 10 },
			{ kind: 'law_fault', play: 2, at_level_db: -12, reason: 'over_budget', consecutive: 3 }
		];
		for (const t of trips) {
			const f = twrTripFacts(t);
			expect(f[0].label).toBe('Play');
			expect(f.length).toBeGreaterThan(2);
			for (const x of f) expect(x.value).not.toMatch(/undefined|NaN/);
		}
		expect(twrTripFacts(trips[1]).find((x) => x.label === 'Found')?.value).toBe('On the prediction, before any play');
		expect(twrTripFacts(trips[1])[0].value).toBe('Before the first play');
		expect(twrTripFacts(trips[0]).find((x) => x.label === 'Found')?.value).toBe('During the play, from a block');
	});
});
