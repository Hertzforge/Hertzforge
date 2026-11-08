/**
 * Plain words for the Random control surface: states, phases, outcomes, trips, alarms and
 * refusals. A refusal is shown by a heading chosen from its `reason` and the service's own
 * `message`, verbatim (the protocol's rule: a client branches on `reason`, never parses
 * `message`). Display only: nothing here computes a number.
 */

import { RequestError } from '$lib/protocol/client.svelte';
import type {
	Alarm,
	LawFaultReason,
	LogKind,
	RandomLiveHead,
	RandomOutcome,
	RandomPhase,
	RandomRunState,
	RandomState,
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
	fault: 'Stopped on a Fault',
	law_fault: 'Stopped: No Usable Drive From the Control Law'
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
	divergence: 'Divergence Abort',
	law_fault: 'Control Law Stop'
};

/** What was wrong with the control law's last unusable output, in words. */
export const LAW_FAULT_WORDS: Record<LawFaultReason, string> = {
	error: 'The control law reported an error',
	panicked: 'The control law failed while computing',
	over_budget: 'The control law took longer than its time budget',
	wrong_length: 'The drive had the wrong number of lines',
	non_finite: 'The drive was not a number',
	negative: 'The drive had a negative value',
	non_finite_level: 'The level was not a number',
	over_memory: 'The control law asked for more memory than its cap',
	bad_diagnostics: 'The control law reported diagnostics that broke the output rules'
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
		case 'law_fault':
			return [{ label: 'Loops in a row', value: String(trip.consecutive) }, { label: 'Last problem', value: LAW_FAULT_WORDS[trip.reason] ?? trip.reason }, ...common];
	}
	return common;
}

export function alarmText(a: Alarm, channelName?: (index: number) => string): string {
	if (a.kind === 'envelope') return `${a.lines} ${a.lines === 1 ? 'line' : 'lines'} outside the alarm band, worst ${db(a.worst_db)}`;
	if (a.kind === 'overload') return `${channelName?.(a.channel) ?? `Channel ${a.channel}`} reached the top of its range`;
	return 'Alarm';
}

/** What the Inputs card's Range column means, for its heading's tooltip. */
export const RANGE_MEANING = 'OK, or Overload once the input reaches the top of its range in this run: 90 % of it in the run, or 98 % at the station.';

/** The Inputs card's note while inputs are in overload: which, and what it means; null when none is. */
export function overloadNote(channels: number[]): string | null {
	if (!channels.length) return null;
	const which = channels.length === 1 ? `channel ${channels[0]} reached the top of its range` : `channels ${channels.join(', ')} reached the top of their range`;
	return `Overload: ${which} during this run (90 % of it in the run, or 98 % at the station).`;
}

/** The level card's status line: what the run is doing now, in plain words. */
export interface LevelStatus {
	/** The shape beside the words: no meaning rests on the color alone. */
	mark: 'idle' | 'armed' | 'identify' | 'hold' | 'control' | 'ramp' | 'stop' | 'done' | 'ended';
	text: string;
	tone: Status;
}

/** The run is moving to a new level: the drive is not yet where it was commanded. */
export function ramping(frame: Pick<RandomLiveHead, 'level_db' | 'commanded_db'> | null): boolean {
	return !!frame && frame.level_db !== null && frame.commanded_db !== null && Math.abs(frame.level_db - frame.commanded_db) > 0.05;
}

export function levelStatus(run: RandomState | null, frame: RandomLiveHead | null): LevelStatus {
	const state = run?.state ?? 'unconfigured';
	const f = frame && run && frame.run_id === run.run_id ? frame : null;
	switch (state) {
		case 'unconfigured':
			return { mark: 'idle', text: 'Not set up', tone: 'info' };
		case 'configured':
			return { mark: 'idle', text: 'Set up, ready to arm', tone: 'info' };
		case 'blocked':
			return { mark: 'hold', text: "Blocked: the profile exceeds the shaker's ratings", tone: 'warn' };
		case 'armed':
			return { mark: 'armed', text: 'Armed, nothing driving', tone: 'info' };
		case 'review':
			return { mark: 'hold', text: 'Holding for review', tone: 'info' };
		case 'stopping':
			return { mark: 'stop', text: f?.level_db !== null && f?.level_db !== undefined ? `Stopping, ramping down from ${db(f.level_db)}` : 'Stopping', tone: 'info' };
		case 'finished': {
			const outcome = run?.end?.outcome;
			if (!outcome) return { mark: 'ended', text: 'Finished', tone: 'info' };
			const tone = outcomeTone(outcome);
			return { mark: tone === 'pass' ? 'done' : 'ended', text: OUTCOME_WORDS[outcome], tone };
		}
		case 'running':
			break;
	}
	if (run?.phase === 'sysid') return { mark: 'identify', text: `Identifying the system at ${db(run.level_db)} against the reference`, tone: 'info' };
	if (run?.phase === 'coherence_gate') return { mark: 'identify', text: 'Checking the coherence', tone: 'info' };
	if (ramping(f)) return { mark: 'ramp', text: `Ramping to ${db(f?.commanded_db)}`, tone: 'info' };
	const level = f?.level_db ?? run?.level_db ?? null;
	if (run?.phase === 'full_level') return { mark: 'control', text: `Controlling at full level, ${db(level)}`, tone: 'pass' };
	return { mark: 'control', text: `Controlling at ${db(level)}`, tone: 'pass' };
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
	note: 'Note',
	pulse: 'Pulse',
	law: 'Control Law'
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
	shaker_record_invalid: 'The Shaker Record Does Not Read',
	setup_invalid: 'The Setup Has Errors',
	gate_failed: 'Identification Failed a Gate',
	prediction_exceeded: 'A Drive Is Predicted Over Its Ceiling',
	law_import_failed: 'The Law Was Not Imported',
	catalog_unavailable: 'The Law Catalog Cannot Be Read',
	law_unknown: 'The Law Is Not in the Catalog',
	law_params_invalid: 'The Law Parameters Are Not Valid',
	law_check_unsupported: 'This Law Cannot Be Checked Here',
	law_check_running: 'A Law Check Is Already Running',
	law_check_not_recorded: 'The Check Was Not Recorded',
	law_not_checked: 'The Law Has Not Passed Its Check',
	shutting_down: 'The Control Service Is Shutting Down'
};

/** Refusals of one test type's own methods (classical shock's, Sine's, TWR's), beside the shared ones. */
const MORE_REFUSAL_TITLES: Record<string, string> = {
	not_yet: 'Not Yet',
	wrong_mode: 'Not in This Ladder Mode',
	drive_ceiling: 'Over the Drive Abort',
	not_a_sweep: 'Not a Sweep',
	out_of_band: 'Outside the Band',
	adjust_refused: 'Parameter Not Changed',
	reference_unreadable: 'The Reference Could Not Be Read',
	not_converged: 'The Level Has Not Met Its Gate',
	coherence_hold: 'Held at the Coherence Floor'
};

/** Why a live law parameter was not changed (`adjust_refused`'s `why`). */
const ADJUST_WHY: Record<string, string> = {
	unknown_key: 'The law has no parameter by that name.',
	not_live: 'That parameter is fixed for the run.',
	out_of_range: "The value is outside the parameter's range.",
	law_refused: 'The control law refused the value.'
};

export function refusalTitle(reason: string | null | undefined, code?: string): string {
	if (reason && reason in REFUSAL_TITLES) return REFUSAL_TITLES[reason as RefusalReason];
	if (reason && reason in MORE_REFUSAL_TITLES) return MORE_REFUSAL_TITLES[reason];
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
		case 'not_yet':
			if (has('seconds')) out.push(`Ready in ${String(detail.seconds)} s`);
			break;
		case 'wrong_mode':
			if (has('mode')) out.push(`Ladder mode: ${String(detail.mode)}`);
			break;
		case 'drive_ceiling':
			// TWR has no drive abort: its refusal names the ceiling.
			if (has('vpk') && has('ceiling_vpk')) out.push(`Drive ${String(detail.vpk)} V pk at ${String(detail.level_db ?? '-')} dB; the drive ceiling is ${String(detail.ceiling_vpk)} V pk`);
			else if (has('ceiling_vpk')) out.push(`At ${String(detail.level_db ?? '-')} dB the drive would pass the drive ceiling, ${String(detail.ceiling_vpk)} V pk`);
			else if (has('vpk')) out.push(`Drive ${String(detail.vpk)} V pk at ${String(detail.level_db ?? '-')} dB; the drive abort is ${String(detail.abort_vpk ?? '-')} V pk`);
			break;
		case 'not_converged':
			if (has('gate_pct')) out.push(`Gate: ${String(detail.gate_pct)}% rms error`);
			break;
		case 'law_not_checked':
			if (has('tool_version') && has('current')) out.push(`Checked with ${String(detail.tool_version)}; the check in force is ${String(detail.current)}`);
			break;
		case 'coherence_hold':
			if (has('coherence_mean') && has('min_mean_coherence')) out.push(`Mean coherence ${String(detail.coherence_mean)}, under ${String(detail.min_mean_coherence)}`);
			break;
		case 'reference_unreadable':
			if (has('line')) out.push(`Line ${String(detail.line)}`);
			if (has('path')) out.push(`File: ${String(detail.path)}`);
			break;
		case 'not_a_sweep':
			if (has('test')) out.push(`Test: ${String(detail.test)}`);
			break;
		case 'out_of_band':
			if (Array.isArray(detail.band_hz)) out.push(`Band: ${String(detail.band_hz[0])} to ${String(detail.band_hz[1])} Hz`);
			break;
		case 'adjust_refused':
			if (has('key')) out.push(`Parameter: ${String(detail.key)}`);
			if (has('why')) out.push(ADJUST_WHY[String(detail.why)] ?? String(detail.why));
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
		// A drive-ceiling refusal that names the ceiling (TWR's) is over the ceiling, not an abort.
		const ceiling = (err.reason as string | undefined) === 'drive_ceiling' && err.detail?.ceiling_vpk !== undefined;
		return {
			verb,
			reason: err.reason ?? null,
			code: err.code,
			title: ceiling ? 'Over the Drive Ceiling' : refusalTitle(err.reason, err.code),
			message: err.message,
			facts: refusalFacts(err.reason, err.detail),
			detail: err.detail
		};
	}
	const message = err instanceof Error ? err.message : String(err);
	return { verb, reason: null, code: 'internal', title: refusalTitle(null, 'internal'), message, facts: [], detail: undefined };
}
