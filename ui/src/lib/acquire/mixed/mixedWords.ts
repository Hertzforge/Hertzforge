/**
 * Plain words for a Mixed Mode run: what the run is doing ("Floor alone at -12.0 dB", "Every part
 * at full level, 0.0 dB"), what each part is doing ("Sweeping up at 1 oct/min, sweep 2 of 4"), a
 * part's measured value against its target, how a run ended and on which part it tripped. Display
 * only: every number is the service's, and nothing here computes one beyond placing two of them side
 * by side.
 */

import type { MixedDerived, MixedLawFaultReason, MixedLiveHead, MixedOutcome, MixedPartKind, MixedPartRow, MixedPhase, MixedState, MixedTrip } from '$lib/protocol/mixed';
import type { Status } from '$lib/protocol/types';
import { formatNumber } from '$lib/viz/format';
import { db, LAW_FAULT_WORDS, type LevelStatus } from '../words';
import { hz, SINE_LAW_FAULT_WORDS } from '../sine/sineWords';

export { hz };

export const MIXED_OUTCOME_WORDS: Record<MixedOutcome, string> = {
	completed: 'Completed',
	operator_stop: 'Stopped by the Operator',
	operator_abort: 'Aborted by the Operator',
	safe_stop: 'Stopped: Nobody Held Control',
	aborted: 'Aborted by a Protection',
	held: 'Held at the Coherence Check',
	sysid_level_unreachable: 'Identification Could Not Reach Its Level',
	law_fault: 'Stopped: No Usable Drive From a Control Law',
	fault: 'Stopped on a Fault'
};

export function mixedOutcomeTone(outcome: MixedOutcome): Status {
	if (outcome === 'completed') return 'pass';
	if (outcome === 'operator_stop') return 'info';
	return 'fail';
}

export const MIXED_PHASE_WORDS: Record<MixedPhase, string> = {
	idle: 'Idle',
	feasibility_blocked: 'Exceeds the shaker ratings',
	armed: 'Station open, nothing driven',
	sysid: 'System identification',
	coherence_gate: 'Coherence check',
	floor_only: 'Floor alone, equalizing',
	engaged: 'Level schedule, every part',
	full_level: 'Full level',
	awaiting_review: 'Identification done, waiting for you',
	ramping_down: 'Ramping down',
	stopped: 'Stopped',
	aborted: 'Aborted',
	coherence_hold: 'Held at the coherence check'
};

export const PART_KIND_WORDS: Record<MixedPartKind, string> = { floor: 'Floor', tone: 'Tone', narrowband: 'Narrowband' };

export const MIXED_TRIP_WORDS: Record<MixedTrip['kind'], string> = {
	envelope: 'Floor Envelope Abort',
	abort_band: 'Tone Abort',
	band_abort: 'Narrowband Abort',
	servo_fault: 'Control Point Lost',
	divergence: 'Divergence Abort',
	law_fault: 'Control Law Stop',
	oscillator_range: 'Frequency Out of Range',
	kinematic: 'Kinematic Abort',
	drive_vpk: 'Drive Abort',
	drive_vrms: 'Random Drive Abort',
	watchdog: 'Input Watchdog'
};

/** Why `mixed.part` was refused, by `detail.why`. */
export const PART_REFUSED_WORDS: Record<string, string> = {
	unknown_part: 'The run has no part by that id.',
	floor: 'The floor plays for the whole run; it cannot be switched off.',
	window_closed: "The part's window has closed.",
	schedule_ended: "The part's schedule has ended."
};

const num = (v: number, sig = 4) => formatNumber(v, sig);

/** A part's value in its unit: "1.21 g", "0.0991 g²/Hz". */
export function partValue(v: number | null | undefined, unit: string): string {
	return v === null || v === undefined || !Number.isFinite(v) ? '-' : `${num(v, 3)} ${unit}`;
}

/** A signed dB error: "+0.15 dB". */
export function errDb(v: number | null | undefined): string {
	return v === null || v === undefined || !Number.isFinite(v) ? '-' : `${v >= 0 ? '+' : '-'}${Math.abs(v).toFixed(2)} dB`;
}

/** The rate now, in words, from the row's signed rate: "1 oct/min", "2 Hz/s". */
export function partRate(row: Pick<MixedPartRow, 'rate_oct_per_min' | 'rate_hz_per_s'>): string | null {
	if (typeof row.rate_oct_per_min === 'number') return `${num(Math.abs(row.rate_oct_per_min), 3)} oct/min`;
	if (typeof row.rate_hz_per_s === 'number') return `${num(Math.abs(row.rate_hz_per_s), 3)} Hz/s`;
	return null;
}

/** What a part is doing now, in words: "Sweeping up at 1 oct/min, sweep 2 of 4", "Standing", "Off". */
export function partMotion(row: MixedPartRow, held: boolean): string {
	if (row.kind === 'floor') return row.settled ? 'Equalized' : 'Equalizing';
	if (!row.on) return row.enabled ? 'Waiting for its window' : 'Off';
	if (row.scale < 0.999) return row.enabled ? 'Ramping in' : 'Ramping out';
	if (held) return 'Held';
	const rate = partRate(row);
	const sw = row.sweeps;
	const count = sw ? `, sweep ${sw.done + 1}${sw.of !== null ? ` of ${sw.of}` : ''}` : '';
	if (row.direction) return `Sweeping ${row.direction}${rate ? ` at ${rate}` : ''}${count}`;
	return 'Standing';
}

/** The trip's facts as labeled values, the part named by its name. */
export function mixedTripFacts(trip: MixedTrip, partName: (id: string) => string): { label: string; value: string }[] {
	const at: { label: string; value: string }[] = [];
	if (trip.part) at.push({ label: 'Part', value: partName(trip.part) });
	if (trip.at_hz !== null) at.push({ label: 'At', value: hz(trip.at_hz) });
	at.push({ label: 'Level', value: db(trip.at_level_db) });
	switch (trip.kind) {
		case 'envelope':
			return [...at, { label: 'Lines out', value: String(trip.abort_lines) }, { label: 'Worst', value: `${num(trip.worst_db, 3)} dB` }];
		case 'abort_band':
			return [...at, { label: 'Measured', value: `${num(trip.measured_g)} g` }, { label: trip.side === 'high' ? 'Upper abort' : 'Lower abort', value: `${num(trip.limit_g)} g` }];
		case 'band_abort':
			return [...at, { label: 'Measured', value: `${num(trip.measured_g2_hz)} g²/Hz` }, { label: trip.side === 'high' ? 'Upper abort' : 'Lower abort', value: `${num(trip.limit_g2_hz)} g²/Hz` }, { label: 'Degrees of freedom', value: String(trip.dof) }];
		case 'servo_fault':
			return [...at, { label: 'Ticks in a row', value: String(trip.consecutive_ticks) }];
		case 'divergence':
			return trip.capped_s !== undefined ? [...at, { label: 'Capped for', value: `${num(trip.capped_s, 3)} s` }] : [...at, { label: 'Loops in a row', value: String(trip.consecutive_loops ?? '-') }, { label: 'Lines diverging', value: String(trip.divergent_bins ?? '-') }];
		case 'law_fault': {
			const words = trip.role === 'floor' ? (LAW_FAULT_WORDS as Record<string, string>)[trip.reason] : (SINE_LAW_FAULT_WORDS as Record<string, string>)[trip.reason];
			return [...at, { label: 'Law', value: trip.role === 'floor' ? 'The floor law' : 'The tone law' }, { label: 'Why', value: words ?? (trip.reason as MixedLawFaultReason) }, { label: 'In a row', value: String(trip.consecutive) }];
		}
		case 'oscillator_range':
			return [...at, { label: 'Asked for', value: hz(trip.requested_hz) }, { label: 'Rate', value: `${num(trip.fs_hz, 6)} Hz` }];
		case 'kinematic': {
			const unit = trip.axis === 'velocity' ? 'in/s' : trip.axis === 'displacement' ? 'in pk-pk' : 'g';
			return [...at, { label: 'Channel', value: String(trip.channel) }, { label: trip.axis === 'accel' ? 'Acceleration' : trip.axis === 'velocity' ? 'Velocity' : 'Displacement', value: `${num(trip.value, 3)} ${unit}` }, { label: 'Limit', value: `${num(trip.limit, 3)} ${unit}` }];
		}
		case 'drive_vpk':
			return [...at, { label: 'Drive', value: `${num(trip.vpk)} V pk` }, { label: 'Drive abort', value: `${num(trip.abort_vpk)} V pk` }];
		case 'drive_vrms':
			return [...at, { label: 'Random drive', value: `${num(trip.realized_vrms)} V rms` }, { label: 'Drive abort', value: `${num(trip.limit_vrms)} V rms` }];
		case 'watchdog':
			return [...at, { label: 'Channel', value: String(trip.channel) }, { label: 'Read', value: `${num(trip.value)} ${trip.axis}`.trim() }, { label: 'Limit', value: `${num(trip.limit)} ${trip.axis}`.trim() }];
	}
}

/** The level card's status line for a Mixed Mode run. */
export function mixedLevelStatus(s: MixedState | null, live: MixedLiveHead | null): LevelStatus {
	const state = s?.state ?? 'unconfigured';
	const f = live && s && live.run_id === s.run_id ? live : null;
	switch (state) {
		case 'unconfigured':
			return { mark: 'idle', text: 'Not set up', tone: 'info' };
		case 'configured':
			return { mark: 'idle', text: 'Set up, ready to arm', tone: 'info' };
		case 'blocked':
			return { mark: 'hold', text: "Blocked: the test exceeds the shaker's ratings", tone: 'warn' };
		case 'armed':
			return { mark: 'armed', text: 'Armed, nothing driven', tone: 'info' };
		case 'review':
			return { mark: 'hold', text: 'Holding for review', tone: 'info' };
		case 'stopping': {
			const from = f?.level_db ?? s?.level_db ?? null;
			return { mark: 'stop', text: from !== null ? `Stopping, every part ramping down from ${db(from)}` : 'Stopping, every part ramping down', tone: 'info' };
		}
		case 'finished': {
			const outcome = s?.end?.outcome;
			if (!outcome) return { mark: 'ended', text: 'Finished', tone: 'info' };
			const tone = mixedOutcomeTone(outcome);
			return { mark: tone === 'pass' ? 'done' : 'ended', text: MIXED_OUTCOME_WORDS[outcome], tone };
		}
		case 'running':
			break;
	}
	const phase = s?.phase ?? f?.phase;
	if (phase === 'sysid') return { mark: 'identify', text: 'Identifying the system with a noise burst', tone: 'info' };
	if (phase === 'coherence_gate') return { mark: 'identify', text: 'Checking the coherence', tone: 'info' };
	if (s?.held ?? f?.held) return { mark: 'hold', text: 'Held: every part at its frequency and level', tone: 'info' };
	const level = f?.level_db ?? s?.level_db ?? null;
	const commanded = s?.level_db ?? f?.commanded_db ?? null;
	if (f && level !== null && commanded !== null && Math.abs(level - commanded) > 0.05) return { mark: 'ramp', text: `Ramping to ${db(commanded)}`, tone: 'info' };
	if (phase === 'floor_only') return { mark: 'control', text: `Floor alone at ${db(level)}, equalizing`, tone: 'info' };
	if (s?.limiting ?? f?.limiting) return { mark: 'control', text: `Limiting at ${db(level)}`, tone: 'warn' };
	return { mark: 'control', text: phase === 'full_level' ? `Every part at full level, ${db(level)}` : `Every part at ${db(level)}`, tone: 'pass' };
}

/** One part's measured value against its target: "1.21 g against 1.19 g, +0.15 dB". */
export function partErrorWords(row: Pick<MixedPartRow, 'measured' | 'target' | 'error_db' | 'unit'>): string | null {
	if (row.measured === null || row.target === null) return null;
	return `${partValue(row.measured, row.unit)} against ${partValue(row.target, row.unit)}${row.error_db === null ? '' : `, ${errDb(row.error_db)}`}`;
}

/** The derived motion of a part: "120 Hz", "300 to 600 Hz". */
export function motionWords(range: [number, number] | null): string {
	if (!range) return '-';
	return range[0] === range[1] ? `${num(range[0], 4)} Hz` : `${num(range[0], 4)} to ${num(range[1], 4)} Hz`;
}

/** The predicted peak of the sum, in a line. */
export function kinematicsWords(k: MixedDerived['kinematics']): string {
	return `${num(k.peak_accel_g, 4)} g, ${num(k.peak_velocity_ips, 4)} in/s, ${num(k.peak_disp_pp_in, 4)} in pk-pk`;
}
