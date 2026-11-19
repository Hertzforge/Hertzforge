/**
 * The level card's words for each state and phase of an Explorer run, the trip's facts, the sweep,
 * the end, and the steering answer: the words the operator reads, from the service's numbers only.
 */
import { describe, expect, it } from 'vitest';
import type { ExplorerState } from '$lib/protocol/explorer';
import { answerWords, endWords, EXPLORER_OUTCOME_WORDS, EXPLORER_PHASE_WORDS, EXPLORER_STATE_WORDS, explorerLevelStatus, explorerOutcomeTone, explorerStateFact, explorerTripFacts, hz, motionWords, octaveStep, sweepWords, vpk } from './explorerWords';

type Card = Parameters<typeof explorerLevelStatus>[0];
const run = (more: Partial<NonNullable<Card>>): Card => ({ state: 'running', phase: 'steady', frequency_hz: 251.3, target_hz: 251.3, drive_vpk: 0.2, target_vpk: 0.2, sweep: null, band_hz: [20, 2000], stall: null, end: null, ...more });

describe('the Explorer words', () => {
	it("say what the Explorer tone is doing in the level card's own words", () => {
		const text = (more: Partial<NonNullable<Card>>) => explorerLevelStatus(run(more)).text;
		expect(text({ phase: 'quiet', drive_vpk: 0, target_vpk: 0 })).toBe('Quiet: the tone is at 0 V');
		expect(text({ phase: 'ramping', drive_vpk: 0.002 })).toBe('Rising to 0.200 V pk');
		expect(text({ phase: 'ramping', drive_vpk: 0.3, target_vpk: 0.05 })).toBe('Falling to 0.050 V pk');
		expect(text({ phase: 'ramping', drive_vpk: 0.1, target_vpk: 0 })).toBe('Falling to 0 V');
		expect(text({})).toBe('Holding 251.3 Hz at 0.200 V pk');
		expect(text({ phase: 'sweeping', sweep: { direction: 'up', rate_oct_per_min: 4, halted: false } })).toBe('Sweeping up at 4 oct/min');
		expect(text({ phase: 'at_edge', frequency_hz: 2000, sweep: { direction: 'up', rate_oct_per_min: 4, halted: true } })).toBe('Parked at the top of the band, 2000 Hz');
		expect(text({ phase: 'at_edge', frequency_hz: 20, sweep: { direction: 'down', rate_oct_per_min: 4, halted: true } })).toBe('Parked at the bottom of the band, 20 Hz');
		expect(text({ state: 'stopping', phase: 'ramping_down' })).toBe('Stopping: the drive is ramping down');
		expect(text({ state: 'starting', phase: 'priming' })).toBe('Starting: the station is opening, nothing driven');
		expect(text({ state: 'configured', phase: 'idle' })).toBe('Set up, ready to start');
		expect(explorerLevelStatus(null).text).toBe('Not set up');
		expect(text({ stall: { stale_s: 3.2, period_s: 0.04, message: 'stalled' } })).toBe("The service's loop has missed its periods for 3.2 s; Abort stays open");
	});

	it('name how an Explorer run ended, and read only the operator stop as a plain end', () => {
		const ended = (outcome: NonNullable<ExplorerState['end']>['outcome']) => explorerLevelStatus(run({ state: 'finished', phase: 'stopped', end: { outcome, message: '', at_s: 1, at_hz: 250, vpk: 0.2, ramp_s: null, trip: null } }));
		expect(ended('operator_stop')).toEqual({ mark: 'ended', text: 'Stopped by the Operator', tone: 'info' });
		expect(ended('limit_stop').text).toBe('Stopped: Motion Limit');
		for (const o of Object.keys(EXPLORER_OUTCOME_WORDS) as (keyof typeof EXPLORER_OUTCOME_WORDS)[]) expect(explorerOutcomeTone(o)).toBe(o === 'operator_stop' ? 'info' : 'fail');
		expect(endWords({ outcome: 'operator_stop', message: '', at_s: 41.2, at_hz: 250, vpk: 0.2, ramp_s: 6.667, trip: null })).toBe('The tone was at 250.0 Hz and 0.200 V pk; the drive ramped down in 6.667 s of station time.');
		expect(endWords({ outcome: 'arm_refused', message: '', at_s: null, at_hz: null, vpk: null, ramp_s: null, trip: null })).toBeNull();
	});

	it('give an Explorer trip its facts in their units', () => {
		expect(explorerTripFacts({ kind: 'watchdog', at_hz: 250, channel: 1, axis: 'accel', value: 0.1532, limit: 0.15 })).toEqual([
			{ label: 'At', value: '250.0 Hz' },
			{ label: 'Channel', value: '1' },
			{ label: 'Acceleration', value: '0.1532 g' },
			{ label: 'Limit', value: '0.15 g' }
		]);
		expect(explorerTripFacts({ kind: 'drive_vpk', at_hz: 100, vpk: 0.51, ceiling_vpk: 0.5 }).map((f) => f.value)).toEqual(['100.0 Hz', '0.510 V pk', '0.500 V pk']);
		expect(explorerTripFacts({ kind: 'limit', at_hz: 30, channel: 1, axis: 'displacement', value: 12.5, limit: 10 })[2]).toEqual({ label: 'Displacement', value: '12.5 mil' });
	});

	it("put the Explorer steering answer in words, naming a clamp and only the parts sent", () => {
		expect(answerWords({ hz: 2000, vpk: 0.2, clamped: { hz: true, vpk: false } }, { hz: true, vpk: false })).toBe('Answered 2000.0 Hz, clamped to the band');
		expect(answerWords({ hz: 250, vpk: 0.5, clamped: { hz: false, vpk: true } }, { hz: false, vpk: true })).toBe('Answered 0.500 V pk, clamped to the ceiling');
		expect(answerWords({ hz: 515, vpk: 0.1, clamped: { hz: false, vpk: false } }, { hz: true, vpk: true })).toBe('Answered 515.0 Hz; 0.100 V pk');
		expect(answerWords({ direction: 'up', rate_oct_per_min: 60, clamped: true }, { hz: true, vpk: false })).toBe('Sweeping up at 60 oct/min, clamped to the fastest sweep');
	});

	it('format the Explorer numbers in their units, and a sweep as it stands', () => {
		expect([hz(251.34), hz(null), vpk(0.2), vpk(undefined), octaveStep(1 / 96), octaveStep(1 / 6), octaveStep(0.25)]).toEqual(['251.3 Hz', '-', '0.200 V pk', '-', '1/96 oct', '1/6 oct', '1/4 oct']);
		expect(sweepWords(null)).toBeNull();
		expect(sweepWords({ direction: 'down', rate_oct_per_min: 30, halted: false })).toBe('Down at 30 oct/min');
		expect(sweepWords({ direction: 'up', rate_oct_per_min: 30, halted: true }, 2000, [20, 2000])).toBe('Parked at the top of the band');
		expect(sweepWords({ direction: 'up', rate_oct_per_min: 30, halted: true }, 500, [20, 2000])).toBe('Halted by hand');
		expect(motionWords({ index: 1, velocity_ips: 0.04916, displacement_mil: 0.0313, settling: true })).toBe('0.0492 in/s, 0.0313 mil (settling)');
		expect(motionWords({ index: 1, velocity_ips: null, displacement_mil: null, settling: false })).toBeNull();
	});

	it("name an Explorer refusal's state and phase in Explorer's own words", () => {
		expect(explorerStateFact({ state: 'stopping', phase: 'ramping_down' })).toBe('State: Stopping, Ramping down');
		expect(explorerStateFact({ state: 'starting', phase: 'priming' })).toBe(`State: Starting, ${EXPLORER_PHASE_WORDS.priming}`);
		expect(explorerStateFact(undefined)).toBeNull();
		expect(Object.keys(EXPLORER_STATE_WORDS)).toEqual(['unconfigured', 'configured', 'starting', 'running', 'stopping', 'finished']);
	});
});
