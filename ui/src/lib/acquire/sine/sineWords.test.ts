/**
 * The words a Sine run reads in: the level card's status line in every state and test kind, the
 * sweep under way, the target against the measured amplitude, the schedule's progress, the
 * identification, and each trip's facts.
 */
import { describe, expect, it } from 'vitest';
import type { SineConfig, SineLiveHead, SineState, SineTrip } from '$lib/protocol/sine';
import { defaultSineSetup, toSineConfig } from './sineSetup';
import { errorWords, hz, identificationWords, limitingWords, progressOf, rateWords, sineLevelStatus, sineTripFacts, sweepWords, SINE_TRIP_WORDS } from './sineWords';

function state(over: Partial<SineState> = {}): SineState {
	return {
		run_id: 'sine-0001',
		state: 'running',
		phase: 'engaged',
		title: 'T',
		source: 'SIM',
		station: { kind: 'sim', class: 'sim', devices: [] },
		grid: null,
		clock: 'shared',
		test: 'sweep',
		mode: 'auto',
		levels_db: [-12, -6, 0],
		level_db: -6,
		rung: 1,
		frequency_hz: 251.3,
		direction: 'up',
		sweeps: { done: 1, of: 2 },
		held: false,
		elapsed_s: 30,
		at_level_s: 10,
		remaining_s: 90,
		alarm: false,
		notch: false,
		identification: null,
		law: null,
		feasibility: null,
		end: null,
		...over
	};
}

function live(over: Partial<SineLiveHead> = {}): SineLiveHead {
	return {
		run_id: 'sine-0001',
		source: 'SIM',
		class: 'sim',
		state: 'running',
		phase: 'engaged',
		test: 'sweep',
		mode: 'auto',
		t_s: 40,
		elapsed_s: 30,
		frequency_hz: 251.3,
		direction: 'up',
		sweep_rate_oct_per_min: 4,
		sweeps: { done: 1, of: 2 },
		held: false,
		level_db: -6,
		commanded_db: -6,
		target: 0.5,
		measured: 0.502,
		error_db: 0.035,
		drive_vpk: 0.12,
		cola_vpk: null,
		notch: false,
		alarm: false,
		channels: [],
		alarms: [],
		law: null,
		...over
	};
}

const config = (over: (d: ReturnType<typeof defaultSineSetup>) => void = () => {}): SineConfig => {
	const d = defaultSineSetup();
	d.limits.drive_vpk_ceiling = 0.5;
	d.limits.drive_vpk_abort = 0.8;
	over(d);
	return toSineConfig(d, null);
};

describe('the Sine words', () => {
	it('say what the run is doing before and after it drives', () => {
		const say = (s: Partial<SineState>) => sineLevelStatus(state(s), null).text;
		expect(sineLevelStatus(null, null).text).toBe('Not set up');
		expect(say({ state: 'configured', phase: 'idle' })).toBe('Set up, ready to arm');
		expect(say({ state: 'blocked', phase: 'feasibility_blocked' })).toBe("Blocked: the profile exceeds the shaker's ratings");
		expect(say({ state: 'armed', phase: 'armed' })).toBe('Armed, nothing driven');
		expect(say({ phase: 'sysid' })).toBe('Identifying the system with a noise burst');
		expect(say({ phase: 'coherence_gate' })).toBe('Checking the coherence');
		expect(say({ state: 'review', phase: 'awaiting_review' })).toBe('Holding for review');
		expect(say({ state: 'stopping', phase: 'ramping_down' })).toBe('Stopping, ramping down from -6.0 dB');
		const done = sineLevelStatus(state({ state: 'finished', phase: 'stopped', end: { outcome: 'completed', message: '', at_s: 1, level_db: 0, at_hz: 2000, ramp_s: 1, trip: null } }), null);
		expect([done.text, done.tone, done.mark]).toEqual(['Completed', 'pass', 'done']);
		const fault = sineLevelStatus(state({ state: 'finished', phase: 'stopped', end: { outcome: 'law_fault', message: '', at_s: 1, level_db: 0, at_hz: 300, ramp_s: 1, trip: null } }), null);
		expect([fault.text, fault.tone]).toEqual(['Stopped: No Usable Drive From the Control Law', 'fail']);
	});

	it('say the sweep, its direction and rate and the level, from the live frame or the setup', () => {
		expect(sineLevelStatus(state(), live()).text).toBe('Sweeping up at 4 oct/min at -6.0 dB');
		expect(sineLevelStatus(state({ direction: 'down' }), live({ direction: 'down', sweep_rate_oct_per_min: -4 })).text).toBe('Sweeping down at 4 oct/min at -6.0 dB');
		expect(sineLevelStatus(state(), null, config()).text).toBe('Sweeping up at 2 oct/min at -6.0 dB');
		const linear = config((d) => (d.test.sweep.law = 'linear'));
		expect(sineLevelStatus(state(), live({ sweep_rate_oct_per_min: undefined, sweep_rate_hz_per_s: 5 }), linear).text).toBe('Sweeping up at 5 Hz/s at -6.0 dB');
		expect(sineLevelStatus(state({ phase: 'full_level', level_db: 0 }), live({ phase: 'full_level', level_db: 0, commanded_db: 0 })).text).toBe('Sweeping up at 4 oct/min at full level, 0.0 dB');
		expect(sineLevelStatus(state(), live({ level_db: -9.5, commanded_db: -6 })).text).toBe('Ramping to -6.0 dB at 251.3 Hz');
	});

	it('say a dwell and a step test at their frequency, a hold and a limiting channel', () => {
		expect(sineLevelStatus(state({ test: 'dwell', direction: null, sweeps: null, frequency_hz: 250 }), live({ test: 'dwell', direction: null, sweep_rate_oct_per_min: 0, frequency_hz: 250 })).text).toBe('Dwelling at 250.0 Hz at -6.0 dB');
		expect(sineLevelStatus(state({ test: 'step', frequency_hz: 125 }), live({ test: 'step', frequency_hz: 125 })).text).toBe('Stepping, at 125.0 Hz at -6.0 dB');
		expect(sineLevelStatus(state({ held: true }), live({ held: true })).text).toBe('Holding at 251.3 Hz');
		const one = config((d) => d.limits.channels.push({ channel: 3, shared_name: '', profile: d.profile }));
		const lim = sineLevelStatus(state({ notch: true }), live({ notch: true }), one);
		expect([lim.text, lim.tone]).toEqual(['Limiting on channel 3', 'warn']);
		expect(limitingWords([3, 4])).toBe('Limiting on a limit channel (3, 4)');
		expect(limitingWords([])).toBe('Limiting on a limit channel');
	});

	it('count the sweep under way, of the count at this level', () => {
		expect(sweepWords(state())).toBe('Sweep 2 of 2, up');
		expect(sweepWords(state({ sweeps: { done: 0, of: null }, direction: 'down' }))).toBe('Sweep 1, down');
		expect(sweepWords(state({ sweeps: { done: 2, of: 2 } }))).toBe('Sweep 2 of 2, up');
		expect(sweepWords(state({ sweeps: null }))).toBeNull();
	});

	it('put the target beside the measured amplitude, and the schedule done beside what is left', () => {
		expect(errorWords(live())).toBe('0.502 g against 0.5 g, +0.04 dB');
		expect(errorWords(live({ measured: 0.45, error_db: -0.915 }), 'in/s')).toBe('0.45 in/s against 0.5 in/s, -0.92 dB');
		expect(errorWords(live({ measured: null }))).toBeNull();
		expect(progressOf(state())).toBe(0.25);
		expect(progressOf(state({ remaining_s: null }))).toBeNull();
		expect(hz(1234.56)).toBe('1234.6 Hz');
		expect(hz(null)).toBe('-');
		expect(rateWords(null, config((d) => (d.test.kind = 'dwell')).test)).toBeNull();
	});

	it("give the identification's facts and each trip's in words", () => {
		expect(identificationWords({ level_db: -20, band_coherence: 0.98, gate_coherence: 0.5, lag_frames: 1, trusted_frac: 0.97, passed: true })).toBe('Coherence 0.98 against 0.50; 97% of the band trusted; lag 1 sample');
		const at = { at_hz: 251.3, at_level_db: -6, at_tick: 40 };
		const trips: SineTrip[] = [
			{ ...at, kind: 'abort_band', side: 'high', measured_g: 0.72, limit_g: 0.7 },
			{ ...at, kind: 'kinematic', axis: 'displacement', value: 0.0098, limit: 0.008 },
			{ ...at, kind: 'limit_channel', channel: 3, measured: 2.3, limit: 2 },
			{ ...at, kind: 'drive_vpk', vpk: 0.26, abort_vpk: 0.25 },
			{ ...at, kind: 'servo_fault', consecutive_ticks: 50 },
			{ ...at, kind: 'watchdog', channel: 1, axis: 'g', value: 12, limit: 10, measured: 11, drive_vpk: 0.3 },
			{ ...at, kind: 'oscillator_range', requested_hz: 9000, fs_hz: 16384 },
			{ ...at, kind: 'divergence', capped_s: 1 },
			{ ...at, kind: 'law_fault', reason: 'over_budget', consecutive: 3 }
		];
		for (const t of trips) {
			const facts = sineTripFacts(t);
			expect(facts.slice(0, 2)).toEqual([
				{ label: 'At', value: '251.3 Hz' },
				{ label: 'Level', value: '-6.0 dB' }
			]);
			expect(facts.length).toBeGreaterThan(2);
			expect(SINE_TRIP_WORDS[t.kind]).toBeTruthy();
		}
		expect(sineTripFacts(trips[1])[2]).toEqual({ label: 'Displacement', value: '0.0098 in pk-pk' });
		expect(sineTripFacts(trips[8]).slice(2)).toEqual([
			{ label: 'Why', value: 'The control law took longer than its time budget' },
			{ label: 'In a row', value: '3' }
		]);
	});
});
