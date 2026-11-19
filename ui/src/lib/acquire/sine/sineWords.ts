/**
 * Plain words for a Sine run: what the run is doing ("Sweeping up at 4 oct/min at -6.0 dB",
 * "Holding at 251.3 Hz", "Limiting on channel 3"), which sweep it is on, the target against the
 * measured amplitude, how a run ended and what tripped it. Display only: every number is the
 * service's, and nothing here computes one beyond placing two of them side by side.
 */

import type { SineConfig, SineLawFaultReason, SineLiveHead, SineOutcome, SinePhase, SineState, SineTest, SineTrip } from '$lib/protocol/sine';
import type { Status } from '$lib/protocol/types';
import { formatNumber } from '$lib/viz/format';
import { db, type LevelStatus } from '../words';

export const SINE_OUTCOME_WORDS: Record<SineOutcome, string> = {
	completed: 'Completed',
	operator_stop: 'Stopped by the Operator',
	operator_abort: 'Aborted by the Operator',
	safe_stop: 'Stopped: Nobody Held Control',
	aborted: 'Aborted by a Protection',
	held: 'Held at the Coherence Check',
	law_fault: 'Stopped: No Usable Drive From the Control Law',
	fault: 'Stopped on a Fault'
};

export function sineOutcomeTone(outcome: SineOutcome): Status {
	if (outcome === 'completed') return 'pass';
	if (outcome === 'operator_stop') return 'info';
	return 'fail';
}

export const SINE_PHASE_WORDS: Record<SinePhase, string> = {
	idle: 'Idle',
	feasibility_blocked: 'Exceeds the shaker ratings',
	armed: 'Station open, nothing driven',
	sysid: 'System identification',
	coherence_gate: 'Coherence check',
	engaged: 'Level schedule',
	full_level: 'Full level',
	awaiting_review: 'Identification done, waiting for you',
	ramping_down: 'Ramping down',
	stopped: 'Stopped',
	aborted: 'Aborted',
	coherence_hold: 'Held at the coherence check'
};

export const SINE_TEST_WORDS: Record<SineTest['kind'], string> = {
	sweep: 'Sweep',
	dwell: 'Dwell',
	step: 'Step',
	rstd: 'Resonance search, track and dwell'
};

export const SINE_TRIP_WORDS: Record<SineTrip['kind'], string> = {
	abort_band: 'Abort Band',
	kinematic: 'Kinematic Abort',
	limit_channel: 'Limit Channel Abort',
	drive_vpk: 'Drive Abort',
	servo_fault: 'Control Point Lost',
	watchdog: 'Input Watchdog',
	oscillator_range: 'Frequency Out of Range',
	divergence: 'Divergence Abort',
	law_fault: 'Control Law Stop'
};

export const SINE_LAW_FAULT_WORDS: Record<SineLawFaultReason, string> = {
	error: 'The control law reported an error, or could not be built',
	panicked: 'The control law failed while computing',
	over_budget: 'The control law took longer than its time budget',
	over_memory: 'The control law asked for more memory than its cap',
	non_finite: 'The drive or a value was not a number',
	negative: 'The drive was below zero',
	undeclared_value: 'The control law reported a value its manifest does not declare',
	too_many_values: 'The control law reported more values than allowed'
};

/** A frequency to a tenth of a hertz: "251.3 Hz". */
export function hz(v: number | null | undefined): string {
	return v === null || v === undefined || !Number.isFinite(v) ? '-' : `${v.toFixed(1)} Hz`;
}

const num = (v: number, sig = 4) => formatNumber(v, sig);

/** The trip's facts as labeled values. */
export function sineTripFacts(trip: SineTrip): { label: string; value: string }[] {
	const at = [
		{ label: 'At', value: hz(trip.at_hz) },
		{ label: 'Level', value: db(trip.at_level_db) }
	];
	switch (trip.kind) {
		case 'abort_band':
			return [...at, { label: 'Measured', value: `${num(trip.measured_g)} g` }, { label: trip.side === 'high' ? 'Upper abort' : 'Lower abort', value: `${num(trip.limit_g)} g` }];
		case 'kinematic': {
			const unit = trip.axis === 'velocity' ? 'in/s' : 'in pk-pk';
			return [...at, { label: trip.axis === 'velocity' ? 'Velocity' : 'Displacement', value: `${num(trip.value, 3)} ${unit}` }, { label: 'Limit', value: `${num(trip.limit, 3)} ${unit}` }];
		}
		case 'limit_channel':
			return [...at, { label: 'Channel', value: String(trip.channel) }, { label: 'Measured', value: num(trip.measured) }, { label: 'Limit', value: num(trip.limit) }];
		case 'drive_vpk':
			return [...at, { label: 'Drive', value: `${num(trip.vpk)} V pk` }, { label: 'Drive abort', value: `${num(trip.abort_vpk)} V pk` }];
		case 'servo_fault':
			return [...at, { label: 'Ticks in a row', value: String(trip.consecutive_ticks) }];
		case 'watchdog':
			return [...at, { label: 'Channel', value: String(trip.channel) }, { label: 'Read', value: `${num(trip.value)} ${trip.axis}`.trim() }, { label: 'Limit', value: `${num(trip.limit)} ${trip.axis}`.trim() }, { label: 'Drive', value: `${num(trip.drive_vpk)} V pk` }];
		case 'oscillator_range':
			return [...at, { label: 'Asked for', value: hz(trip.requested_hz) }, { label: 'Rate', value: `${num(trip.fs_hz, 6)} Hz` }];
		case 'divergence':
			return [...at, { label: 'Capped for', value: `${num(trip.capped_s, 3)} s` }];
		case 'law_fault':
			return [...at, { label: 'Why', value: SINE_LAW_FAULT_WORDS[trip.reason] ?? trip.reason }, { label: 'In a row', value: String(trip.consecutive) }];
	}
}

/** The sweep's rate now, in words: the live frame's, else the setup's. Null outside a sweep. */
export function rateWords(live: Pick<SineLiveHead, 'sweep_rate_oct_per_min' | 'sweep_rate_hz_per_s'> | null, test: SineTest | null | undefined): string | null {
	const oct = live?.sweep_rate_oct_per_min;
	if (typeof oct === 'number' && oct !== 0) return `${num(Math.abs(oct), 3)} oct/min`;
	const lin = live?.sweep_rate_hz_per_s;
	if (typeof lin === 'number' && lin !== 0) return `${num(Math.abs(lin), 3)} Hz/s`;
	if (test?.kind !== 'sweep') return null;
	return test.rate.law === 'log' ? `${num(test.rate.oct_per_min, 3)} oct/min` : `${num(test.rate.hz_per_s, 3)} Hz/s`;
}

/** The limit channels the run has, by table index. */
export const limitChannelsOf = (c: SineConfig | null | undefined): number[] => (c?.limits.channels ?? []).map((ch) => ch.channel);

/** "Limiting on channel 3", or on any of several. */
export function limitingWords(channels: number[]): string {
	if (channels.length === 1) return `Limiting on channel ${channels[0]}`;
	if (channels.length > 1) return `Limiting on a limit channel (${channels.join(', ')})`;
	return 'Limiting on a limit channel';
}

/** The level card's status line for a Sine run. */
export function sineLevelStatus(s: SineState | null, live: SineLiveHead | null, config?: SineConfig | null): LevelStatus {
	const state = s?.state ?? 'unconfigured';
	const f = live && s && live.run_id === s.run_id ? live : null;
	switch (state) {
		case 'unconfigured':
			return { mark: 'idle', text: 'Not set up', tone: 'info' };
		case 'configured':
			return { mark: 'idle', text: 'Set up, ready to arm', tone: 'info' };
		case 'blocked':
			return { mark: 'hold', text: "Blocked: the profile exceeds the shaker's ratings", tone: 'warn' };
		case 'armed':
			return { mark: 'armed', text: 'Armed, nothing driven', tone: 'info' };
		case 'review':
			return { mark: 'hold', text: 'Holding for review', tone: 'info' };
		case 'stopping': {
			const from = f?.level_db ?? s?.level_db ?? null;
			return { mark: 'stop', text: from !== null ? `Stopping, ramping down from ${db(from)}` : 'Stopping, ramping down', tone: 'info' };
		}
		case 'finished': {
			const outcome = s?.end?.outcome;
			if (!outcome) return { mark: 'ended', text: 'Finished', tone: 'info' };
			const tone = sineOutcomeTone(outcome);
			return { mark: tone === 'pass' ? 'done' : 'ended', text: SINE_OUTCOME_WORDS[outcome], tone };
		}
		case 'running':
			break;
	}
	// The state and the frame have been laid over each other by recency; the frame adds the level
	// the drive is at now and the sweep's rate, which the state does not carry.
	const phase = s?.phase ?? f?.phase;
	if (phase === 'sysid') return { mark: 'identify', text: 'Identifying the system with a noise burst', tone: 'info' };
	if (phase === 'coherence_gate') return { mark: 'identify', text: 'Checking the coherence', tone: 'info' };
	const freq = s?.frequency_hz ?? f?.frequency_hz ?? null;
	if (s?.held ?? f?.held) return { mark: 'hold', text: `Holding at ${hz(freq)}`, tone: 'info' };
	if (s?.notch ?? f?.notch) return { mark: 'control', text: limitingWords(limitChannelsOf(config)), tone: 'warn' };
	const level = f?.level_db ?? s?.level_db ?? null;
	const commanded = s?.level_db ?? f?.commanded_db ?? null;
	if (f && level !== null && commanded !== null && Math.abs(level - commanded) > 0.05) return { mark: 'ramp', text: `Ramping to ${db(commanded)} at ${hz(freq)}`, tone: 'info' };
	const atLevel = phase === 'full_level' ? `at full level, ${db(level)}` : `at ${db(level)}`;
	const test = s?.test ?? f?.test ?? null;
	if (test === 'sweep') {
		const dir = s?.direction ?? f?.direction;
		const rate = rateWords(f, config?.test);
		return { mark: 'control', text: `Sweeping ${dir === 'down' ? 'down' : 'up'}${rate ? ` at ${rate}` : ''} ${atLevel}`, tone: 'pass' };
	}
	if (test === 'dwell') return { mark: 'control', text: `Dwelling at ${hz(freq)} ${atLevel}`, tone: 'pass' };
	if (test === 'step') return { mark: 'control', text: `Stepping, at ${hz(freq)} ${atLevel}`, tone: 'pass' };
	return { mark: 'control', text: `Controlling at ${hz(freq)} ${atLevel}`, tone: 'pass' };
}

/** "Sweep 2 of 4, up": the one-way sweep under way at this level; null outside a sweep. */
export function sweepWords(s: Pick<SineState, 'sweeps' | 'direction' | 'state'> | null): string | null {
	const sw = s?.sweeps;
	if (!sw) return null;
	const n = s?.state === 'finished' || (sw.of !== null && sw.done >= sw.of) ? Math.max(1, sw.done) : sw.done + 1;
	const of = sw.of !== null ? ` of ${sw.of}` : '';
	const dir = s?.direction ? `, ${s.direction}` : '';
	return `Sweep ${n}${of}${dir}`;
}

/** The target against the measured amplitude: "0.502 g against 0.500 g, +0.03 dB". */
export function errorWords(live: Pick<SineLiveHead, 'measured' | 'target' | 'error_db'> | null, unit = 'g'): string | null {
	if (!live || live.measured === null || live.target === null) return null;
	const e = live.error_db;
	return `${num(live.measured)} ${unit} against ${num(live.target)} ${unit}${e === null || e === undefined ? '' : `, ${e >= 0 ? '+' : '-'}${Math.abs(e).toFixed(2)} dB`}`;
}

/** The share of the schedule's test time done, from the service's two numbers; null when not known. */
export function progressOf(s: Pick<SineState, 'elapsed_s' | 'remaining_s'> | null): number | null {
	if (!s || s.elapsed_s === null || s.remaining_s === null) return null;
	const total = s.elapsed_s + s.remaining_s;
	return total > 0 ? Math.min(1, Math.max(0, s.elapsed_s / total)) : null;
}

/** The identification's facts in a line: "Coherence 0.98 against 0.50; 97% of the band trusted; lag 0 samples". */
export function identificationWords(id: SineState['identification']): string | null {
	if (!id) return null;
	return `Coherence ${id.band_coherence.toFixed(2)} against ${id.gate_coherence.toFixed(2)}; ${Math.round(id.trusted_frac * 100)}% of the band trusted; lag ${id.lag_frames} ${id.lag_frames === 1 ? 'sample' : 'samples'}`;
}
