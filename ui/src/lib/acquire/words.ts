/**
 * Plain words for the Random control surface: states, phases, outcomes, trips, alarms and
 * refusals. A refusal is shown by a heading chosen from its `reason` and the service's own
 * `message`, verbatim (the protocol's rule: a client branches on `reason`, never parses
 * `message`). Display only: nothing here computes a number.
 */

import { RequestError } from '$lib/protocol/client.svelte';
import type {
	Alarm,
	LogKind,
	RandomOutcome,
	RandomPhase,
	RandomRunState,
	RefusalReason,
	Status,
	Trip
} from '$lib/protocol/types';
import { formatNumber } from '$lib/viz/format';

/** A signed dB value: "+3.0 dB", "-12.0 dB". */
export function db(v: number | null | undefined, digits = 1): string {
	if (v === null || v === undefined || !Number.isFinite(v)) return '-';
	const s = Math.abs(v).toFixed(digits);
	const sign = Number(s) === 0 ? '' : v > 0 ? '+' : '-';
	return `${sign}${s} dB`;
}

/** Run clock: "42.0 s" under a minute, "12:05" from a minute, "1:02:05" from an hour. */
export function clock(seconds: number | null | undefined): string {
	if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return '-';
	if (seconds < 60) return `${seconds.toFixed(1)} s`;
	const t = Math.floor(seconds);
	const h = Math.floor(t / 3600);
	const m = Math.floor((t % 3600) / 60);
	const s = String(t % 60).padStart(2, '0');
	return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

export const STATE_WORDS: Record<RandomRunState, string> = {
	unconfigured: 'Not Set Up',
	configured: 'Ready to Arm',
	blocked: 'Blocked',
	armed: 'Armed',
	running: 'Running',
	review: 'Holding for Review',
	stopping: 'Stopping',
	finished: 'Finished'
};

const PHASE_WORDS: Record<RandomPhase, string> = {
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

/** The controller's phase in words; a phase this shell does not know is shown as named. */
export function phaseWord(phase: string | null | undefined): string {
	if (!phase) return '-';
	return PHASE_WORDS[phase as RandomPhase] ?? phase.replaceAll('_', ' ').replace(/^./, (c) => c.toUpperCase());
}

export const OUTCOME_WORDS: Record<RandomOutcome, string> = {
	completed: 'Completed',
	operator_stop: 'Stopped by the Operator',
	operator_abort: 'Aborted by the Operator',
	safe_stop: 'Stopped: Nobody Held Control',
	aborted: 'Aborted by a Protection',
	held: 'Held at the Coherence Check',
	sysid_level_unreachable: 'Stopped: Identification Level Not Reached',
	fault: 'Stopped on a Fault'
};

/** Every outcome but a completed schedule and the operator's own stop is shown as an abort. */
export function outcomeTone(outcome: RandomOutcome): Status {
	if (outcome === 'completed') return 'pass';
	if (outcome === 'operator_stop') return 'info';
	return 'fail';
}

export const TRIP_WORDS: Record<Trip['kind'], string> = {
	envelope: 'Envelope Abort',
	kinematic: 'Kinematic Abort',
	drive_vrms: 'Drive Abort',
	divergence: 'Divergence Abort'
};

const AXIS_WORDS = { accel: 'Acceleration', velocity: 'Velocity', displacement: 'Displacement' } as const;
const AXIS_UNITS = { accel: 'g', velocity: 'in/s', displacement: 'mil' } as const;

/** The trip's facts as labeled values, the axis first for a kinematic trip. */
export function tripFacts(trip: Trip): { label: string; value: string }[] {
	const common = [
		{ label: 'At level', value: db(trip.at_level_db) },
		{ label: 'At loop', value: String(trip.at_loop) }
	];
	switch (trip.kind) {
		case 'envelope':
			return [{ label: 'Lines outside the abort band', value: String(trip.abort_lines) }, { label: 'Worst line', value: db(trip.worst_db) }, ...common];
		case 'kinematic':
			return [
				{ label: 'Axis', value: AXIS_WORDS[trip.axis] ?? trip.axis },
				{ label: 'Channel', value: String(trip.channel) },
				{ label: 'Value', value: `${formatNumber(trip.value, 4)} ${AXIS_UNITS[trip.axis] ?? ''}`.trim() },
				{ label: 'Limit', value: `${formatNumber(trip.limit, 4)} ${AXIS_UNITS[trip.axis] ?? ''}`.trim() },
				...common
			];
		case 'drive_vrms':
			return [{ label: 'Drive', value: `${formatNumber(trip.realized_vrms, 4)} V rms` }, { label: 'Limit', value: `${formatNumber(trip.limit_vrms, 4)} V rms` }, ...common];
		case 'divergence':
			return [{ label: 'Loops in a row', value: String(trip.consecutive_loops) }, { label: 'Diverging lines', value: String(trip.divergent_bins) }, ...common];
	}
	return common;
}

export function alarmText(a: Alarm, channelName?: (index: number) => string): string {
	if (a.kind === 'envelope') return `${a.lines} ${a.lines === 1 ? 'line' : 'lines'} outside the alarm band, worst ${db(a.worst_db)}`;
	if (a.kind === 'overload') return `${channelName?.(a.channel) ?? `Channel ${a.channel}`} reached 90 % of its range`;
	return 'Alarm';
}

export const LOG_WORDS: Record<LogKind, string> = {
	state: 'State',
	feasibility: 'Feasibility',
	override: 'Override',
	station: 'Station',
	gate: 'Gate',
	level: 'Level',
	alarm: 'Alarm',
	overload: 'Overload',
	trip: 'Trip',
	note: 'Note'
};

// ---------------------------------------------------------------------------------------------
// Refusals

const REFUSAL_TITLES: Record<RefusalReason, string> = {
	bad_params: 'Request Not Accepted',
	not_controller: 'This Screen Is Observing',
	controller_held: 'Another Screen Has Control',
	run_active: 'A Run Is Live',
	not_configured: 'Not Set Up',
	run_finished: 'The Run Has Finished',
	wrong_state: 'Not Available Now',
	manual_mode_required: 'Manual Mode Needed',
	not_a_rung: 'Not a Level of the Schedule',
	feasibility_exceeded: 'Exceeds the Shaker Ratings',
	override_not_needed: 'No Override Needed',
	arm_refused: 'The Station Refused to Arm',
	ni_driver_unavailable: 'NI-DAQmx Is Not Installed',
	no_ni_devices: 'No NI Devices Found',
	scan_failed: 'The Device Scan Failed',
	devices_absent: 'Devices Not Found',
	mixed_simulated_and_real: 'Simulated and Physical Devices Mixed',
	mixed_timing_families: 'Devices Cannot Share One Clock',
	not_enough_channels: 'Not Enough Channels',
	hardware_busy: 'The Hardware Is In Use',
	stale_revision: 'The Channel Table Changed',
	table_invalid: 'The Channel Table Has Errors',
	config_invalid: 'The Setup Has Errors',
	shaker_record_invalid: 'The Shaker Record Does Not Read'
};

export function refusalTitle(reason: string | null | undefined, code?: string): string {
	if (reason && reason in REFUSAL_TITLES) return REFUSAL_TITLES[reason as RefusalReason];
	switch (code) {
		case 'timeout':
			return 'No Answer in Time';
		case 'disconnected':
		case 'offline':
			return 'Not Connected';
		case 'unknown_method':
			return 'Not Supported by This Service';
		case 'internal':
			return 'The Service Failed';
		default:
			return 'Not Done';
	}
}

const list = (v: unknown): string => (Array.isArray(v) ? v.map(String).join(', ') : String(v));

/** The facts a refusal carries, in words. Only the facts the protocol names are shown. */
export function refusalFacts(reason: string | null | undefined, detail: Record<string, unknown> | undefined): string[] {
	if (!detail) return [];
	const out: string[] = [];
	const has = (k: string) => detail[k] !== undefined && detail[k] !== null;
	switch (reason) {
		case 'not_controller':
		case 'controller_held':
		case 'hardware_busy':
			if (has('holder')) out.push(`Held by ${String(detail.holder)}`);
			break;
		case 'run_active':
			if (has('run_id')) out.push(`Run ${String(detail.run_id)}`);
			break;
		case 'run_finished':
			if (has('outcome')) out.push(`Outcome: ${OUTCOME_WORDS[detail.outcome as RandomOutcome] ?? String(detail.outcome)}`);
			break;
		case 'wrong_state':
			if (has('state')) out.push(`State: ${STATE_WORDS[detail.state as RandomRunState] ?? String(detail.state)}, ${phaseWord(String(detail.phase ?? ''))}`);
			break;
		case 'manual_mode_required':
			if (has('mode')) out.push(`Ladder mode: ${String(detail.mode)}`);
			break;
		case 'not_a_rung':
			if (has('levels_db')) out.push(`Schedule levels: ${list(detail.levels_db)} dB`);
			break;
		case 'devices_absent':
			if (has('missing')) out.push(`Missing: ${list(detail.missing)}`);
			if (has('found')) out.push(`Found: ${list(detail.found)}`);
			break;
		case 'mixed_simulated_and_real':
			if (has('simulated')) out.push(`Driver-simulated: ${list(detail.simulated)}`);
			if (has('real')) out.push(`Physical: ${list(detail.real)}`);
			break;
		case 'mixed_timing_families':
			if (has('families')) out.push(`Clock families: ${list(detail.families)}`);
			break;
		case 'not_enough_channels':
			out.push(`Needed ${String(detail.want_inputs ?? '-')} inputs and ${String(detail.want_outputs ?? '-')} outputs; the devices have ${String(detail.have_inputs ?? '-')} and ${String(detail.have_outputs ?? '-')}`);
			break;
		case 'stale_revision':
			if (has('current')) out.push(`The service holds revision ${String(detail.current)}; these edits started from revision ${String(detail.provided ?? '-')}`);
			break;
		case 'bad_params':
			if (has('param')) out.push(`Parameter: ${String(detail.param)}`);
			break;
	}
	return out;
}

export interface Refusal {
	/** What the operator asked for, in words ("Arm", "Take Control"). */
	verb: string;
	reason: string | null;
	code: string;
	title: string;
	/** The service's words, shown verbatim. */
	message: string;
	facts: string[];
	detail: Record<string, unknown> | undefined;
}

export function toRefusal(verb: string, err: unknown): Refusal {
	if (err instanceof RequestError) {
		return {
			verb,
			reason: err.reason ?? null,
			code: err.code,
			title: refusalTitle(err.reason, err.code),
			message: err.message,
			facts: refusalFacts(err.reason, err.detail),
			detail: err.detail
		};
	}
	const message = err instanceof Error ? err.message : String(err);
	return { verb, reason: null, code: 'internal', title: refusalTitle(null, 'internal'), message, facts: [], detail: undefined };
}
