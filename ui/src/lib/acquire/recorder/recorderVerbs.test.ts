/**
 * What a Recorder run offers in each state, phase and trigger mode, and why a verb waits: the run
 * bar's verbs and the Recorder's own (Arm, Disarm, Record, Cut).
 */
import { describe, expect, it } from 'vitest';
import type { RecorderPhase, RecorderRunState, RecorderTriggerMode } from '$lib/protocol/recorder';
import { recorderBarActions, recorderPrimary, recorderVerbs, type RecorderVerbContext } from './recorderVerbs';

const ctx = (state: RecorderRunState | null, phase: RecorderPhase = 'idle', mode: RecorderTriggerMode = 'manual', more: Partial<RecorderVerbContext> = {}, left: number | null = null): RecorderVerbContext => ({
	connected: true,
	controller: true,
	run: state ? { state, phase, mode, left } : null,
	setup: 'applied',
	setupErrors: 0,
	busy: null,
	pending: null,
	...more
});

const shown = (o: Record<string, { visible: boolean }>) =>
	Object.entries(o)
		.filter(([, s]) => s.visible)
		.map(([k]) => k);
const enabled = (o: Record<string, { enabled: boolean }>) =>
	Object.entries(o)
		.filter(([, s]) => s.enabled)
		.map(([k]) => k);

describe('the Recorder verbs', () => {
	it("offers the Recorder's setup, start, stop and a new run on the bar, never the bar's arm, proceed, level or ladder", () => {
		expect(shown(recorderBarActions(ctx(null)))).toEqual(['apply']);
		expect(shown(recorderBarActions(ctx('configured')))).toEqual(['apply', 'start']);
		for (const [s, p] of [
			['starting', 'opening'],
			['running', 'monitoring'],
			['running', 'recording'],
			['stopping', 'closing']
		] as const)
			expect(shown(recorderBarActions(ctx(s, p)))).toEqual(['stop', 'abort']);
		expect(shown(recorderBarActions(ctx('finished', 'stopped')))).toEqual(['apply', 'reset']);
		expect(recorderBarActions(ctx('stopping', 'closing')).stop.reason).toBe('The run is already finishing its recordings.');
		expect([recorderPrimary(ctx(null)), recorderPrimary(ctx('configured')), recorderPrimary(ctx('running', 'recording')), recorderPrimary(ctx('finished', 'faulted'))]).toEqual(['apply', 'start', null, 'reset']);
		expect(recorderPrimary(ctx('configured', 'idle', 'manual', { setup: 'changed' }))).toBe('apply');
		expect(recorderBarActions(ctx('configured', 'idle', 'manual', { setup: 'changed' })).start.reason).toBe('The setup changed since it was applied. Apply it first.');
	});

	it('offers the Recorder verbs a manual trigger takes in each phase', () => {
		const monitoring = recorderVerbs(ctx('running', 'monitoring', 'manual'));
		expect(shown(monitoring)).toEqual(['arm', 'record', 'cut']);
		expect(enabled(monitoring)).toEqual(['arm', 'record']);
		expect(monitoring.cut.reason).toBe('No recording is being saved.');
		const armed = recorderVerbs(ctx('running', 'armed', 'manual'));
		expect(shown(armed)).toEqual(['disarm', 'record', 'cut']);
		expect(enabled(armed)).toEqual(['disarm', 'record']);
		const recording = recorderVerbs(ctx('running', 'recording', 'manual'));
		expect(shown(recording)).toEqual(['arm', 'record', 'cut']);
		expect(enabled(recording)).toEqual(['cut']);
		expect(recording.record.reason).toBe('A recording is being saved: cut it first.');
		expect(recording.arm.reason).toBe('Once this recording has ended.');
	});

	it('offers a Recorder level trigger Arm and Disarm only, and Disarm in place of Arm while its count re-arms it', () => {
		expect(shown(recorderVerbs(ctx('running', 'monitoring', 'level')))).toEqual(['arm']);
		expect(enabled(recorderVerbs(ctx('running', 'armed', 'level')))).toEqual(['disarm']);
		const more = recorderVerbs(ctx('running', 'recording', 'level', {}, 3));
		expect(shown(more)).toEqual(['disarm']);
		expect(more.disarm.reason).toBe('Once this recording has ended: the trigger then waits again.');
		expect(shown(recorderVerbs(ctx('running', 'recording', 'level', {}, null)))).toEqual(['disarm']);
		const last = recorderVerbs(ctx('running', 'recording', 'level', {}, 0));
		expect(shown(last)).toEqual(['arm']);
		expect(last.arm.reason).toBe('Once this recording has ended.');
	});

	it('offers a Recorder run from the start only Cut, while its recording runs', () => {
		const rec = recorderVerbs(ctx('running', 'recording', 'start'));
		expect(shown(rec)).toEqual(['cut']);
		expect(enabled(rec)).toEqual(['cut']);
		expect(recorderVerbs(ctx('running', 'monitoring', 'start')).cut.enabled).toBe(false);
	});

	it('shows no Recorder verb outside a live run, and says why each waits while the station opens or the run stops', () => {
		for (const s of ['configured', 'finished'] as const) expect(shown(recorderVerbs(ctx(s, 'idle', 'manual')))).toEqual([]);
		expect(shown(recorderVerbs(ctx(null)))).toEqual([]);
		expect(recorderVerbs(ctx('starting', 'opening', 'manual')).arm.reason).toBe('Once the station has opened.');
		const stopping = recorderVerbs(ctx('stopping', 'closing', 'manual'));
		for (const id of ['arm', 'record', 'cut'] as const) expect(stopping[id].reason).toBe('The run is stopping: its recordings are being finished.');
	});

	it('lets any Recorder screen stop and abort, and asks for control for every recording verb', () => {
		const o = ctx('running', 'monitoring', 'manual', { controller: false });
		const bar = recorderBarActions(o);
		expect([bar.stop.enabled, bar.abort.enabled]).toEqual([true, true]);
		const v = recorderVerbs(o);
		expect(v.arm).toEqual({ visible: true, enabled: false, reason: 'Take control to arm.' });
		expect(v.record.reason).toBe('Take control to record.');
		expect(recorderVerbs(ctx('running', 'recording', 'manual', { busy: 'Cut' })).cut.reason).toBe('Waiting for Cut.');
		expect(recorderVerbs(ctx('running', 'monitoring', 'manual', { connected: false })).arm.reason).toBe('The control service is not connected.');
	});
});
