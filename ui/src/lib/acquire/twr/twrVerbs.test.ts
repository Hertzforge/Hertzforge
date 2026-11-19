/**
 * What a TWR run offers in each state and ladder mode, and why a verb waits: the run bar's verbs,
 * the module's own (Iterate, Play, the level verbs, Freeze and Resume), and which test type the
 * workspace counts as live.
 */
import { describe, expect, it } from 'vitest';
import type { TwrPlayRow, TwrState } from '$lib/protocol/twr';
import { liveTestOf } from '../testTypes.svelte';
import { levelGated, twrBarActions, twrModuleVerbs, twrPrimary, type TwrVerbContext, type TwrVerbRun } from './twrVerbs';

const row = (over: Partial<TwrPlayRow> = {}): TwrPlayRow => ({
	index: 0,
	kind: 'iteration',
	level_db: -12,
	rung: 0,
	iteration: 1,
	rms_error_pct: 4,
	peak_error_pct: 9,
	spectral_error_db: 0.2,
	gated: true,
	clipped_samples: 0,
	clamped_bins: 0,
	drive_vpk: 0.05,
	response_peak: 0.05,
	lag_frames: 41,
	drift_frames: 0,
	start_frame: 0,
	frames: 65536,
	at_s: 4,
	law: null,
	...over
});

function run(over: Partial<TwrVerbRun> = {}): TwrVerbRun {
	return { state: 'running', phase: 'waiting', mode: 'manual', levels_db: [-12, -9, -6, -3, 0], level_db: -12, frozen: false, last_play: null, ...over };
}

function ctx(r: TwrVerbRun | null, over: Partial<TwrVerbContext> = {}): TwrVerbContext {
	return { connected: true, controller: true, run: r, setup: 'applied', setupErrors: 0, busy: null, pending: null, ...over };
}

const shown = (v: Record<string, { visible: boolean }>) => Object.keys(v).filter((k) => v[k].visible);
const ok = (v: Record<string, { enabled: boolean }>) => Object.keys(v).filter((k) => v[k].enabled);

describe("the TWR run bar's verbs", () => {
	it('follow the run from setup to its end, with no level or mode verbs of the bar', () => {
		const at = (state: TwrState['state'], phase: TwrState['phase'] = 'idle') => shown(twrBarActions(ctx(run({ state, phase }))));
		expect(at('configured')).toEqual(['apply', 'arm', 'start']);
		expect(at('blocked', 'feasibility_blocked')).toEqual(['apply', 'override']);
		expect(at('armed', 'armed')).toEqual(['start', 'stop', 'abort']);
		expect(at('review', 'awaiting_review')).toEqual(['proceed', 'stop', 'abort']);
		expect(at('running', 'iterating')).toEqual(['stop', 'abort']);
		expect(at('finished', 'stopped')).toEqual(['apply', 'reset']);
	});

	it('the coherence hold is live, and only New Run releases it', () => {
		const c = ctx(run({ state: 'held', phase: 'coherence_hold' }));
		expect(shown(twrBarActions(c))).toEqual(['stop', 'abort', 'reset']);
		expect(twrPrimary(c)).toBe('reset');
		expect(twrBarActions(ctx(run({ state: 'held', phase: 'coherence_hold' }), { controller: false })).reset.reason).toBe('Take control to start a new run.');
	});

	it('Stop and Abort need no control', () => {
		const a = twrBarActions(ctx(run({ phase: 'playing' }), { controller: false }));
		expect(ok(a)).toEqual(['stop', 'abort']);
	});

	it('names the primary verb of each state', () => {
		const p = (state: TwrState['state'], setup: TwrVerbContext['setup'] = 'applied') => twrPrimary(ctx(run({ state }), { setup }));
		expect([p('unconfigured', 'none'), p('configured'), p('configured', 'changed'), p('armed'), p('review'), p('running'), p('finished')]).toEqual(['apply', 'start', 'apply', 'start', 'proceed', null, 'reset']);
	});
});

describe("the TWR module's verbs", () => {
	it('on the manual ladder between plays: Iterate, the level verbs and Freeze; Play waits for the gate', () => {
		const v = twrModuleVerbs(ctx(run()));
		expect(shown(v)).toEqual(['iterate', 'play', 'levelDown', 'levelUp', 'rung', 'freeze']);
		expect(ok(v)).toEqual(['iterate', 'levelUp', 'rung', 'freeze']);
		expect(v.play.reason).toBe('Once a play at this level meets the gate.');
		expect(v.levelDown.reason).toBe('At the first level of the table.');
	});

	it('Play once the last play gated the level the run is at, and not after a level move', () => {
		expect(twrModuleVerbs(ctx(run({ last_play: row() }))).play.enabled).toBe(true);
		expect(twrModuleVerbs(ctx(run({ level_db: -11, last_play: row() }))).play.reason).toBe('Once a play at this level meets the gate.');
		expect(twrModuleVerbs(ctx(run({ last_play: row({ gated: false }) }))).play.enabled).toBe(false);
		expect(levelGated({ level_db: -12, last_play: row({ kind: 'playback' }) })).toBe(false);
	});

	it('wait while a play is in flight or identification runs', () => {
		const flight = twrModuleVerbs(ctx(run({ phase: 'iterating' })));
		expect([flight.iterate.reason, flight.play.reason, flight.levelUp.reason]).toEqual(['A play is in flight.', 'A play is in flight.', 'The level moves between plays.']);
		const sysid = twrModuleVerbs(ctx(run({ phase: 'sysid', level_db: null })));
		expect([sysid.iterate.reason, sysid.levelUp.reason]).toEqual(['Identification is running.', 'The level moves once identification is done.']);
	});

	it('the automatic ladder offers no Iterate or Play, the level verbs only at a level held for its gate, and Freeze', () => {
		expect(shown(twrModuleVerbs(ctx(run({ mode: 'auto', phase: 'iterating' }))))).toEqual(['freeze']);
		expect(shown(twrModuleVerbs(ctx(run({ mode: 'auto', phase: 'holding' }))))).toEqual(['levelDown', 'levelUp', 'rung', 'freeze']);
	});

	it('the level moves during the review hold on the manual ladder; at the last level it does not rise', () => {
		const v = twrModuleVerbs(ctx(run({ state: 'review', phase: 'awaiting_review', level_db: 0 })));
		expect(v.levelDown.enabled).toBe(true);
		expect(v.levelUp.reason).toBe('At the last level of the table.');
		expect(v.iterate.visible).toBe(false);
	});

	it('Freeze turns to Resume while frozen; both need control', () => {
		const v = twrModuleVerbs(ctx(run({ frozen: true })));
		expect([v.freeze.visible, v.unfreeze.visible]).toEqual([false, true]);
		expect(twrModuleVerbs(ctx(run(), { controller: false })).freeze.reason).toBe('Take control to freeze the corrections.');
	});

	it('nothing outside a running or held-for-review run', () => {
		for (const state of ['configured', 'armed', 'held', 'stopping', 'finished'] as const) expect(shown(twrModuleVerbs(ctx(run({ state }))))).toEqual([]);
	});
});

describe('the live test type', () => {
	const acq = { run: { state: 'configured' } as never, syncError: null };
	const none = { status: null, syncError: 'unknown_method' };
	it('is TWR while its run is live, its coherence hold included', () => {
		for (const state of ['armed', 'running', 'review', 'held', 'stopping'] as const) expect(liveTestOf(acq, 'open', none, none, none, none, { status: { state }, syncError: null })).toBe('twr');
		expect(liveTestOf(acq, 'open', none, none, none, none, { status: { state: 'finished' }, syncError: null })).toBeNull();
	});

	it('waits for the TWR state to be read', () => {
		expect(liveTestOf(acq, 'open', none, none, none, none, { status: null, syncError: null })).toBeUndefined();
	});
});
