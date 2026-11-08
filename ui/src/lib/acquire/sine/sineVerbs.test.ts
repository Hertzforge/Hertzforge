/**
 * What a Sine run offers, by state and by test kind: the run bar's verbs (Random's rules), Sine's
 * own (Hold or Resume, and on a sweep Reverse, Jog and Go to), the law parameters changeable during
 * a run, and the law panel's marking of them.
 */
import { describe, expect, it } from 'vitest';
import type { LawEntry } from '$lib/protocol/mimo';
import type { SineState } from '$lib/protocol/sine';
import { lawParamDescriptor, lawValues, liveParamLabels } from '../law/lawForm';
import { liveParams, liveValueError, sineBarActions, sineModuleVerbs, sinePrimary, type SineVerbContext } from './sineVerbs';

const run = (over: Partial<Pick<SineState, 'state' | 'phase' | 'mode' | 'test' | 'held'>> = {}) => ({ state: 'running' as const, phase: 'engaged' as const, mode: 'auto' as const, test: 'sweep' as const, held: false, ...over });

function ctx(over: Partial<SineVerbContext> = {}): SineVerbContext {
	return { connected: true, controller: true, run: run(), setup: 'applied', setupErrors: 0, busy: null, pending: null, ...over };
}

const shown = (v: Record<string, { visible: boolean; enabled: boolean }>) =>
	Object.entries(v)
		.filter(([, s]) => s.visible)
		.map(([k, s]) => `${k}${s.enabled ? '' : '-'}`);

const SERVO_BLEND: LawEntry = {
	id: 'servo_blend',
	world: 'sine',
	builtin: true,
	name: 'Servo blend',
	version: '0.1.0',
	author: 'Hertzforge contributors',
	summary: '',
	sha256: null,
	check: { status: 'built_in', date: null, tool_version: null, cases: [], hardware: true, message: '' },
	params: [
		{ key: 'blend_w', label: 'Prediction weight', help: 'h', kind: 'number', unit: '', min: 0, max: 1, step: 0.05, default: 0.5 },
		{ key: 'compression', label: 'Compression', help: 'h', kind: 'choice', options: ['slow', 'fast', 'custom'], default: 'slow' },
		{ key: 'ki', label: 'Integral gain', help: 'h', kind: 'number', unit: '', min: 0.1, max: 20, step: 0.1, default: 2 },
		{ key: 'steps', label: 'Steps', help: 'h', kind: 'integer', unit: '', min: 1, max: 9, step: 1, default: 3 },
		{ key: 'on', label: 'On', help: 'h', kind: 'toggle', default: true }
	],
	diagnostics: [],
	shapes: null,
	needs: null,
	live: ['blend_w', 'steps']
};

describe('the Sine verbs', () => {
	it("offer the run bar's verbs by Random's rules, the level by the ladder mode", () => {
		expect(shown(sineBarActions(ctx({ run: null, setup: 'none' })))).toEqual(['apply']);
		expect(shown(sineBarActions(ctx({ run: run({ state: 'configured', phase: 'idle' }) })))).toEqual(['apply-', 'arm', 'start', 'mode']);
		expect(sinePrimary(ctx({ run: run({ state: 'configured', phase: 'idle' }) }))).toBe('start');
		expect(sinePrimary(ctx({ run: run({ state: 'review', phase: 'awaiting_review' }) }))).toBe('proceed');
		const auto = sineBarActions(ctx());
		expect(shown(auto)).toEqual(['level-', 'mode', 'stop', 'abort']);
		expect(auto.level.reason).toBe('Switch the ladder to manual to step the level.');
		expect(sineBarActions(ctx({ run: run({ mode: 'manual' }) })).level.enabled).toBe(true);
		expect(sineBarActions(ctx({ run: run({ mode: 'manual', phase: 'sysid' }) })).level.reason).toBe('The level moves once the run is driving at the schedule.');
		expect(shown(sineBarActions(ctx({ controller: false })))).toEqual(['level-', 'mode-', 'stop', 'abort']);
		expect(shown(sineBarActions(ctx({ run: run({ state: 'finished', phase: 'stopped' }) })))).toEqual(['apply-', 'reset']);
	});

	it('offer Hold or Resume while running, and Reverse, Jog and Go to on a sweep only', () => {
		expect(shown(sineModuleVerbs(ctx()))).toEqual(['hold', 'reverse', 'jog', 'goto']);
		expect(shown(sineModuleVerbs(ctx({ run: run({ held: true }) })))).toEqual(['resume', 'reverse', 'jog', 'goto']);
		expect(shown(sineModuleVerbs(ctx({ run: run({ phase: 'full_level' }) })))).toEqual(['hold', 'reverse', 'jog', 'goto']);
		expect(shown(sineModuleVerbs(ctx({ run: run({ test: 'dwell' }) })))).toEqual(['hold']);
		expect(shown(sineModuleVerbs(ctx({ run: run({ test: 'step' }) })))).toEqual(['hold']);
		expect(shown(sineModuleVerbs(ctx({ run: run({ test: 'dwell', held: true }) })))).toEqual(['resume']);
		for (const state of ['configured', 'armed', 'review', 'stopping', 'finished'] as const) expect(shown(sineModuleVerbs(ctx({ run: run({ state }) })))).toEqual([]);
		expect(shown(sineModuleVerbs(ctx({ run: null })))).toEqual([]);
	});

	it('say why a verb waits: identification, an observer, a request in flight, no connection', () => {
		const sysid = sineModuleVerbs(ctx({ run: run({ phase: 'sysid' }) }));
		expect(shown(sysid)).toEqual(['hold-', 'reverse-', 'jog-', 'goto-']);
		expect(sysid.hold.reason).toBe('Once the tone is driving at the schedule.');
		expect(sineModuleVerbs(ctx({ controller: false })).reverse.reason).toBe('Take control to turn the sweep.');
		expect(sineModuleVerbs(ctx({ controller: false, run: run({ held: true }) })).resume.reason).toBe('Take control to resume the test.');
		expect(sineModuleVerbs(ctx({ busy: 'Jog' })).goto.reason).toBe('Waiting for Jog.');
		expect(sineModuleVerbs(ctx({ connected: false })).hold.reason).toBe('The control service is not connected.');
	});

	it("open only the law's live number parameters to a change, once the tone is driving, to the screen in control", () => {
		const values = { blend_w: 0.4, ki: 3 };
		const at = (over: Partial<SineVerbContext>) => liveParams(SERVO_BLEND, { live: { blend_w: 0.25 } }, values, ctx(over)).map((p) => [p.key, p.live, p.enabled, p.value, p.reason]);
		expect(at({})).toEqual([
			['blend_w', true, true, 0.25, null],
			['steps', true, true, 3, null],
			['ki', false, false, 3, 'Fixed for the run.']
		]);
		expect(at({ run: run({ state: 'review', phase: 'awaiting_review' }) }).map((p) => [p[2], p[4]])).toEqual([
			[false, 'Once the tone is driving at the schedule.'],
			[false, 'Once the tone is driving at the schedule.'],
			[false, 'Fixed for the run.']
		]);
		expect(at({ run: run({ phase: 'sysid' }) }).map((p) => p[2])).toEqual([false, false, false]);
		expect(at({ run: run({ phase: 'full_level' }) }).map((p) => p[2])).toEqual([true, true, false]);
		expect(at({ run: run({ state: 'configured', phase: 'idle' }) }).map((p) => p[4])).toEqual(['Changed during a run.', 'Changed during a run.', 'Fixed for the run.']);
		expect(at({ run: run({ state: 'stopping', phase: 'ramping_down' }) }).map((p) => p[2])).toEqual([false, false, false]);
		expect(at({ controller: false })[0][4]).toBe('Take control to change it.');
		expect(at({ busy: 'Change Parameter' })[0][4]).toBe('Waiting for Change Parameter.');
		expect(liveParams(null, null, {}, ctx())).toEqual([]);
	});

	it('name a live value outside its range before it is sent', () => {
		const p = { min: 0, max: 1, unit: '' };
		expect(liveValueError(p, 0.5)).toBeNull();
		expect(liveValueError(p, 1.5)).toBe('From 0 to 1.');
		expect(liveValueError({ ...p, unit: 'dB/s' }, -1)).toBe('From 0 to 1 dB/s.');
		expect(liveValueError(p, NaN)).toBe('Enter a number.');
	});

	it("draw a Sine law's form from its manifest, and mark its live parameters by name", () => {
		expect(SERVO_BLEND.params.map((p) => lawParamDescriptor(p)?.type)).toEqual(['number', 'enum', 'number', 'integer', 'boolean']);
		expect(lawValues(SERVO_BLEND, { blend_w: 0.3 }, 1)).toEqual({ blend_w: 0.3, compression: 'slow', ki: 2, steps: 3, on: true });
		expect(liveParamLabels(SERVO_BLEND, SERVO_BLEND.live)).toEqual(['Prediction weight', 'Steps']);
		// only a number parameter can be live; another test type marks none
		expect(liveParamLabels(SERVO_BLEND, ['compression', 'on'])).toEqual([]);
		expect(liveParamLabels(SERVO_BLEND, undefined)).toBeNull();
		expect(liveParamLabels(null, [])).toEqual([]);
	});
});
