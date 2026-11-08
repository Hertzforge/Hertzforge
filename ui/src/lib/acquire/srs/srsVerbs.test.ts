/**
 * What an SRS shock run offers in each state and ladder mode, and the words for why a verb waits;
 * the level card's words; the pulses picked to lay over the plots and what each page asks the
 * service for.
 */
import { describe, expect, it } from 'vitest';
import type { SrsPulseRow, SrsState } from '$lib/protocol/srs';
import type { View } from '$lib/protocol/types';
import { defaultSrsLayout, SRS_PAGE_QUANTITIES, srsLayoutFromSaved, srsLayoutToSaved, srsPagePlots, srsSubscribeParams, toggleOverlay, withOverlay } from './srsLayout';
import { srsBarActions, srsModuleVerbs, srsPrimary, type SrsVerbContext } from './srsVerbs';
import { lastPulseWords, nextPulseWords, notchWords, pulseWords, srsLevelStatus, srsTripFacts, worstBandWords } from './srsWords';

function state(over: Partial<SrsState> = {}): SrsState {
	return {
		run_id: 'r-0004',
		state: 'running',
		phase: 'waiting',
		title: 'SRS 100 g, 100 to 2000 Hz',
		source: 'SIM',
		station: { kind: 'sim', class: 'sim', devices: [] },
		grid: { fs_hz: 51200, record_len: 14336, actual: true },
		clock: 'shared',
		mode: 'auto',
		levels_db: [-12, -6, 0],
		level_db: -6,
		rung: 1,
		pulses: { fired: 2, passed: 1, at_level: 0, of_level: 2 },
		next_pulse_s: 0.6,
		elapsed_s: 9.5,
		alarm: false,
		identification: null,
		prediction: null,
		notch: null,
		last_pulse: null,
		law: null,
		feasibility: null,
		end: null,
		...over
	};
}

function ctx(run: SrsState | null, over: Partial<SrsVerbContext> = {}): SrsVerbContext {
	return { connected: true, controller: true, run, setup: 'applied', setupErrors: 0, busy: null, pending: null, ...over };
}

function row(over: Partial<SrsPulseRow> = {}): SrsPulseRow {
	return {
		index: 1,
		origin: 'ladder',
		scheduled_db: -6,
		commanded_db: -6,
		limit_scale: 1,
		drive_vpk: 0.205,
		peak_g: 36.3,
		velocity_pk_ips: 7.1,
		displacement_pk_pk_in: 0.016,
		worst_err_db: -1.2,
		worst_band_hz: 630,
		alarm_bands: 0,
		abort_bands: 0,
		passed: true,
		limited_components: null,
		worst_limit_db: null,
		at_s: 7.5,
		law: { used: true, miss: null, call_ms: 0.2 },
		...over
	};
}

describe('the run bar for an SRS run', () => {
	it('follows the setup to a run: apply, start, proceed, then stop and abort', () => {
		expect(srsPrimary(ctx(null, { setup: 'none' }))).toBe('apply');
		expect(srsPrimary(ctx(state({ state: 'configured', phase: 'idle' })))).toBe('start');
		expect(srsPrimary(ctx(state({ state: 'review', phase: 'awaiting_review' })))).toBe('proceed');
		const live = srsBarActions(ctx(state()));
		expect([live.stop.enabled, live.abort.enabled, live.apply.visible, live.level.visible, live.mode.visible]).toEqual([true, true, false, false, false]);
		expect(srsPrimary(ctx(state({ state: 'finished', phase: 'stopped' })))).toBe('reset');
	});

	it('lets an observer stop and abort, and nothing else', () => {
		const a = srsBarActions(ctx(state({ state: 'review', phase: 'awaiting_review' }), { controller: false }));
		expect([a.stop.enabled, a.abort.enabled, a.proceed.enabled]).toEqual([true, true, false]);
		expect(a.proceed.reason).toBe('Take control to proceed.');
	});
});

describe("the manual ladder's verbs", () => {
	it('are not offered while the schedule fires itself, and never offer a train or a burst', () => {
		const auto = srsModuleVerbs(ctx(state()));
		expect(Object.values(auto).some((v) => v.visible)).toBe(false);
		const manual = srsModuleVerbs(ctx(state({ mode: 'manual', next_pulse_s: 0 })));
		expect([manual.fire.enabled, manual.levelUp.enabled, manual.levelDown.enabled, manual.rung.enabled]).toEqual([true, true, true, true]);
		expect([manual.train.visible, manual.burst.visible]).toEqual([false, false]);
	});

	it('wait out the time between pulses, a pulse in flight and identification', () => {
		expect(srsModuleVerbs(ctx(state({ mode: 'manual', next_pulse_s: 0.6 }))).fire.reason).toBe('The next pulse may fire in 0.6 s.');
		expect(srsModuleVerbs(ctx(state({ mode: 'manual', phase: 'firing', next_pulse_s: null }))).fire.reason).toBe('A pulse is playing.');
		expect(srsModuleVerbs(ctx(state({ mode: 'manual', phase: 'sysid', next_pulse_s: null }))).levelUp.reason).toBe('The level moves once identification is done.');
	});

	it('step the level between the first and last levels of the table, and at review', () => {
		expect(srsModuleVerbs(ctx(state({ mode: 'manual', level_db: 0 }))).levelUp.reason).toBe('At the last level of the table.');
		expect(srsModuleVerbs(ctx(state({ mode: 'manual', level_db: -12 }))).levelDown.reason).toBe('At the first level of the table.');
		const review = srsModuleVerbs(ctx(state({ mode: 'manual', state: 'review', phase: 'awaiting_review' })));
		expect([review.fire.visible, review.levelUp.enabled]).toEqual([false, true]);
	});
});

describe("the level card's words", () => {
	it('count the pulse coming against the plan, and say when the next is due', () => {
		expect(pulseWords(state(), 7)).toBe('Pulse 3 of 7 at -6.0 dB');
		expect(pulseWords(state({ phase: 'firing', pulses: { fired: 2, passed: 1, at_level: 0, of_level: 2 } }), 7)).toBe('Firing pulse 2 of 7 at -6.0 dB');
		expect(pulseWords(state({ mode: 'manual' }), 7)).toBe('Pulse 3 at -6.0 dB');
		expect(nextPulseWords(state())).toBe('Next pulse in 0.6 s');
		expect(nextPulseWords(state({ mode: 'manual', next_pulse_s: 0 }))).toBe('Ready to fire');
	});

	it("name the last pulse's worst band and whether it passed", () => {
		expect(worstBandWords(row())).toBe('Worst band 630 Hz, 1.2 dB low');
		expect(worstBandWords(row({ worst_err_db: 2.5, worst_band_hz: 1250 }))).toBe('Worst band 1250 Hz, 2.5 dB high');
		expect(worstBandWords(row({ worst_err_db: null }))).toBeNull();
		expect(lastPulseWords(row(), true)?.text).toBe('Pulse 2 passed: every band within the alarm band');
		expect(lastPulseWords(row({ passed: false, alarm_bands: 3 }), true)?.text).toBe('Pulse 2: 3 bands outside the alarm band; correcting');
		expect(lastPulseWords(row({ passed: false, alarm_bands: 1 }), false)?.text).toBe('Pulse 2: 1 band outside the alarm band');
		expect(lastPulseWords(row({ passed: null }), true)?.tone).toBe('info');
	});

	it('name the hold, identification, the firing and the end', () => {
		expect(srsLevelStatus(state({ state: 'review', phase: 'awaiting_review' })).text).toBe('Holding for review');
		expect(srsLevelStatus(state({ phase: 'sysid' })).text).toBe('Identifying the system with a quiet burst');
		expect(srsLevelStatus(state({ phase: 'firing' })).text).toBe('Firing at -6.0 dB');
		const end = (outcome: NonNullable<SrsState['end']>['outcome']) => srsLevelStatus(state({ state: 'finished', phase: 'stopped', end: { outcome, message: '', at_s: 1, level_db: 0, pulse: 3, trip: null } }));
		expect([end('completed').text, end('completed').mark]).toEqual(['Completed', 'done']);
		expect(end('not_converged').text).toBe('Stopped: The Spectrum Did Not Converge');
		expect(end('law_fault').text).toBe('Stopped: No Usable Gains From the Control Law');
	});

	it("give a trip's facts, a predicted one said so", () => {
		const drive = srsTripFacts({ kind: 'drive_vpk', pulse: 0, at_level_db: 0, vpk: 0.412, abort_vpk: 0.38, predicted: true });
		expect(drive).toContainEqual({ label: 'Pulse', value: 'Before the first pulse' });
		expect(drive).toContainEqual({ label: 'Found', value: 'On the prediction, before the pulse fired' });
		const env = srsTripFacts({ kind: 'envelope', pulse: 4, at_level_db: -6, abort_bands: 2, worst_band_hz: 1600, worst_err_db: -9.4 });
		expect(env).toContainEqual({ label: 'Its error', value: '9.4 dB low' });
		const law = srsTripFacts({ kind: 'law_fault', pulse: 3, at_level_db: -12, reason: 'gain_rise', consecutive: 3 });
		expect(law).toContainEqual({ label: 'Why', value: "A component's gain rose more than the cap allows in one pulse" });
		const kin = srsTripFacts({ kind: 'kinematic', pulse: 5, at_level_db: 0, axis: 'accel', value: 210, limit: 200, predicted: true });
		expect(kin).toContainEqual({ label: 'Acceleration', value: '210 g' });
	});

	it('say what the limit notch did', () => {
		expect(notchWords(null)).toBeNull();
		expect(notchWords({ notched_bands: 2, worst_before_db: 3.1, worst_after_db: -0.6, converged: true })).toBe('2 bands notched; worst prediction 3.1 dB over its allowed level before, 0.6 dB under after');
		expect(notchWords({ notched_bands: 0, worst_before_db: -2, worst_after_db: -2, converged: true })).toMatch(/No band needed a notch/);
	});
});

describe('the pages and the pulses laid over them', () => {
	const spectrum = (title: string, unit: string, xUnit: string, names: [string, 'measured' | 'reference'][]): View => ({
		view: 'spectrum',
		title,
		x: { label: 'x', unit: xUnit, scale: xUnit === 'Hz' ? 'log' : 'linear' },
		y: { label: 'y', unit, scale: 'log' },
		series: names.map(([name, role]) => ({ name, role, x: new Float64Array([100, 200]), y: new Float64Array([1, 2]) }))
	});

	it('picks pulses in order, takes one off when picked again, and keeps the newest past the limit', () => {
		let p: number[] = [];
		for (const i of [0, 4, 0, 2, 3]) p = toggleOverlay(p, i, 3);
		expect(p).toEqual([4, 2, 3]);
	});

	it("lays each picked pulse's measured SRS over the SRS page, named for its pulse", () => {
		const base = spectrum('SRS', 'g', 'Hz', [['Target', 'reference'], ['Measured', 'measured']]);
		const pulse3 = [spectrum('SRS', 'g', 'Hz', [['Target', 'reference'], ['Measured', 'measured']]), spectrum('Pulse', 'g', 'ms', [['Measured', 'measured']])];
		expect(withOverlay(base as never, [{ index: 2, views: pulse3 }], 'srs')?.series.map((s) => s.name)).toEqual(['Target', 'Measured', 'Pulse 3, measured']);
	});

	it('the eight pages by default, each asking only for what it draws, a point count on the time histories only', () => {
		const layout = defaultSrsLayout();
		expect(layout.pages.map((p) => p.name)).toEqual(['SRS', 'Pulse', 'Kinematics', 'Identification', 'Limit Channels', 'Components', 'Pulses', 'Law']);
		expect(layout.pages.map((p) => p.id)).toEqual(['rp-1', 'rp-2', 'rp-3', 'rp-4', 'rp-5', 'rp-6', 'rp-7', 'rp-8']);
		expect(SRS_PAGE_QUANTITIES.limit_channels).toEqual(['limit_channels', 'limit_frf']);
		expect(srsSubscribeParams([], 2)).toEqual({ default: true });
		const sent = srsSubscribeParams([...srsPagePlots(layout.pages[0], 800), ...srsPagePlots(layout.pages[1], 800), ...srsPagePlots(layout.pages[4], 800)], 2);
		expect('plots' in sent && sent.plots).toEqual([
			{ id: 'rp-1:srs', quantity: 'srs' },
			{ id: 'rp-2:pulse', quantity: 'pulse', points: 800 },
			{ id: 'rp-5:limit_channels', quantity: 'limit_channels' },
			{ id: 'rp-5:limit_frf', quantity: 'limit_frf' }
		]);
		expect(srsLayoutFromSaved(srsLayoutToSaved(layout))).toEqual(layout);
		// a classical shock layout's page kinds are not SRS's
		expect(srsLayoutFromSaved('{"version":1,"rate":2,"serial":1,"pages":[{"id":"sp-1","name":"Spectrum","kind":"spectrum"}]}')).toBeNull();
	});
});
