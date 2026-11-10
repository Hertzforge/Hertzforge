/**
 * What an Explorer run offers in each state and phase, and why a verb waits: the run bar's verbs,
 * the steering verbs, and the steering keys, one step a press and only the panel's own keys.
 */
import { describe, expect, it } from 'vitest';
import type { ExplorerPhase, ExplorerRunState } from '$lib/protocol/explorer';
import { explorerBarActions, explorerPrimary, explorerSteerVerbs, steerForKey, type ExplorerVerbContext } from './explorerVerbs';

const ctx = (state: ExplorerRunState | null, phase: ExplorerPhase = 'idle', more: Partial<ExplorerVerbContext> = {}, run: Partial<{ target_vpk: number | null; sweep: { direction: 'up' | 'down'; rate_oct_per_min: number; halted: boolean } | null }> = {}): ExplorerVerbContext => ({
	connected: true,
	controller: true,
	run: state ? { state, phase, target_vpk: run.target_vpk === undefined ? 0.2 : run.target_vpk, sweep: run.sweep ?? null } : null,
	setup: 'applied',
	setupErrors: 0,
	busy: null,
	pending: null,
	...more
});

const STEPS = { fine_per_oct: 96, coarse_per_oct: 6, fine_db: 0.5, coarse_db: 3 };

describe('the Explorer verbs', () => {
	it('offers Explorer setup and start, never arm, proceed, a level step or a ladder mode', () => {
		const shown = (c: ExplorerVerbContext) =>
			Object.entries(explorerBarActions(c))
				.filter(([, s]) => s.visible)
				.map(([k]) => k);
		expect(shown(ctx(null))).toEqual(['apply']);
		expect(shown(ctx('configured'))).toEqual(['apply', 'start']);
		expect(shown(ctx('starting', 'priming'))).toEqual(['stop', 'abort']);
		expect(shown(ctx('running', 'quiet'))).toEqual(['stop', 'abort']);
		expect(shown(ctx('stopping', 'ramping_down'))).toEqual(['stop', 'abort']);
		expect(shown(ctx('finished', 'stopped'))).toEqual(['apply', 'reset']);
		expect(explorerBarActions(ctx('stopping', 'ramping_down')).stop.reason).toBe('The drive is already ramping down.');
		expect([explorerPrimary(ctx(null)), explorerPrimary(ctx('configured')), explorerPrimary(ctx('running', 'steady')), explorerPrimary(ctx('finished', 'aborted'))]).toEqual(['apply', 'start', null, 'reset']);
		expect(explorerPrimary(ctx('configured', 'idle', { setup: 'changed' }))).toBe('apply');
	});

	it('lets any Explorer screen stop and abort, and asks for control for everything else', () => {
		const o = ctx('running', 'steady', { controller: false });
		const bar = explorerBarActions(o);
		expect([bar.stop.enabled, bar.abort.enabled]).toEqual([true, true]);
		const steer = explorerSteerVerbs(o);
		expect(steer.frequency).toEqual({ visible: true, enabled: false, reason: 'Take control to steer the tone.' });
		expect(steer.clearSurvey.reason).toBe('Take control to clear the survey.');
	});

	it('steers an Explorer tone only while it runs; once the stop begins nothing raises or moves the drive', () => {
		const running = explorerSteerVerbs(ctx('running', 'steady'));
		expect(Object.values(running).filter((s) => s.enabled)).toHaveLength(5);
		expect(running.hold.reason).toBe('No sweep is moving the tone.');
		const stopping = explorerSteerVerbs(ctx('stopping', 'ramping_down'));
		for (const id of ['frequency', 'levelVolts', 'levelStep', 'sweep', 'hold'] as const) expect(stopping[id].reason).toBe('The run is stopping: nothing raises or moves the drive now.');
		expect(stopping.clearSurvey.enabled).toBe(true);
		const starting = explorerSteerVerbs(ctx('starting', 'priming'));
		expect(starting.frequency.reason).toMatch(/station has opened/);
		expect(explorerSteerVerbs(ctx('finished', 'stopped')).frequency.visible).toBe(false);
		expect(explorerSteerVerbs(ctx('configured')).frequency.visible).toBe(false);
	});

	it('waits an Explorer dB step until there is a level to step from, and offers hold while a sweep moves', () => {
		const quiet = explorerSteerVerbs(ctx('running', 'quiet', {}, { target_vpk: 0 }));
		expect(quiet.levelStep.reason).toBe('The level is 0 V: set a level in volts first, then step it in dB.');
		expect(quiet.levelVolts.enabled).toBe(true);
		const sweeping = explorerSteerVerbs(ctx('running', 'sweeping', {}, { sweep: { direction: 'up', rate_oct_per_min: 4, halted: false } }));
		expect(sweeping.hold.enabled).toBe(true);
		const parked = explorerSteerVerbs(ctx('running', 'at_edge', {}, { sweep: { direction: 'up', rate_oct_per_min: 4, halted: true } }));
		expect(parked.hold.enabled).toBe(false);
		expect(explorerSteerVerbs(ctx('running', 'steady', { busy: 'Steer' })).frequency.reason).toBe('Waiting for Steer.');
	});

	it('maps an Explorer steering key to one step a press, the coarse step with Shift, and ignores a held key and the shell keys', () => {
		expect(steerForKey({ key: 'ArrowRight' }, STEPS)).toEqual({ step_oct: 1 / 96 });
		expect(steerForKey({ key: 'ArrowLeft', shiftKey: true }, STEPS)).toEqual({ step_oct: -1 / 6 });
		expect(steerForKey({ key: 'ArrowUp' }, STEPS)).toEqual({ step_db: 0.5 });
		expect(steerForKey({ key: 'ArrowDown', shiftKey: true }, STEPS)).toEqual({ step_db: -3 });
		expect(steerForKey({ key: 'h' }, STEPS)).toEqual({ hold: true });
		expect(steerForKey({ key: 'ArrowRight', repeat: true }, STEPS)).toBeNull();
		for (const mod of ['ctrlKey', 'altKey', 'metaKey'] as const) expect(steerForKey({ key: 'ArrowUp', [mod]: true }, STEPS)).toBeNull();
		for (const key of ['PageUp', 'Enter', ' ', 'a', '+']) expect(steerForKey({ key }, STEPS)).toBeNull();
		// no key moves further than one coarse step
		const all = ['ArrowRight', 'ArrowLeft', 'ArrowUp', 'ArrowDown'].flatMap((key) => [steerForKey({ key }, STEPS), steerForKey({ key, shiftKey: true }, STEPS)]);
		for (const a of all) {
			if (a && 'step_oct' in a) expect(Math.abs(a.step_oct)).toBeLessThanOrEqual(1 / 6);
			if (a && 'step_db' in a) expect(Math.abs(a.step_db)).toBeLessThanOrEqual(3);
		}
	});
});
