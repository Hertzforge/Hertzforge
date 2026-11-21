/**
 * Modal Test in words: the state in large words, each hit's verdict and flags, the counter, the
 * force target signal, the outcomes and the shaker sources. Every number shown is the control
 * service's (its counts, its checks, its percent from the target); these functions only put the
 * words around them.
 */

import type { ModalCounts, ModalDecay, ModalDrivePhase, ModalHitReport, ModalLatest, ModalOutcome, ModalRejectReason, ModalRunState, ModalSource, ModalTargetVerdict, ModalTile } from '$lib/protocol/modal_test';

/** The state in large words. */
export function modalStateWords(state: ModalRunState | null | undefined, mode: 'impact' | 'shaker' | null | undefined): string {
	switch (state) {
		case 'configured':
			return 'Ready to Start';
		case 'starting':
			return 'Starting';
		case 'idle':
			return mode === 'shaker' ? 'Ready to Drive' : 'Ready';
		case 'arming':
			return 'Arming';
		case 'armed':
			return 'Waiting for Hit';
		case 'captured':
			return 'Captured';
		case 'measuring':
			return 'Driving';
		case 'stopping':
			return 'Ending';
		case 'point_done':
			return 'Point Done';
		case 'finished':
			return 'Test Ended';
		default:
			return 'Not Set Up';
	}
}

/** What the state asks of the operator next, in a line under the large words. */
export function modalStateHint(state: ModalRunState | null | undefined, mode: 'impact' | 'shaker' | null | undefined, accept: 'auto' | 'manual' | null): string {
	switch (state) {
		case 'configured':
			return 'Start opens the station; the test begins at its first point.';
		case 'starting':
			return 'The station is opening.';
		case 'idle':
			return mode === 'shaker' ? 'Start Driving to measure this point.' : 'Arm (Space) to take hits at this point.';
		case 'arming':
			return 'The trigger goes live once the pre-trigger has filled: hit when this says Waiting for Hit.';
		case 'armed':
			return accept === 'manual' ? 'Hit the structure. Each hit waits for you to accept or reject it.' : 'Hit the structure. Each hit is judged as it lands; override it with the keys until the next one.';
		case 'captured':
			return 'Accept (Enter or A) or reject (Backspace or R) this hit.';
		case 'measuring':
			return 'The average builds frame by frame.';
		case 'point_done':
			return 'This point has its averages. Move on (Right arrow), or undo (U) to take one again.';
		case 'stopping':
			return 'The drive ramps down and the test ends.';
		case 'finished':
			return 'Open the test in Analysis to fit its FRFs.';
		default:
			return 'Set up the test and apply it.';
	}
}

/** States the shared run bar's chip knows; the Modal Test's own state is its phase. */
export function chipState(state: ModalRunState | null | undefined): 'unconfigured' | 'configured' | 'starting' | 'armed' | 'running' | 'stopping' | 'finished' {
	switch (state) {
		case 'configured':
		case 'starting':
		case 'armed':
		case 'stopping':
		case 'finished':
			return state;
		case 'arming':
			return 'armed';
		case 'idle':
		case 'captured':
		case 'measuring':
		case 'point_done':
			return 'running';
		default:
			return 'unconfigured';
	}
}

export const MODAL_CHIP_WORDS: Record<string, string> = {
	unconfigured: 'Not Set Up',
	configured: 'Configured',
	starting: 'Starting',
	armed: 'Armed',
	running: 'Live',
	stopping: 'Ending',
	finished: 'Ended'
};

/** The phase line of the run bar: the Modal Test's own state, with the drive's phase in shaker mode. */
export const MODAL_PHASE_WORDS: Record<string, string> = {
	none: 'Not set up',
	configured: 'Configured',
	starting: 'Starting',
	idle: 'Ready',
	arming: 'Arming',
	armed: 'Waiting for hit',
	captured: 'Captured',
	measuring: 'Driving',
	stopping: 'Ending',
	point_done: 'Point done',
	finished: 'Ended'
};

export const MODAL_DRIVE_PHASE_WORDS: Record<ModalDrivePhase, string> = {
	idle: 'No drive',
	waiting: 'Waiting',
	ramping: 'Ramping up',
	driving: 'Driving',
	stopping: 'Ramping down',
	zeroed: 'At zero'
};

export const MODAL_OUTCOME_WORDS: Record<ModalOutcome, string> = {
	ended: 'Test Ended',
	aborted: 'Aborted',
	response_abort: 'Response Abort',
	drive_underrun: 'The Drive Was Not Made in Time',
	drive_ceiling: 'Drive Ceiling',
	source_failed: 'The Drive Source Failed',
	input_overload: 'Input Overload',
	loop_faulted: 'Control Loop Fault',
	loop_panicked: 'Control Loop Failure',
	run_panicked: 'The Test Stopped on an Internal Error',
	run_not_started: 'The Test Could Not Start',
	arm_refused: 'Start Refused'
};

/** What an outcome that is no one's choice means for the person at the structure, under its title. */
export const MODAL_OUTCOME_HELP: Partial<Record<ModalOutcome, string>> = {
	source_failed: 'The shaker drive stopped being made, so the drive went to zero on the next sample with the emergency stop. Every point measured before it is saved.',
	run_panicked: 'The test stopped on an internal error and the drive went to zero with the emergency stop. Every point measured before it is saved. The message below says what failed.',
	run_not_started: 'The test could not begin, so the station was stopped and closed. Nothing was measured. Start the test again.'
};

/** A refusal of a Measure verb in words: the service's own, except where the screen can say it plainer. */
export function refusalWords(r: { reason?: string | null; message: string }): string {
	if (r.reason === 'ending') return 'The test is ending: nothing new starts after End Test or Abort.';
	return r.message;
}

export const REJECT_WORDS: Record<ModalRejectReason, string> = {
	overload: 'Overload',
	double_hit: 'Double Hit',
	operator: 'Rejected by You'
};

export const SOURCE_WORDS: Record<ModalSource, string> = {
	random: 'Random',
	burst_random: 'Burst Random',
	pseudo_random: 'Pseudo Random',
	periodic_random: 'Periodic Random',
	periodic_chirp: 'Chirp (Periodic)',
	burst_chirp: 'Burst Chirp'
};

/** A signed percent as the flag shows it: "+26%", "-8%". */
export function pct(v: number | null | undefined): string {
	if (v === null || v === undefined || !Number.isFinite(v)) return '-';
	const r = Math.round(v);
	return `${r > 0 ? '+' : r < 0 ? '−' : ''}${Math.abs(r)}%`;
}

/** A number for the screen, at most four significant figures, no trailing zeros. */
export function num(v: number | null | undefined, unit = ''): string {
	if (v === null || v === undefined || !Number.isFinite(v)) return '-';
	const a = Math.abs(v);
	const s = a !== 0 && (a >= 1e5 || a < 1e-3) ? v.toExponential(2) : String(Number(v.toPrecision(4)));
	return unit ? `${s} ${unit}` : s;
}

/** The force target's flag on a hit: High or Low with the service's percent, On Target, or none. */
export function targetFlag(verdict: ModalTargetVerdict | null | undefined, diff: number | null | undefined): { word: string; tone: 'high' | 'low' | 'on' } | null {
	if (!verdict) return null;
	if (verdict === 'high') return { word: `High ${pct(diff)}`, tone: 'high' };
	if (verdict === 'low') return { word: `Low ${pct(diff)}`, tone: 'low' };
	return { word: 'On Target', tone: 'on' };
}

export interface TileWords {
	/** Accepted, Rejected, Waiting, Undone. */
	status: string;
	tone: 'pass' | 'fail' | 'wait' | 'muted';
	/** Why it was rejected, in words, or null. */
	reason: string | null;
	/** Every flag the tile carries, in words: Double Hit, Overload, High +26%, Tip, Window Cuts Pulse. */
	flags: { word: string; kind: 'double' | 'overload' | 'high' | 'low' | 'on' | 'tip' | 'cut' | 'decay' }[];
}

/** One hit tile in words: its status, why it was rejected, and every flag it carries. */
export function tileWords(t: ModalTile): TileWords {
	const status = t.status === 'accepted' ? 'Accepted' : t.status === 'rejected' ? 'Rejected' : t.status === 'undone' ? 'Undone' : 'Waiting';
	const tone = t.status === 'accepted' ? 'pass' : t.status === 'rejected' ? 'fail' : t.status === 'undone' ? 'muted' : 'wait';
	const flags: TileWords['flags'] = [];
	if (t.double_hit) flags.push({ word: 'Double Hit', kind: 'double' });
	if (t.overload) flags.push({ word: 'Overload', kind: 'overload' });
	const tf = targetFlag(t.target_verdict, t.target_diff_pct);
	if (tf && tf.tone !== 'on') flags.push({ word: tf.word, kind: tf.tone });
	if (tf && tf.tone === 'on') flags.push({ word: tf.word, kind: 'on' });
	if (t.tip_warning) flags.push({ word: 'Tip', kind: 'tip' });
	if (t.force_window_cut) flags.push({ word: 'Window Cuts Pulse', kind: 'cut' });
	if (t.decay?.warning) flags.push({ word: 'Not Died Away', kind: 'decay' });
	return { status, tone, reason: t.status === 'rejected' && t.reason ? REJECT_WORDS[t.reason] : null, flags };
}

/** The latest hit's verdict in large words: "Hit 3 Accepted", "Hit 4 Rejected: Double Hit". */
export function verdictWords(l: ModalLatest | null | undefined, unit: 'hits' | 'frames' = 'hits'): string | null {
	if (!l) return null;
	const what = `${unit === 'frames' ? 'Frame' : 'Hit'} ${l.n}`;
	const t = tileWords(l);
	if (l.status === 'rejected') return `${what} Rejected${t.reason ? `: ${t.reason}` : ''}`;
	if (l.status === 'pending') return `${what} Captured: Accept or Reject`;
	return `${what} ${t.status}`;
}

/** The counter's words, every number the service's: "3 of 5", "1 rejected", the dataset's totals. */
export function counterWords(c: ModalCounts | null | undefined): { point: string; rejected: string; dataset: string; points: string } | null {
	if (!c) return null;
	const u = c.unit === 'frames' ? 'frames' : 'hits';
	return {
		point: `${c.point.accepted} of ${c.point.asked}`,
		rejected: `${c.point.rejected} rejected`,
		dataset: `${c.totals.accepted} accepted ${u} in the dataset`,
		points: `${c.totals.points_measured} of ${c.totals.points_planned} points measured`
	};
}

/** The warnings of a hit's report in words, as the service names them. */
export const HIT_WARNING_WORDS: Record<string, string> = {
	force_window_cuts_pulse: 'The force window ends before the pulse has fallen: widen the window.',
	force_high: 'The force is above the target.',
	force_low: 'The force is below the target.',
	tip_roll_off: 'The force spectrum falls more than the tip limit across the band: a harder tip reaches higher.',
	no_response: 'A response stayed inside its noise: check the sensor and its cable.',
	response_not_decayed: 'A response has not died away by the end of the block: a longer block or an exponential window helps.'
};

/**
 * A shaker frame's decay check in words, with the service's row and percent: short enough for the
 * Response plot's header line, with Not Died Away when the service warns, as a hit's tile says it,
 * and the whole sentence a hit's report gives.
 */
export function frameDecayWords(d: ModalDecay | null | undefined): { flag: string | null; text: string; full: string } | null {
	if (!d) return null;
	const pct = num(d.ratio * 100);
	const text = `Row ${d.row}, ${pct}%`;
	return d.warning
		? { flag: 'Not Died Away', text, full: `Row ${d.row} has not died away by the end of the frame: it ends at ${pct}% of its loudest.` }
		: { flag: null, text, full: `Row ${d.row} ends the frame at ${pct}% of its loudest.` };
}

/** A hit report's checks as lines of words with the service's numbers. */
export function reportLines(r: ModalHitReport | null | undefined, unit: string): string[] {
	if (!r) return [];
	const out: string[] = [`Peak force ${num(r.peak, unit)}.`];
	if (r.double_hit) out.push(`Double hit: a second impact of ${num(r.double_hit.peak, unit)}, ${num(r.double_hit.ratio * 100)}% of the peak.`);
	if (r.overloaded.length) out.push(`Overload on ${r.overloaded.map((k) => (k === 0 ? 'the force' : `response ${k}`)).join(', ')}.`);
	out.push(r.tip_drop_db === null ? 'The force spectrum is zero at the band top.' : `Force spectrum falls ${num(r.tip_drop_db)} dB across the band${r.tip_warning ? ', more than the tip limit' : ''}.`);
	if (r.no_response.some((x) => x === true)) out.push(HIT_WARNING_WORDS.no_response);
	if (r.force_window_cut) out.push(HIT_WARNING_WORDS.force_window_cuts_pulse);
	if (r.decay?.warning) out.push(`Row ${r.decay.row} has not died away by the end of the block: it ends at ${num(r.decay.ratio * 100)}% of its loudest. A longer block or an exponential window helps.`);
	return out;
}
