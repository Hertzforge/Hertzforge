/**
 * Plain words for a classical shock run: what the run is doing, the pulse coming and the one just
 * fired, how a run ended and what tripped it. Display only: every number is the service's, and
 * nothing here computes one. Pulses are numbered from 1 on screen (the first pulse fired is
 * "Pulse 1"); a row's `index` counts from 0.
 */

import type { ShockLawFaultReason, ShockOutcome, ShockPulseRow, ShockState, ShockTrip } from '$lib/protocol/shock';
import type { Status } from '$lib/protocol/types';
import { formatNumber } from '$lib/viz/format';
import { db, type LevelStatus } from '../words';

export const SHOCK_OUTCOME_WORDS: Record<ShockOutcome, string> = {
	completed: 'Completed',
	operator_stop: 'Stopped by the Operator',
	operator_abort: 'Aborted by the Operator',
	safe_stop: 'Stopped: Nobody Held Control',
	aborted: 'Aborted by a Protection',
	not_converged: 'Stopped: The Pulse Did Not Converge',
	held: 'Held at Identification',
	law_fault: 'Stopped: No Usable Drive From the Control Law',
	fault: 'Stopped on a Fault'
};

export function shockOutcomeTone(outcome: ShockOutcome): Status {
	if (outcome === 'completed') return 'pass';
	if (outcome === 'operator_stop') return 'info';
	return 'fail';
}

export const SHOCK_PHASE_WORDS: Record<ShockState['phase'], string> = {
	idle: 'Idle',
	feasibility_blocked: 'Exceeds the shaker ratings',
	armed: 'Station open, nothing driven',
	sysid: 'System identification',
	waiting: 'Between pulses',
	firing: 'Firing a pulse',
	awaiting_review: 'Identification done, waiting for you',
	stopping: 'Stopping after this pulse',
	stopped: 'Stopped',
	aborted: 'Aborted',
	held: 'Held at identification'
};

export const SHOCK_TRIP_WORDS: Record<ShockTrip['kind'], string> = {
	tolerance: 'Tolerance Abort',
	overshoot: 'Overshoot Abort',
	kinematic: 'Kinematic Abort',
	drive_vpk: 'Drive Abort',
	accel: 'Peak Acceleration Abort',
	monitor: 'Monitor Abort',
	divergence: 'Divergence Abort',
	not_converged: 'Iteration Limit',
	law_fault: 'Control Law Stop'
};

export const SHOCK_LAW_FAULT_WORDS: Record<ShockLawFaultReason, string> = {
	error: 'The control law reported an error',
	panicked: 'The control law failed while computing',
	over_budget: 'The control law took longer than its time budget',
	over_memory: 'The control law asked for more memory than its cap',
	wrong_length: 'The drive had the wrong number of samples',
	non_finite: 'The drive was not a number',
	undeclared_value: 'The control law reported a value its manifest does not declare',
	too_many_values: 'The control law reported more values than allowed',
	peak_rise: "The drive's peak rose more than the cap allows in one pulse",
	bin_rise: 'A frequency of the drive rose more than the cap allows in one pulse',
	first_drive: "The first drive's predicted peak was over its bound"
};

const num = (v: number, digits = 3) => formatNumber(v, digits);
const pct = (v: number) => `${formatNumber(Math.abs(v), 2)}%`;

/** A pulse's number on screen, from its row index. */
export const pulseNumber = (index: number) => index + 1;

/** The trip's facts as labeled values. */
export function shockTripFacts(trip: ShockTrip): { label: string; value: string }[] {
	const at = [
		{ label: 'Pulse', value: trip.pulse > 0 ? String(trip.pulse) : 'Before the first pulse' },
		{ label: 'Level', value: trip.at_level_db === null ? '-' : db(trip.at_level_db) }
	];
	const predicted = (p: boolean) => ({ label: 'Found', value: p ? 'On the prediction, before the pulse fired' : 'On the measured pulse' });
	switch (trip.kind) {
		case 'tolerance':
			return [
				...at,
				{ label: 'Amplitude envelope', value: trip.envelope_pass ? 'Pass' : 'Fail' },
				{ label: 'Velocity change', value: trip.velocity_change_pass ? 'Pass' : 'Fail' },
				{ label: 'Error', value: pct(trip.error_pct) },
				{ label: 'Worst deviation', value: `${formatNumber(trip.worst_deviation_frac * 100, 2)}% of peak` }
			];
		case 'overshoot':
			return [...at, { label: 'Peak', value: `${num(trip.peak_ratio)} times the commanded peak` }, { label: 'Limit', value: `${num(trip.limit_ratio)} times` }];
		case 'kinematic': {
			const unit = trip.axis === 'velocity' ? 'in/s' : 'in';
			return [...at, { label: trip.axis === 'velocity' ? 'Velocity' : 'Displacement', value: `${num(trip.value)} ${unit}` }, { label: 'Limit', value: `${num(trip.limit)} ${unit}` }, predicted(trip.predicted)];
		}
		case 'drive_vpk':
			return [...at, { label: 'Drive', value: `${num(trip.vpk)} V pk` }, { label: 'Drive abort', value: `${num(trip.abort_vpk)} V pk` }, predicted(trip.predicted)];
		case 'accel':
			return [...at, { label: 'Predicted peak', value: `${num(trip.predicted_g)} g` }, { label: 'Ceiling', value: `${num(trip.limit_g)} g` }];
		case 'monitor':
			return [...at, { label: 'Channel', value: String(trip.channel) }, { label: 'Value', value: num(trip.value) }, { label: 'Limit', value: num(trip.limit) }];
		case 'divergence':
			return [...at, { label: 'Pulses with a growing error', value: String(trip.pulses) }, { label: 'Error', value: pct(trip.error_pct) }];
		case 'not_converged':
			return [...at, { label: 'Limit', value: trip.limit === 'per_level' ? 'Pulses at one level without a pass' : "The run's pulse cap" }, { label: 'Pulses', value: String(trip.pulses) }];
		case 'law_fault':
			return [...at, { label: 'Why', value: SHOCK_LAW_FAULT_WORDS[trip.reason] ?? trip.reason }, { label: 'In a row', value: String(trip.consecutive) }];
	}
}

/** The pulses a schedule plans: the sum over its levels. */
export function plannedPulses(pulsesPerLevel: number[] | null | undefined, levels = 0): number {
	const p = pulsesPerLevel ?? [];
	if (p.length === 1 && levels > 1) return p[0] * levels;
	return p.reduce((n, x) => n + (Number.isFinite(x) ? x : 0), 0);
}

/**
 * The pulse coming, or playing: "Pulse 3 of 7 at -6 dB". In manual there is no plan to count
 * against; past the plan (a level repeated until it passes) the count says so.
 */
export function pulseWords(s: Pick<ShockState, 'state' | 'phase' | 'pulses' | 'level_db' | 'mode'>, planned: number): string | null {
	if (s.state !== 'running' && s.state !== 'stopping') return null;
	if (s.phase === 'sysid') return null;
	const n = s.pulses.fired + (s.phase === 'firing' ? 0 : 1);
	const at = s.level_db === null ? '' : ` at ${db(s.level_db)}`;
	const head = s.phase === 'firing' ? 'Firing pulse' : 'Pulse';
	if (s.mode === 'manual' || planned <= 0) return `${head} ${n}${at}`;
	if (n > planned) return `${head} ${n}${at}, ${n - planned} past the ${planned} planned`;
	return `${head} ${n} of ${planned}${at}`;
}

/** "Next pulse in 0.6 s", "Ready to fire", or null when no pulse is due. */
export function nextPulseWords(s: Pick<ShockState, 'state' | 'phase' | 'next_pulse_s' | 'mode'>): string | null {
	if (s.state !== 'running' || s.phase !== 'waiting') return null;
	if (s.next_pulse_s !== null && s.next_pulse_s > 0.05) return `Next pulse in ${s.next_pulse_s.toFixed(1)} s`;
	return s.mode === 'manual' ? 'Ready to fire' : null;
}

/** The pulse just fired, in words: "Pulse 2 was 25% low; correcting". */
export function lastPulseWords(row: ShockPulseRow | null, continuing: boolean): { text: string; tone: Status } | null {
	if (!row) return null;
	const n = pulseNumber(row.index);
	const corr = continuing && row.origin === 'ladder' ? '; correcting' : '';
	if (row.passed === true) {
		const err = row.error_pct === null ? '' : `, ${pct(row.error_pct)} error`;
		return { text: `Pulse ${n} passed${err}`, tone: 'pass' };
	}
	if (row.passed === null || row.peak_ratio === null) return { text: `Pulse ${n} fired; not judged`, tone: 'info' };
	const off = (row.peak_ratio - 1) * 100;
	if (Math.abs(off) >= 0.5 && row.verdicts && !row.verdicts.envelope) {
		return { text: `Pulse ${n} was ${formatNumber(Math.abs(off), 2)}% ${off < 0 ? 'low' : 'high'}${corr}`, tone: 'warn' };
	}
	if (row.verdicts && !row.verdicts.velocity) return { text: `Pulse ${n} missed the velocity change${corr}`, tone: 'warn' };
	return { text: `Pulse ${n} failed the tolerance${corr}`, tone: 'warn' };
}

/** The level card's status line for a classical shock run. */
export function shockLevelStatus(s: ShockState | null): LevelStatus {
	const state = s?.state ?? 'unconfigured';
	switch (state) {
		case 'unconfigured':
			return { mark: 'idle', text: 'Not set up', tone: 'info' };
		case 'configured':
			return { mark: 'idle', text: 'Set up, ready to arm', tone: 'info' };
		case 'blocked':
			return { mark: 'hold', text: "Blocked: the pulse exceeds the shaker's ratings", tone: 'warn' };
		case 'armed':
			return { mark: 'armed', text: 'Armed, nothing driven', tone: 'info' };
		case 'review':
			return { mark: 'hold', text: 'Holding for review', tone: 'info' };
		case 'stopping':
			return { mark: 'stop', text: 'Stopping: the pulse in flight finishes, nothing more fires', tone: 'info' };
		case 'finished': {
			const outcome = s?.end?.outcome;
			if (!outcome) return { mark: 'ended', text: 'Finished', tone: 'info' };
			const tone = shockOutcomeTone(outcome);
			return { mark: tone === 'pass' ? 'done' : 'ended', text: SHOCK_OUTCOME_WORDS[outcome], tone };
		}
		case 'running':
			break;
	}
	if (s?.phase === 'sysid') return { mark: 'identify', text: s.identification ? 'Identifying the system' : 'Identifying the system with band-limited random', tone: 'info' };
	if (s?.phase === 'firing') return { mark: 'control', text: `Firing at ${db(s.level_db)}`, tone: 'pass' };
	if (s?.mode === 'manual') return { mark: 'control', text: `Waiting for Fire at ${db(s?.level_db)}`, tone: 'info' };
	return { mark: 'control', text: `Between pulses at ${db(s?.level_db)}`, tone: 'pass' };
}

/** The words of a log entry's kind, for the run log (the Random words cover every kind). */
export function verdictWord(v: boolean | null | undefined): string {
	return v === true ? 'Pass' : v === false ? 'Fail' : '-';
}
