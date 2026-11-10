/**
 * What a classical shock run offers in each state and ladder mode, and the words for why a verb
 * waits; the level card's words; the pulses picked to lay over the plots and what each page asks
 * the service for.
 */
import { describe, expect, it } from 'vitest';
import type { ShockPulseRow, ShockState } from '$lib/protocol/shock';
import type { View } from '$lib/protocol/types';
import { PAGE_QUANTITIES, toggleOverlay, withOverlay, pulseViewOf } from './shockLayout';
import { shockBarActions, shockModuleVerbs, shockPrimary, type ShockVerbContext } from './shockVerbs';
import { lastPulseWords, nextPulseWords, plannedPulses, pulseWords, shockLevelStatus, shockTripFacts } from './shockWords';

function state(over: Partial<ShockState> = {}): ShockState {
	return {
		run_id: 's-0003',
		state: 'running',
		phase: 'waiting',
		title: 'Half-Sine 20 g 11 ms',
		source: 'SIM',
		station: { kind: 'sim', class: 'sim', devices: [] },
		grid: { fs_hz: 51200, record_len: 16384, actual: true },
		clock: 'shared',
		mode: 'auto',
		levels_db: [-12, -9, -6, -3, 0],
		level_db: -6,
		rung: 2,
		pulses: { fired: 2, passed: 2, at_level: 0, of_level: 1 },
		next_pulse_s: 0.6,
		elapsed_s: 12.5,
		alarm: false,
		identification: null,
		prediction: null,
		last_pulse: null,
		law: null,
		feasibility: null,
		end: null,
		...over
	};
}

function ctx(run: ShockState | null, over: Partial<ShockVerbContext> = {}): ShockVerbContext {
	return { connected: true, controller: true, run, setup: 'applied', setupErrors: 0, busy: null, pending: null, gunfire: false, ...over };
}

function row(over: Partial<ShockPulseRow> = {}): ShockPulseRow {
	return {
		index: 1,
		origin: 'ladder',
		scheduled_db: -12,
		commanded_db: -12,
		limit_scale: 1,
		drive_vpk: 0.1,
		peak_g: 3.75,
		peak_ratio: 0.75,
		error_pct: 25,
		velocity_change_ips: 10,
		velocity_change_ratio: 0.75,
		verdicts: { envelope: false, velocity: false, substitution: true },
		passed: false,
		srs: { bands: 21, passed_bands: 14 },
		at_s: 3,
		law: { used: true, miss: null, capped_bins: 0, peak_capped: false, call_ms: 1.2 },
		...over
	};
}

describe('the run bar for a shock run', () => {
	it('follows the setup to a run: apply, start, proceed, then stop and abort', () => {
		expect(shockPrimary(ctx(null, { setup: 'none' }))).toBe('apply');
		expect(shockPrimary(ctx(state({ state: 'configured', phase: 'idle' })))).toBe('start');
		const review = shockBarActions(ctx(state({ state: 'review', phase: 'awaiting_review' })));
		expect([review.proceed.enabled, review.apply.visible, review.stop.enabled, review.abort.enabled]).toEqual([true, false, true, true]);
		expect(shockPrimary(ctx(state({ state: 'review', phase: 'awaiting_review' })))).toBe('proceed');
		const running = shockBarActions(ctx(state()));
		expect([running.start.visible, running.level.visible, running.mode.visible]).toEqual([false, false, false]);
		expect(shockPrimary(ctx(state({ state: 'finished', phase: 'stopped' })))).toBe('reset');
	});

	it('lets an observer stop and abort, and nothing else', () => {
		const a = shockBarActions(ctx(state(), { controller: false }));
		expect([a.stop.enabled, a.abort.enabled]).toEqual([true, true]);
		const m = shockModuleVerbs(ctx(state({ mode: 'manual' }), { controller: false }));
		expect(m.fire).toEqual({ visible: true, enabled: false, reason: 'Take control to fire.' });
	});

	it('says why Apply and Start wait', () => {
		expect(shockBarActions(ctx(null, { setup: 'none', setupErrors: 2 })).apply.reason).toBe('Fix the 2 errors in the setup first.');
		expect(shockBarActions(ctx(state({ state: 'configured', phase: 'idle' }), { setup: 'changed' })).start.reason).toBe('The setup changed since it was applied. Apply it first.');
		expect(shockBarActions(ctx(state({ state: 'stopping', phase: 'stopping' }))).stop.reason).toMatch(/already stopping/);
	});
});

describe("the manual ladder's verbs", () => {
	it('are not offered while the schedule fires itself', () => {
		const v = shockModuleVerbs(ctx(state()));
		expect(Object.values(v).every((s) => !s.visible)).toBe(true);
	});

	it('wait out the time between pulses and a pulse in flight', () => {
		const manual = (over: Partial<ShockState>) => shockModuleVerbs(ctx(state({ mode: 'manual', ...over })));
		expect(manual({ next_pulse_s: 0.6 }).fire.reason).toBe('The next pulse may fire in 0.6 s.');
		expect(manual({ next_pulse_s: null }).fire.enabled).toBe(true);
		expect(manual({ phase: 'firing', next_pulse_s: null }).train.reason).toBe('A pulse is playing.');
		expect(manual({ phase: 'sysid' }).levelUp.reason).toBe('The level moves once identification is done.');
	});

	it('step the level between the first and last levels of the table', () => {
		const at = (level_db: number) => shockModuleVerbs(ctx(state({ mode: 'manual', level_db, next_pulse_s: null })));
		expect(at(0).levelUp.reason).toBe('At the last level of the table.');
		expect(at(0).levelDown.enabled).toBe(true);
		expect(at(-12).levelDown.reason).toBe('At the first level of the table.');
		// a review hold moves the level too, but fires nothing
		const review = shockModuleVerbs(ctx(state({ mode: 'manual', state: 'review', phase: 'awaiting_review', level_db: -6 })));
		expect([review.levelUp.enabled, review.rung.enabled, review.fire.visible]).toEqual([true, true, false]);
	});

	it('offer the burst only with a gunfire burst configured', () => {
		expect(shockModuleVerbs(ctx(state({ mode: 'manual', next_pulse_s: null }))).burst.visible).toBe(false);
		expect(shockModuleVerbs(ctx(state({ mode: 'manual', next_pulse_s: null }), { gunfire: true })).burst.enabled).toBe(true);
	});
});

describe("the level card's words", () => {
	it('count the pulse coming against the plan, and say when the next is due', () => {
		const planned = plannedPulses([1, 1, 1, 1, 3]);
		expect(planned).toBe(7);
		expect(plannedPulses([2], 5)).toBe(10);
		expect(pulseWords(state(), planned)).toBe('Pulse 3 of 7 at -6.0 dB');
		expect(pulseWords(state({ phase: 'firing', pulses: { fired: 3, passed: 2, at_level: 1, of_level: 1 } }), planned)).toBe('Firing pulse 3 of 7 at -6.0 dB');
		expect(pulseWords(state({ pulses: { fired: 8, passed: 6, at_level: 2, of_level: 3 }, level_db: 0 }), planned)).toBe('Pulse 9 at 0.0 dB, 2 past the 7 planned');
		expect(pulseWords(state({ mode: 'manual' }), planned)).toBe('Pulse 3 at -6.0 dB');
		expect(pulseWords(state({ phase: 'sysid' }), planned)).toBeNull();
		expect(nextPulseWords(state())).toBe('Next pulse in 0.6 s');
		expect(nextPulseWords(state({ mode: 'manual', next_pulse_s: 0 }))).toBe('Ready to fire');
		expect(nextPulseWords(state({ phase: 'firing' }))).toBeNull();
	});

	it('say how the last pulse went, and that the law corrects it', () => {
		expect(lastPulseWords(row(), true)).toEqual({ text: 'Pulse 2 was 25% low; correcting', tone: 'warn' });
		expect(lastPulseWords(row({ peak_ratio: 1.12 }), false)?.text).toBe('Pulse 2 was 12% high');
		expect(lastPulseWords(row({ passed: true, error_pct: 1.9, peak_ratio: 0.981, verdicts: { envelope: true, velocity: true, substitution: true } }), true)).toEqual({ text: 'Pulse 2 passed, 1.9% error', tone: 'pass' });
		expect(lastPulseWords(row({ peak_ratio: 0.99, verdicts: { envelope: true, velocity: false, substitution: true } }), true)?.text).toBe('Pulse 2 missed the velocity change; correcting');
		expect(lastPulseWords(row({ origin: 'train' }), true)?.text).toBe('Pulse 2 was 25% low');
	});

	it('name the hold, the firing and the end', () => {
		expect(shockLevelStatus(state({ state: 'review', phase: 'awaiting_review' })).text).toBe('Holding for review');
		expect(shockLevelStatus(state({ phase: 'firing' })).text).toBe('Firing at -6.0 dB');
		expect(shockLevelStatus(state({ mode: 'manual' })).text).toBe('Waiting for Fire at -6.0 dB');
		const done = shockLevelStatus(state({ state: 'finished', phase: 'stopped', end: { outcome: 'not_converged', message: '', at_s: 20, level_db: -12, pulse: 5, trip: null } }));
		expect(done).toEqual({ mark: 'ended', text: 'Stopped: The Pulse Did Not Converge', tone: 'fail' });
	});

	it("give a trip's facts, a predicted one said so", () => {
		const facts = shockTripFacts({ kind: 'drive_vpk', pulse: 0, at_level_db: 0, vpk: 0.92, abort_vpk: 0.8, predicted: true });
		expect(facts.map((f) => f.value)).toEqual(['Before the first pulse', '0.0 dB', '0.92 V pk', '0.8 V pk', 'On the prediction, before the pulse fired']);
		expect(shockTripFacts({ kind: 'not_converged', pulse: 5, at_level_db: -12, limit: 'per_level', pulses: 5 })[2].value).toBe('Pulses at one level without a pass');
	});
});

describe('the pages and the pulses laid over them', () => {
	const spectrum = (title: string, unit: string, xUnit: string, names: [string, 'measured' | 'reference'][]): View => ({
		view: 'spectrum',
		title,
		x: { label: 'x', unit: xUnit, scale: xUnit === 'Hz' ? 'log' : 'linear' },
		y: { label: 'y', unit, scale: 'linear' },
		series: names.map(([name, role]) => ({ name, role, x: new Float64Array([0, 1]), y: new Float64Array([1, 2]) }))
	});

	it('picks pulses in order, takes one off when picked again, and keeps the newest past the limit', () => {
		let p: number[] = [];
		for (const i of [3, 5, 3, 1, 2]) p = toggleOverlay(p, i, 3);
		expect(p).toEqual([5, 1, 2]);
		expect(toggleOverlay(p, 7, 3)).toEqual([1, 2, 7]);
	});

	it("lays each picked pulse's measured series over the page, named for its pulse", () => {
		const base = spectrum('Pulse', 'g', 'ms', [['Reference', 'reference'], ['Measured', 'measured']]);
		const pulse2 = [spectrum('Pulse', 'g', 'ms', [['Reference', 'reference'], ['Measured', 'measured']]), spectrum('SRS', 'g', 'Hz', [['Measured', 'measured']])];
		const v = withOverlay(base as never, [{ index: 1, views: pulse2 }], 'pulse');
		expect(v?.series.map((s) => s.name)).toEqual(['Reference', 'Measured', 'Pulse 2, measured']);
		expect(withOverlay(base as never, [{ index: 1, views: pulse2 }], 'srs')?.series.at(-1)?.name).toBe('Pulse 2, measured');
		expect(pulseViewOf(pulse2, 'srs')?.title).toBe('SRS');
		expect(withOverlay(null, [], 'pulse')).toBeNull();
	});

	it('the pages hold the quantities each kind drew', () => {
		expect(PAGE_QUANTITIES.identification).toEqual(['identification', 'spectrum']);
	});
});
