/**
 * Plain words for a time waveform replication (TWR) run: what the run is doing, the play in
 * flight and the one just judged, how a run ended and what tripped it. Display only: every number
 * is the service's, and nothing here computes one. Plays are numbered from 1 on screen (the first
 * play after identification is "Play 1"); a row's `index` counts from 0.
 */

import type {
	TwrLastMetrics,
	TwrLawFaultReason,
	TwrOutcome,
	TwrPhase,
	TwrPlayInFlight,
	TwrPlayKind,
	TwrPlayRow,
	TwrPlaysCount,
	TwrRunState,
	TwrState,
	TwrTrip
} from '$lib/protocol/twr';
import type { Status } from '$lib/protocol/types';
import { formatNumber } from '$lib/viz/format';
import { db, type LevelStatus } from '../words';

export const TWR_OUTCOME_WORDS: Record<TwrOutcome, string> = {
	completed: 'Completed',
	operator_stop: 'Stopped by the Operator',
	operator_abort: 'Aborted by the Operator',
	safe_stop: 'Stopped: Nobody Held Control',
	aborted: 'Aborted by a Protection',
	not_converged: 'Stopped: A Level Did Not Meet Its Gate',
	law_fault: 'Stopped: No Usable Drive From the Control Law',
	fault: 'Stopped on a Fault'
};

export function twrOutcomeTone(outcome: TwrOutcome): Status {
	if (outcome === 'completed') return 'pass';
	if (outcome === 'operator_stop') return 'info';
	return 'fail';
}

/** The run's state in the run bar's chip: the shared words, and the coherence hold's own. */
export const TWR_STATE_WORDS: Record<TwrRunState, string> = {
	unconfigured: 'Not Set Up',
	configured: 'Ready to Arm',
	blocked: 'Blocked',
	armed: 'Armed',
	running: 'Running',
	review: 'Holding for Review',
	held: 'Held at Identification',
	stopping: 'Stopping',
	finished: 'Finished'
};

export const TWR_PHASE_WORDS: Record<TwrPhase, string> = {
	idle: 'Idle',
	feasibility_blocked: 'Over a feasibility ceiling',
	armed: 'Station open, nothing driven',
	sysid: 'System identification',
	iterating: 'A correction play',
	playing: 'A playback play',
	waiting: 'Between plays',
	holding: 'Holding at a level that did not meet its gate',
	awaiting_review: 'Identification done, waiting for you',
	coherence_hold: 'Held: the coherence stayed under its floor',
	stopping: 'Bringing the play to rest',
	stopped: 'Stopped',
	aborted: 'Aborted'
};

export const TWR_TRIP_WORDS: Record<TwrTrip['kind'], string> = {
	envelope: 'Envelope Abort',
	drive_vpk: 'Drive Ceiling',
	drive_rms: 'Drive RMS Limit',
	response_rms: 'Response RMS Limit',
	alignment: 'Alignment Check',
	coverage: 'Reference Coverage',
	divergence: 'Divergence Abort',
	not_converged: 'Iteration Limit',
	law_fault: 'Control Law Stop'
};

export const TWR_LAW_FAULT_WORDS: Record<TwrLawFaultReason, string> = {
	error: 'The control law reported an error',
	panicked: 'The control law failed while computing',
	over_budget: 'The control law took longer than its time budget',
	over_memory: 'The control law asked for more memory than its cap',
	wrong_length: 'The drive had the wrong number of samples',
	non_finite: 'The drive was not a number',
	undeclared_value: 'The control law reported a value its manifest does not declare',
	too_many_values: 'The control law reported more values than allowed',
	peak_rise: "The drive's peak rose more than the cap allows in one call",
	bin_rise: 'A frequency of the drive rose more than the cap allows in one call',
	predicted_peak: "The drive's predicted response peak was over its bound",
	first_drive: "The first drive's predicted response peak was over its bound"
};

export const PLAY_KIND_WORDS: Record<TwrPlayKind, string> = {
	iteration: 'Correction',
	playback: 'Playback'
};

const num = (v: number, digits = 3) => formatNumber(v, digits);
const pct = (v: number) => `${formatNumber(Math.abs(v), 3)}%`;

/** A play's number on screen, from its row index. */
export const playNumber = (index: number) => index + 1;

/** The level in words: "full level" at the table's last level, else its dB. */
export function levelWords(level: number | null | undefined, levels: readonly number[] | null | undefined): string {
	if (level === null || level === undefined || !Number.isFinite(level)) return '-';
	const top = levels?.length ? levels[levels.length - 1] : null;
	if (top !== null && Math.abs(level - top) < 1e-9 && Math.abs(level) < 1e-9) return 'full level';
	return db(level);
}

/** What the level card and the run view read of a run: the state with the latest frame over it. */
export type TwrRunNow = Pick<TwrState, 'state' | 'phase' | 'mode' | 'levels_db' | 'level_db' | 'rung' | 'plays' | 'identification' | 'end' | 'frozen'> & {
	/** The play in flight, from the live frame; null between plays. */
	play: TwrPlayInFlight | null;
};

/** The level card's status line for a TWR run. */
export function twrLevelStatus(s: TwrRunNow | null, excitation: 'random' | 'log_chirp' = 'random'): LevelStatus {
	const state = s?.state ?? 'unconfigured';
	switch (state) {
		case 'unconfigured':
			return { mark: 'idle', text: 'Not set up', tone: 'info' };
		case 'configured':
			return { mark: 'idle', text: 'Set up, ready to arm', tone: 'info' };
		case 'blocked':
			return { mark: 'hold', text: "Blocked: the reference's kinematics are over a feasibility ceiling", tone: 'warn' };
		case 'armed':
			return { mark: 'armed', text: 'Armed, nothing driven', tone: 'info' };
		case 'review':
			return { mark: 'hold', text: 'Holding for review', tone: 'info' };
		case 'held':
			return { mark: 'hold', text: 'Holding: the identification did not reach its coherence floor', tone: 'warn' };
		case 'stopping':
			return { mark: 'stop', text: 'Stopping: the play in flight comes to rest', tone: 'info' };
		case 'finished': {
			const outcome = s?.end?.outcome;
			if (!outcome) return { mark: 'ended', text: 'Finished', tone: 'info' };
			const tone = twrOutcomeTone(outcome);
			return { mark: tone === 'pass' ? 'done' : 'ended', text: TWR_OUTCOME_WORDS[outcome], tone };
		}
		case 'running':
			break;
	}
	const at = levelWords(s?.level_db, s?.levels_db);
	switch (s?.phase) {
		case 'sysid':
			return { mark: 'identify', text: excitation === 'log_chirp' ? 'Identifying the system with a log chirp' : 'Identifying the system with band-limited random', tone: 'info' };
		case 'iterating': {
			const n = s.play?.iteration ?? s.plays.at_level + 1;
			return { mark: 'control', text: `Correcting at ${at}, iteration ${n} of ${s.plays.of_level}`, tone: 'pass' };
		}
		case 'playing': {
			const n = s.plays.playback + 1;
			return { mark: 'control', text: `Playing ${n} of ${s.plays.of_playback} at ${at}`, tone: 'pass' };
		}
		case 'holding':
			return { mark: 'hold', text: `Holding at ${at}: the level did not meet its gate in ${s.plays.at_level} ${s.plays.at_level === 1 ? 'play' : 'plays'}`, tone: 'warn' };
		default:
			if (s?.mode === 'manual') return { mark: 'control', text: `Waiting for Iterate or Play at ${at}`, tone: 'info' };
			return { mark: 'control', text: `Between plays at ${at}`, tone: 'pass' };
	}
}

/** The play in flight in words: "Play 4, correction 2 at -6.0 dB"; null between plays. */
export function playWords(s: Pick<TwrRunNow, 'play' | 'level_db' | 'levels_db'> | null): string | null {
	const p = s?.play;
	if (!p) return null;
	const what = p.kind === 'playback' ? 'playback' : `correction ${p.iteration ?? '-'}`;
	return `Play ${playNumber(p.index)}, ${what} at ${levelWords(s?.level_db, s?.levels_db)}`;
}

/** How far through the record the play in flight is, 0 to 1; null between plays. */
export function playProgress(p: TwrPlayInFlight | null | undefined): number | null {
	if (!p || !(p.frames > 0)) return null;
	return Math.min(1, Math.max(0, p.frames_done / p.frames));
}

/**
 * The correction plays against the level's budget: the one in flight ("Iteration 2 of 8 at this
 * level"), or between plays those done ("1 of 8 correction plays at this level").
 */
export function iterationWords(plays: TwrPlaysCount | null | undefined, inFlight: TwrPlayInFlight | null | undefined): string | null {
	if (!plays) return null;
	if (inFlight?.kind === 'iteration') return `Iteration ${inFlight.iteration ?? plays.at_level + 1} of ${plays.of_level} at this level`;
	return `${plays.at_level} of ${plays.of_level} correction plays at this level`;
}

/** The playback count against the schedule: "Playback 3 of 15"; null before playback. */
export function playbackWords(plays: TwrPlaysCount | null | undefined, inFlight: TwrPlayInFlight | null | undefined): string | null {
	if (!plays || (plays.playback === 0 && inFlight?.kind !== 'playback')) return null;
	const n = plays.playback + (inFlight?.kind === 'playback' ? 1 : 0);
	return `Playback ${n} of ${plays.of_playback}`;
}

/** The last play's error against the gate: "4.2% rms error, gate 10%: met". */
export function errorAgainstGate(last: TwrLastMetrics | null | undefined, gatePct: number | null | undefined): { text: string; tone: Status } | null {
	if (!last) return null;
	const gate = gatePct === null || gatePct === undefined || !Number.isFinite(gatePct) ? '' : `, gate ${formatNumber(gatePct, 3)}%`;
	return { text: `${pct(last.rms_error_pct)} rms error${gate}: ${last.gated ? 'met' : 'not met'}`, tone: last.gated ? 'pass' : 'warn' };
}

/** The play just judged, in words: "Play 3 met the gate at -6.0 dB, 4.2% rms error". */
export function lastPlayWords(row: TwrPlayRow | null, levels: readonly number[] | null | undefined): { text: string; tone: Status } | null {
	if (!row) return null;
	const n = playNumber(row.index);
	const at = levelWords(row.level_db, levels);
	const err = `${pct(row.rms_error_pct)} rms error`;
	if (row.kind === 'playback') return { text: `Play ${n} played back at ${at}, ${err}`, tone: row.gated ? 'pass' : 'warn' };
	if (row.gated) return { text: `Play ${n} met the gate at ${at}, ${err}`, tone: 'pass' };
	return { text: `Play ${n} missed the gate at ${at}, ${err}; correcting`, tone: 'warn' };
}

/**
 * A play's alignment: its fitted lag's drift from the identification's, in frames, or that the
 * alignment check has not measured it. Never a 0 that was not measured.
 */
export function driftWords(row: Pick<TwrPlayRow, 'lag_frames' | 'drift_frames'>): string {
	if (row.drift_frames === null || row.lag_frames === null) return 'Not yet measured';
	return `${formatNumber(row.drift_frames, 3)} frames`;
}

/** The law call behind a play, in a few words. */
export function lawCallWords(row: TwrPlayRow): string {
	if (!row.law) return 'In force';
	if (!row.law.used) return row.law.miss ? `Missed: ${TWR_LAW_FAULT_WORDS[row.law.miss] ?? row.law.miss}` : 'Missed';
	return `${formatNumber(row.law.call_ms, 3)} ms`;
}

/** The trip's facts as labeled values. */
export function twrTripFacts(trip: TwrTrip): { label: string; value: string }[] {
	const at = [
		{ label: 'Play', value: trip.play === null ? 'Before the first play' : String(playNumber(trip.play)) },
		{ label: 'Level', value: trip.at_level_db === null ? '-' : db(trip.at_level_db) }
	];
	switch (trip.kind) {
		case 'envelope':
			return [
				...at,
				{ label: 'Found', value: trip.in_play ? 'During the play, from a block' : 'After the play' },
				{ label: 'Worst', value: `${formatNumber(trip.worst_frac * 100, 3)}% of the target's peak` },
				{ label: 'Abort band', value: `${formatNumber(trip.abort_frac * 100, 3)}%` },
				{ label: 'Samples in a row', value: String(trip.samples) },
				{ label: 'At record sample', value: String(trip.at_sample) }
			];
		case 'drive_vpk':
			return [
				...at,
				{ label: 'Drive', value: `${num(trip.vpk, 4)} V pk` },
				{ label: 'Ceiling', value: `${num(trip.ceiling_vpk, 4)} V pk` },
				{ label: 'Found', value: trip.predicted ? 'On the prediction, before any play' : 'On the drive, before it played' }
			];
		case 'drive_rms':
			return [...at, { label: 'Drive', value: `${num(trip.vrms, 4)} V rms` }, { label: 'Limit', value: `${num(trip.limit_vrms, 4)} V rms` }];
		case 'response_rms':
			return [...at, { label: 'Response', value: `${num(trip.rms, 4)} rms` }, { label: 'Limit', value: `${num(trip.limit, 4)} rms` }];
		case 'alignment':
			return [
				...at,
				{ label: "The play's lag", value: `${num(trip.lag_frames, 4)} frames` },
				{ label: "Identification's lag", value: `${num(trip.identification_lag_frames, 4)} frames` },
				{ label: 'Drift', value: `${num(trip.drift_frames, 3)} frames, limit ${num(trip.limit_frames, 3)}` }
			];
		case 'coverage':
			return [
				...at,
				{ label: 'Covered', value: `${formatNumber(trip.coverage * 100, 4)}% of the reference's energy` },
				{ label: 'Needed', value: `${formatNumber(trip.min_band_energy * 100, 4)}%` },
				{ label: 'Error floor', value: pct(trip.error_floor_pct) }
			];
		case 'divergence':
			return [...at, { label: 'Plays with a growing error', value: String(trip.plays) }, { label: 'Error', value: pct(trip.error_pct) }];
		case 'not_converged':
			return [...at, { label: 'Correction plays', value: String(trip.iterations) }, { label: 'Error', value: pct(trip.error_pct) }, { label: 'Gate', value: pct(trip.gate_pct) }];
		case 'law_fault':
			return [...at, { label: 'Why', value: TWR_LAW_FAULT_WORDS[trip.reason] ?? trip.reason }, { label: 'In a row', value: String(trip.consecutive) }];
	}
}
