/**
 * Plain words for a MIMO Shock run: what the run is doing, each control's verdict and why it
 * failed, how the run ended and what stopped it, the ladder and the reachable floors before the
 * first pulse. Display only: every number is the service's, and nothing here computes a
 * measurement. Pulses are numbered from 1 on screen; a row's `index` counts from 0. Controls are
 * named by their channel ("Control 3" is the table's row 3).
 */

import type {
	MimoShockControlVerdict,
	MimoShockEnd,
	MimoShockFailReason,
	MimoShockLawMiss,
	MimoShockOutcome,
	MimoShockPhase,
	MimoShockPulseRow,
	MimoShockReview,
	MimoShockRunState,
	MimoShockState
} from '$lib/protocol/mimoShock';
import type { Status } from '$lib/protocol/types';
import { formatNumber } from '$lib/viz/format';
import { db, type LevelStatus } from '../words';

export const MS_OUTCOME_WORDS: Record<MimoShockOutcome, string> = {
	completed: 'Completed',
	operator_stop: 'Stopped by the Operator',
	operator_abort: 'Aborted by the Operator',
	safe_stop: 'Stopped by a Guard',
	aborted: 'Aborted by a Protection',
	not_converged: 'Stopped: A Level Did Not Pass',
	held: 'Held at Identification',
	law_fault: 'Stopped: No Usable Drives From the Control Law',
	fault: 'Stopped on a Fault'
};

export function msOutcomeTone(outcome: MimoShockOutcome): Status {
	if (outcome === 'completed') return 'pass';
	if (outcome === 'operator_stop') return 'info';
	return 'fail';
}

export const MS_PHASE_WORDS: Record<MimoShockPhase, string> = {
	idle: 'Idle',
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

export const MS_STATE_WORDS: Record<MimoShockRunState, string> = {
	unconfigured: 'Not set up',
	configured: 'Set up',
	armed: 'Armed',
	running: 'Running',
	review: 'Holding for review',
	stopping: 'Stopping',
	finished: 'Finished'
};

/** Why a control failed, by name, in a word, and in a sentence. */
export const FAIL_WORDS: Record<MimoShockFailReason, { word: string; short: string; sentence: string }> = {
	tolerance: { word: 'Tolerance', short: 'Tol', sentence: 'outside its tolerance' },
	timing: { word: 'Timing', short: 'Time', sentence: 'arrived outside its timing window' },
	polarity: { word: 'Polarity', short: 'Pol', sentence: 'came out upside down' },
	duration: { word: 'Duration', short: 'Dur', sentence: 'lasted too long or too short' }
};

export const MS_LAW_MISS_WORDS: Record<MimoShockLawMiss, string> = {
	error: 'the control law reported an error',
	panicked: 'the control law failed while computing',
	over_budget: 'the control law took longer than its time budget',
	wrong_length: 'the drives had the wrong number of samples',
	non_finite: 'a drive was not a number',
	undeclared_value: 'the control law reported a value its manifest does not declare',
	too_many_values: 'the control law reported more values than allowed',
	peak_rise: "a drive's peak grew more than the cap allows in one pulse",
	bin_rise: 'a band of a drive grew more than the cap allows in one pulse',
	predicted_peak: 'the predicted response of the next drives was over an abort line',
	first_drive: "the first drives' predicted response was over its bound",
	target_not_accepted: 'the control law does not accept this kind of target'
};

/** Why the run ended, in plain words, by the end's reason. */
const END_REASON_WORDS: Record<string, string> = {
	not_converged: "A level's pulse allowance was spent without every control passing.",
	divergence: 'The divergence watch stopped the run: the error, or a drive, kept growing at one level.',
	max_pulses: "The run reached its cap on pulses.",
	predicted_abort: 'The predicted response of the next pulse was over an abort line, so it was not fired.',
	orphan: 'Nobody held control, so nothing more was fired.',
	shutdown: 'The control service shut down.',
	drive_abort: 'A drive went over its abort voltage.',
	control_abort_absolute: "A control's pulse went over its absolute abort line, the line at full level, which always ends the run.",
	control_abort_scaled: "A control's pulse went over its abort line scaled to the level played, and scaled aborts were on.",
	limit_abort: 'A limit channel went over its abort peak.',
	kinematics: "A control's velocity or displacement went over its limit.",
	low_coherence: 'Identification held the run: a control was not measured with enough coherence.',
	reference_out_of_band: "Identification held the run: too little of a target's energy is on lines that can be trusted.",
	operator_stop: 'Stopped by the operator.',
	operator_abort: 'Aborted by the operator.'
};

/** The run's end in a sentence a person can act on: what happened, and where it happened. */
export function endWords(end: MimoShockEnd): string {
	let why: string;
	if (end.outcome === 'completed') why = 'Full level passed its pulses.';
	else if (end.outcome === 'law_fault') {
		const miss = end.reason ? MS_LAW_MISS_WORDS[end.reason as MimoShockLawMiss] : null;
		why = miss ? `The control law gave no usable drives: ${miss}.` : 'The control law gave no usable drives.';
	} else if (end.outcome === 'fault') why = end.message || 'The run or the station failed.';
	else why = (end.reason && END_REASON_WORDS[end.reason]) || END_REASON_WORDS[end.outcome] || end.message;
	const where = end.pulse === null ? (end.outcome === 'completed' ? '' : ' Before the first pulse.') : ` At pulse ${end.pulse + 1}${end.level_db === null ? '' : `, ${db(end.level_db)}`}.`;
	return `${why}${where}`;
}

/** Which abort line ended the run, when one did: `scaled`, `absolute`, or null. */
export function abortLineOf(end: MimoShockEnd | null): 'scaled' | 'absolute' | null {
	if (!end) return null;
	if (end.reason === 'control_abort_scaled') return 'scaled';
	if (end.reason === 'control_abort_absolute') return 'absolute';
	return null;
}

const num = (v: number, digits = 3) => formatNumber(v, digits);

/** A signed dB value: "+1.3 dB" over, "-0.4 dB" under. */
export function signedDb(v: number | null | undefined, digits = 1): string {
	if (v === null || v === undefined || !Number.isFinite(v)) return '-';
	const s = v.toFixed(digits);
	return `${v > 0 ? '+' : ''}${s === `-${(0).toFixed(digits)}` ? (0).toFixed(digits) : s} dB`;
}

/** "Over-test" or "Under-test" for a signed error, or "On target" inside a tenth of a dB. */
export function overUnder(v: number): string {
	if (Math.abs(v) < 0.05) return 'On target';
	return v > 0 ? 'Over-test' : 'Under-test';
}

/** A control's failure reasons in words: "Tolerance and timing". */
export function reasonsWords(reasons: readonly MimoShockFailReason[]): string {
	const w = reasons.map((r, i) => (i === 0 ? FAIL_WORDS[r]?.word ?? r : (FAIL_WORDS[r]?.word ?? r).toLowerCase()));
	if (w.length <= 1) return w[0] ?? '';
	return `${w.slice(0, -1).join(', ')} and ${w[w.length - 1]}`;
}

/** One control's verdict in a sentence. */
export function controlVerdictWords(c: MimoShockControlVerdict): string {
	if (c.passed) return `Control ${c.channel} passed, ${signedDb(c.over_test_db)} (${overUnder(c.over_test_db).toLowerCase()}).`;
	const why = c.reasons.map((r) => FAIL_WORDS[r]?.sentence ?? r);
	return `Control ${c.channel} failed: ${why.length ? why.join('; ') : 'not judged a pass'}, ${signedDb(c.over_test_db)} (${overUnder(c.over_test_db).toLowerCase()}).`;
}

/** The pulse just fired, in words: "Pulse 5 at -6 dB: 3 of 4 controls passed". */
export function lastPulseWords(row: MimoShockPulseRow | null): { text: string; tone: Status } | null {
	if (!row) return null;
	const n = row.index + 1;
	const passed = row.controls.filter((c) => c.passed).length;
	const at = ` at ${db(row.level_db)}`;
	if (row.passed) return { text: `Pulse ${n}${at} passed: ${passed} of ${row.controls.length} controls inside tolerance`, tone: 'pass' };
	return { text: `Pulse ${n}${at} did not pass: ${passed} of ${row.controls.length} controls inside tolerance`, tone: 'warn' };
}

/** The controls over their scaled abort line on a pulse, and by how much (dB), from their margins. */
export function scaledCrossings(row: MimoShockPulseRow): { channel: number; over_db: number }[] {
	return row.controls.filter((c) => c.scaled_margin_db > 0).map((c) => ({ channel: c.channel, over_db: c.scaled_margin_db }));
}

/** A pulse's flag for the controls over their scaled abort line, or null when none is. */
export function scaledCrossingWords(row: MimoShockPulseRow): string | null {
	const list = scaledCrossings(row);
	if (!list.length) return null;
	const parts = list.map((x) => `control ${x.channel} by ${num(x.over_db, 2)} dB`);
	return `Pulse ${row.index + 1} went over the scaled abort line at ${parts.join(', ')}.`;
}

/** A margin to an abort line in words: "1.3 dB over" or "2.3 dB under". */
export function marginWords(v: number | null | undefined): string {
	if (v === null || v === undefined || !Number.isFinite(v)) return '-';
	return `${formatNumber(Math.abs(v), 2)} dB ${v > 0 ? 'over' : 'under'}`;
}

/** The arrival spread, as a warning, when the pulse's warnings carry one, else null. */
export function spreadWarning(row: MimoShockPulseRow, windowS: number | null): string | null {
	if (windowS === null || !(row.arrival_spread_s > windowS)) return null;
	return `The controls' arrivals spread over ${formatNumber(row.arrival_spread_s * 1000, 3)} ms, past the ${formatNumber(windowS * 1000, 3)} ms window.`;
}

/** The ladder's half step, or an engine-halved step, in a sentence; null when the step was the table's. */
export function stepNoteWords(row: MimoShockPulseRow): string | null {
	const s = row.step_note;
	if (!s) return null;
	return `The ladder stepped to ${db(s.taken_db)} instead of ${db(s.planned_db)}: the first pulses predicted ${signedDb(s.predicted_planned_db)} at ${db(s.planned_db)}, past the ${num(s.tolerance_db, 2)} dB tolerance (${signedDb(s.predicted_taken_db)} predicted at ${db(s.taken_db)}).`;
}

// ---------------------------------------------------------------------------------------------
// The review before the first pulse

/** The controls whose reachable floor is outside their own tolerance. */
export function unreachableControls(review: MimoShockReview | null): number[] {
	return (review?.controls ?? []).filter((c) => !c.reachable_ok).map((c) => c.channel);
}

/** The hold the operator must acknowledge before any pulse, or null when there is none. */
export function needsAcknowledgement(s: Pick<MimoShockState, 'state' | 'review'> | null): boolean {
	return !!s && s.state === 'review' && s.review?.hold?.reason === 'unreachable_target' && !s.review.acknowledged;
}

// ---------------------------------------------------------------------------------------------
// The ladder

export interface LadderRung {
	db: number;
	fired: number;
	passed: number;
	/** The level in force now. */
	now: boolean;
	/** A level the table does not hold: an engine-halved step. */
	halved: boolean;
}

/**
 * The ladder as fired so far: every level of the table, with the pulses fired and passed at each
 * from the pulse rows, and any level the engine stepped to off the table (a halved step) in its
 * place. Counting rows is display, not measurement.
 */
export function ladderRungs(levels: readonly number[] | null, rows: readonly MimoShockPulseRow[], level: number | null): LadderRung[] {
	const out: LadderRung[] = (levels ?? []).map((v) => ({ db: v, fired: 0, passed: 0, now: false, halved: false }));
	const near = (a: number, b: number) => Math.abs(a - b) < 1e-6;
	for (const r of rows) {
		let rung = out.find((x) => near(x.db, r.level_db));
		if (!rung) {
			rung = { db: r.level_db, fired: 0, passed: 0, now: false, halved: true };
			out.push(rung);
		}
		rung.fired += 1;
		if (r.passed) rung.passed += 1;
	}
	if (level !== null) {
		const at = out.find((x) => near(x.db, level));
		if (at) at.now = true;
		else out.push({ db: level, fired: 0, passed: 0, now: true, halved: true });
	}
	return out.sort((a, b) => a.db - b.db);
}

/** A level's role in the ladder, in words: the half step before full level reads as such. */
export function rungWord(r: LadderRung, levels: readonly number[] | null): string {
	if (Math.abs(r.db) < 1e-9) return 'Full level';
	if (r.halved) return 'Halved step';
	const l = levels ?? [];
	const i = l.findIndex((v) => Math.abs(v - r.db) < 1e-9);
	if (i === l.length - 2 && i > 0 && Math.abs(l[l.length - 1] - r.db) < Math.abs(l[i] - l[i - 1]) - 1e-9) return 'Half step';
	return '';
}

// ---------------------------------------------------------------------------------------------
// The level card's status line

export function msLevelStatus(s: MimoShockState | null): LevelStatus {
	const state = s?.state ?? 'unconfigured';
	switch (state) {
		case 'unconfigured':
			return { mark: 'idle', text: 'Not set up', tone: 'info' };
		case 'configured':
			return { mark: 'idle', text: 'Set up, ready to start', tone: 'info' };
		case 'armed':
			return { mark: 'armed', text: 'Armed, nothing driven', tone: 'info' };
		case 'review':
			return needsAcknowledgement(s)
				? { mark: 'hold', text: 'Holding: a control cannot reach its target', tone: 'warn' }
				: { mark: 'hold', text: 'Holding for review', tone: 'info' };
		case 'stopping':
			return { mark: 'stop', text: 'Stopping: the pulse in flight finishes, nothing more fires', tone: 'info' };
		case 'finished': {
			const outcome = s?.end?.outcome;
			if (!outcome) return { mark: 'ended', text: 'Finished', tone: 'info' };
			const tone = msOutcomeTone(outcome);
			return { mark: tone === 'pass' ? 'done' : 'ended', text: MS_OUTCOME_WORDS[outcome], tone };
		}
		case 'running':
			break;
	}
	if (s?.phase === 'sysid') return { mark: 'identify', text: 'Identifying the system on every drive at once', tone: 'info' };
	if (s?.phase === 'firing') return { mark: 'control', text: `Firing at ${db(s.level_db)}`, tone: 'pass' };
	if (s?.mode === 'manual') return { mark: 'control', text: `Waiting for Fire at ${db(s?.level_db)}`, tone: 'info' };
	return { mark: 'control', text: `Between pulses at ${db(s?.level_db)}`, tone: 'pass' };
}

/** The verdict mode in words. */
export const VERDICT_WORDS: Record<'all' | 'global', { label: string; help: string }> = {
	all: { label: 'Every control', help: 'A pulse passes when every control is inside its own tolerance.' },
	global: { label: 'Global SRS error', help: "A pulse passes when the energy-weighted SRS error over every control is inside the tolerance and no control is over its abort line." }
};
