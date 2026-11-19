/**
 * Plain words for an SRS shock run: what the run is doing, the pulse coming and the one just
 * fired, the worst band of the last pulse, how a run ended and what tripped it. Display only:
 * every number is the service's, and nothing here computes one. Pulses are numbered from 1 on
 * screen (the first pulse fired is "Pulse 1"); a row's `index` counts from 0. The pulse count and
 * the time to the next pulse read as classical shock's.
 */

import type { SrsLawFaultReason, SrsOutcome, SrsPulseRow, SrsState, SrsTrip } from '$lib/protocol/srs';
import type { Status } from '$lib/protocol/types';
import { formatNumber } from '$lib/viz/format';
import { db, type LevelStatus } from '../words';
import { nextPulseWords, plannedPulses, pulseNumber, pulseWords, SHOCK_PHASE_WORDS } from '../shock/shockWords';

export { nextPulseWords, plannedPulses, pulseNumber, pulseWords };

export const SRS_OUTCOME_WORDS: Record<SrsOutcome, string> = {
	completed: 'Completed',
	operator_stop: 'Stopped by the Operator',
	operator_abort: 'Aborted by the Operator',
	safe_stop: 'Stopped: Nobody Held Control',
	aborted: 'Aborted by a Protection',
	not_converged: 'Stopped: The Spectrum Did Not Converge',
	held: 'Held at Identification',
	law_fault: 'Stopped: No Usable Gains From the Control Law',
	fault: 'Stopped on a Fault'
};

export function srsOutcomeTone(outcome: SrsOutcome): Status {
	if (outcome === 'completed') return 'pass';
	if (outcome === 'operator_stop') return 'info';
	return 'fail';
}

export const SRS_PHASE_WORDS: Record<SrsState['phase'], string> = SHOCK_PHASE_WORDS;

export const SRS_TRIP_WORDS: Record<SrsTrip['kind'], string> = {
	envelope: 'Envelope Abort',
	kinematic: 'Kinematic Abort',
	drive_vpk: 'Drive Abort',
	accel: 'Peak Acceleration Abort',
	monitor: 'Monitor Abort',
	not_converged: 'Iteration Limit',
	law_fault: 'Control Law Stop'
};

export const SRS_LAW_FAULT_WORDS: Record<SrsLawFaultReason, string> = {
	error: 'The control law reported an error',
	panicked: 'The control law failed while computing',
	over_budget: 'The control law took longer than its time budget',
	over_memory: 'The control law asked for more memory than its cap',
	wrong_length: 'The control law did not give one gain per component',
	non_finite: 'A gain was not a number',
	not_positive: 'A gain was zero or below',
	undeclared_value: 'The control law reported a value its manifest does not declare',
	too_many_values: 'The control law reported more values than allowed',
	gain_rise: "A component's gain rose more than the cap allows in one pulse"
};

const num = (v: number, digits = 3) => formatNumber(v, digits);
const dbAbs = (v: number) => `${formatNumber(Math.abs(v), 2)} dB`;
const hz = (v: number) => `${formatNumber(v, 4)} Hz`;

const AXIS: Record<'accel' | 'velocity' | 'displacement', { label: string; unit: string }> = {
	accel: { label: 'Acceleration', unit: 'g' },
	velocity: { label: 'Velocity', unit: 'in/s' },
	displacement: { label: 'Displacement', unit: 'in pk-pk' }
};

/** The trip's facts as labeled values. */
export function srsTripFacts(trip: SrsTrip): { label: string; value: string }[] {
	const at = [
		{ label: 'Pulse', value: trip.pulse > 0 ? String(trip.pulse) : 'Before the first pulse' },
		{ label: 'Level', value: trip.at_level_db === null ? '-' : db(trip.at_level_db) }
	];
	const predicted = (p: boolean) => ({ label: 'Found', value: p ? 'On the prediction, before the pulse fired' : 'On the measured pulse' });
	switch (trip.kind) {
		case 'envelope':
			return [
				...at,
				{ label: 'Bands outside the abort band', value: String(trip.abort_bands) },
				{ label: 'Worst band', value: hz(trip.worst_band_hz) },
				{ label: 'Its error', value: `${dbAbs(trip.worst_err_db)} ${trip.worst_err_db < 0 ? 'low' : 'high'}` }
			];
		case 'kinematic': {
			const a = AXIS[trip.axis];
			return [...at, { label: a.label, value: `${num(trip.value)} ${a.unit}` }, { label: 'Limit', value: `${num(trip.limit)} ${a.unit}` }, predicted(trip.predicted)];
		}
		case 'drive_vpk':
			return [...at, { label: 'Drive', value: `${num(trip.vpk)} V pk` }, { label: 'Drive abort', value: `${num(trip.abort_vpk)} V pk` }, predicted(trip.predicted)];
		case 'accel':
			return [...at, { label: 'Predicted peak', value: `${num(trip.predicted_g)} g` }, { label: 'Ceiling', value: `${num(trip.limit_g)} g` }];
		case 'monitor':
			return [...at, { label: 'Channel', value: String(trip.channel) }, { label: 'Value', value: num(trip.value) }, { label: 'Limit', value: num(trip.limit) }];
		case 'not_converged':
			return [...at, { label: 'Limit', value: trip.limit === 'per_level' ? 'Pulses at one level without a pass' : "The run's pulse cap" }, { label: 'Pulses', value: String(trip.pulses) }];
		case 'law_fault':
			return [...at, { label: 'Why', value: SRS_LAW_FAULT_WORDS[trip.reason] ?? trip.reason }, { label: 'In a row', value: String(trip.consecutive) }];
	}
}

/** The last pulse's worst band: "Worst band 630 Hz, 1.2 dB low", or null when it was not measured. */
export function worstBandWords(row: Pick<SrsPulseRow, 'worst_err_db' | 'worst_band_hz'> | null): string | null {
	if (!row || row.worst_err_db === null || row.worst_band_hz === null) return null;
	const e = row.worst_err_db;
	if (Math.abs(e) < 0.05) return `Worst band ${hz(row.worst_band_hz)}, on the target`;
	return `Worst band ${hz(row.worst_band_hz)}, ${dbAbs(e)} ${e < 0 ? 'low' : 'high'}`;
}

/**
 * The pulse just fired, in words: "Pulse 2 passed", or "Pulse 2: 3 bands outside the alarm band;
 * correcting" while the run goes on.
 */
export function lastPulseWords(row: SrsPulseRow | null, continuing: boolean): { text: string; tone: Status } | null {
	if (!row) return null;
	const n = pulseNumber(row.index);
	if (row.passed === true) return { text: `Pulse ${n} passed: every band within the alarm band`, tone: 'pass' };
	if (row.passed === null) return { text: `Pulse ${n} fired; not judged`, tone: 'info' };
	const corr = continuing ? '; correcting' : '';
	const k = row.alarm_bands ?? 0;
	if (k > 0) return { text: `Pulse ${n}: ${k} ${k === 1 ? 'band' : 'bands'} outside the alarm band${corr}`, tone: 'warn' };
	return { text: `Pulse ${n} did not pass${corr}`, tone: 'warn' };
}

/** The level card's status line for an SRS shock run. */
export function srsLevelStatus(s: SrsState | null): LevelStatus {
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
			const tone = srsOutcomeTone(outcome);
			return { mark: tone === 'pass' ? 'done' : 'ended', text: SRS_OUTCOME_WORDS[outcome], tone };
		}
		case 'running':
			break;
	}
	if (s?.phase === 'sysid') return { mark: 'identify', text: 'Identifying the system with a quiet burst', tone: 'info' };
	if (s?.phase === 'firing') return { mark: 'control', text: `Firing at ${db(s.level_db)}`, tone: 'pass' };
	if (s?.mode === 'manual') return { mark: 'control', text: `Waiting for Fire at ${db(s?.level_db)}`, tone: 'info' };
	return { mark: 'control', text: `Between pulses at ${db(s?.level_db)}`, tone: 'pass' };
}

/** The arm-time notch in a line: "2 bands notched; worst predicted 3.1 dB over, 0.6 dB under after". */
export function notchWords(n: SrsState['notch']): string | null {
	if (!n) return null;
	if (n.notched_bands === 0) return 'No band needed a notch: every prediction is under its bound.';
	const over = (v: number | null) => (v === null ? '-' : `${dbAbs(v)} ${v > 0 ? 'over' : 'under'}`);
	return `${n.notched_bands} ${n.notched_bands === 1 ? 'band' : 'bands'} notched; worst prediction ${over(n.worst_before_db)} its allowed level before, ${over(n.worst_after_db)} after${n.converged ? '' : '; the fit did not converge'}`;
}
