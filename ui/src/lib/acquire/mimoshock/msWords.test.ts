/**
 * MIMO Shock's words and the verdict grid: every verdict carried by mark and word, the reasons by
 * name, the run's end in plain words for every way it can end (both abort lines named), the ladder
 * with its half step and a halved step, and the hold before the first pulse.
 */
import { describe, expect, it } from 'vitest';
import type { MimoShockControlVerdict, MimoShockEnd, MimoShockPulseRow } from '$lib/protocol/mimoShock';
import { verdictGrid } from './msGrid';
import { abortLineOf, endWords, ladderRungs, lastPulseWords, needsAcknowledgement, reasonsWords, rungWord, scaledCrossings, scaledCrossingWords, otherWarnings, signedDb, spreadWarning, stepNoteWords } from './msWords';

function control(channel: number, over: number, reasons: MimoShockControlVerdict['reasons'] = [], scaled = -2): MimoShockControlVerdict {
	return {
		channel, passed: reasons.length === 0, reasons, error_db: -20, over_test_db: over, delay_s: 0.0003, peak: 10, peak_ratio: 1, scaled_margin_db: scaled, absolute_margin_db: scaled - 6,
		timing_ok: !reasons.includes('timing'), polarity_ok: !reasons.includes('polarity'), duration_ratio: null, duration_ok: true, energy_db: 0, rms_duration_ratio: 1, velocity_ips: 1, displacement_pk_pk_in: 0.1, predicted_peak_ratio: 1, srs: null
	};
}

function pulse(index: number, level: number, controls: MimoShockControlVerdict[], extra: Partial<MimoShockPulseRow> = {}): MimoShockPulseRow {
	return {
		index, scheduled_db: level, level_db: level, passed: controls.every((c) => c.passed), at_s: index, drives: [], controls, over_test_db: Math.max(...controls.map((c) => c.over_test_db)),
		global_srs_error_db: null, arrival_spread_s: 0.0001, warnings: [], step_note: null, law: { used: true, miss: null, call_ms: 1, delays_s: null, values: [] }, ...extra
	};
}

describe('the verdict grid', () => {
	const rows = [pulse(0, -12, [control(1, 0.2), control(2, 1.6, ['tolerance'])]), pulse(1, -12, [control(1, -0.1), control(2, 0.4, ['timing', 'polarity'], 0.7)])];

	it('lays controls down and pulses across, each cell a mark and a word', () => {
		const g = verdictGrid(rows, [1, 2]);
		expect(g.controls).toEqual([1, 2]);
		expect(g.columns.map((c) => [c.index, c.passed])).toEqual([[0, false], [1, false]]);
		expect(g.cells.map((r) => r.map((c) => c?.text))).toEqual([['Pass', 'Pass'], ['Tol', 'Time Pol']]);
		// the reasons by name, and the signed error with over-test positive, in the cell's words
		expect(g.cells[1][0]?.label).toBe('Control 2, pulse 1: fail (Tolerance), +1.6 dB');
		expect(g.cells[1][1]?.label).toBe('Control 2, pulse 2: fail (Timing, Polarity), +0.4 dB, over its scaled abort line');
		expect(g.cells[1][1]?.scaledOver).toBe(true);
	});

	it('shows the latest pulses when there are more than fit, and an empty cell for a control not judged', () => {
		const many = Array.from({ length: 30 }, (_, i) => pulse(i, -6, [control(1, 0)]));
		const g = verdictGrid(many, [1, 9], 24);
		expect([g.columns[0].index, g.hidden]).toEqual([6, 6]);
		expect(g.cells[1].every((c) => c === null)).toBe(true);
	});
});

describe('the words', () => {
	it('signs an error, over-test positive', () => {
		expect([signedDb(1.25), signedDb(-0.4), signedDb(-0.01), signedDb(null)]).toEqual(['+1.3 dB', '-0.4 dB', '0.0 dB', '-']);
		expect(reasonsWords(['tolerance', 'timing', 'duration'])).toBe('Tolerance, timing and duration');
	});

	it('says how a pulse went, and flags the controls over their scaled line by how far', () => {
		const row = pulse(4, -6, [control(1, 0.1), control(2, 2.9, ['tolerance'], 0.62), control(3, 2.4, ['tolerance'], 0.12)]);
		expect(lastPulseWords(row)).toEqual({ text: 'Pulse 5 at -6.0 dB did not pass: 1 of 3 controls inside tolerance', tone: 'warn' });
		expect(scaledCrossings(row)).toEqual([{ channel: 2, over_db: 0.62 }, { channel: 3, over_db: 0.12 }]);
		expect(scaledCrossingWords(row)).toBe('Pulse 5 went over the scaled abort line at control 2 by 0.62 dB, control 3 by 0.12 dB.');
		expect(scaledCrossingWords(pulse(0, -6, [control(1, 0)]))).toBeNull();
	});

	it('warns when the arrivals spread past the window', () => {
		expect(spreadWarning(pulse(0, -6, [control(1, 0)], { arrival_spread_s: 0.003 }), 0.002)).toBe("The controls' arrivals spread over 3 ms, past the 2 ms window.");
		expect(spreadWarning(pulse(0, -6, [control(1, 0)], { arrival_spread_s: 0.001 }), 0.002)).toBeNull();
	});

	it("says why the run ended, in plain words, for each of the engine's reasons", () => {
		const end = (outcome: MimoShockEnd['outcome'], reason: string | null, pulse: number | null = 6): MimoShockEnd => ({ outcome, reason, message: 'the service words', at_s: 20, level_db: -3, pulse });
		expect(endWords(end('not_converged', 'not_converged'))).toBe("A level's pulse allowance was spent without every control passing. At pulse 7, -3.0 dB.");
		expect(endWords(end('safe_stop', 'divergence'))).toMatch(/^The divergence watch stopped the run/);
		expect(endWords(end('safe_stop', 'control_abort_scaled'))).toMatch(/scaled to the level played/);
		expect(endWords(end('aborted', 'control_abort_absolute'))).toMatch(/absolute abort line/);
		expect(endWords(end('law_fault', 'peak_rise'))).toBe("The control law gave no usable drives: a drive's peak grew more than the cap allows in one pulse. At pulse 7, -3.0 dB.");
		expect(endWords(end('held', 'low_coherence', null))).toMatch(/coherence\. Before the first pulse\.$/);
		expect(endWords(end('completed', null))).toBe('Full level passed its pulses. At pulse 7, -3.0 dB.');
		// a reason the shell does not know reads the service's own words
		expect(endWords(end('fault', 'unknown'))).toMatch(/^the service words/);
	});

	it('names the abort line that ended the run, scaled or absolute', () => {
		const end = (outcome: MimoShockEnd['outcome'], reason: string): MimoShockEnd => ({ outcome, reason, message: '', at_s: 1, level_db: -6, pulse: 2 });
		expect(abortLineOf(end('safe_stop', 'control_abort_scaled'))).toBe('scaled');
		expect(abortLineOf(end('aborted', 'control_abort_absolute'))).toBe('absolute');
		expect(abortLineOf(end('aborted', 'drive_abort'))).toBeNull();
		expect(abortLineOf(null)).toBeNull();
	});
});

describe('the ladder', () => {
	const levels = [-12, -9, -6, -3, -1.5, 0];

	it('counts the pulses fired and passed at each level, and marks the level in force', () => {
		const rows = [pulse(0, -12, [control(1, 0)]), pulse(1, -9, [control(1, 2, ['tolerance'])]), pulse(2, -9, [control(1, 0)])];
		const r = ladderRungs(levels, rows, -6);
		expect(r.map((x) => [x.db, x.fired, x.passed, x.now])).toEqual([[-12, 1, 1, false], [-9, 2, 1, false], [-6, 0, 0, true], [-3, 0, 0, false], [-1.5, 0, 0, false], [0, 0, 0, false]]);
		expect(r.map((x) => rungWord(x, levels))).toEqual(['', '', '', '', 'Half step', 'Full level']);
	});

	it('shows a step the engine halved, with its note', () => {
		const note = { from_db: -6, planned_db: -3, taken_db: -4.5, predicted_planned_db: 1.6, predicted_taken_db: 0.9, tolerance_db: 1.2 };
		const rows = [pulse(0, -6, [control(1, 0)], { step_note: note })];
		const r = ladderRungs(levels, rows, -4.5);
		const halved = r.find((x) => x.db === -4.5)!;
		expect([halved.halved, halved.now, rungWord(halved, levels)]).toEqual([true, true, 'Halved step']);
		expect(stepNoteWords(rows[0])).toBe('The ladder stepped to -4.5 dB instead of -3.0 dB: the first pulses predicted +1.6 dB at -3.0 dB, past the 1.2 dB tolerance (+0.9 dB predicted at -4.5 dB).');
	});
});

describe('the hold before the first pulse', () => {
	it('waits for an acknowledgement only while the unreachable hold stands unanswered', () => {
		const review = { controls: [], hold: { reason: 'unreachable_target' as const, message: '' }, acknowledged: null };
		expect(needsAcknowledgement({ state: 'review', review })).toBe(true);
		expect(needsAcknowledgement({ state: 'review', review: { ...review, acknowledged: 'known short' } })).toBe(false);
		expect(needsAcknowledgement({ state: 'review', review: { ...review, hold: null } })).toBe(false);
		expect(needsAcknowledgement({ state: 'running', review })).toBe(false);
	});
});

describe("the service's other warnings on a pulse", () => {
	it('leave out what the card already says, in the words the engine uses, and read as sentences', () => {
		const engine = [
			'the controls arrive 2.80 ms apart, over the 2.00 ms window',
			'control 2 crossed the abort line scaled to -6 dB',
			'controls 1, 3 crossed the abort line scaled to -6 dB',
			"control 4's rms duration is 1.16 times its target's",
			"control 1 has +1.20 dB of its target's energy"
		];
		expect(otherWarnings(engine, { spread: true, crossing: true })).toEqual(["Control 4's rms duration is 1.16 times its target's.", "Control 1 has +1.20 dB of its target's energy."]);
		// what the card does not flag stays
		expect(otherWarnings(engine.slice(0, 2), { spread: false, crossing: false })).toEqual(['The controls arrive 2.80 ms apart, over the 2.00 ms window.', 'Control 2 crossed the abort line scaled to -6 dB.']);
	});
});
