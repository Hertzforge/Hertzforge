/**
 * Playback in words: its states, phases and outcomes for the run bar and the end line, the run's
 * progress, and the drive file's check as the operator reads it. Every number is the control
 * service's; these only say it.
 */

import type { PlaybackCheckChannel, PlaybackEnd, PlaybackOutcome, PlaybackPhase, PlaybackProgress, PlaybackRunState } from '$lib/protocol/playback';
import { formatNumber } from '$lib/viz/format';

export const PLAYBACK_STATE_WORDS: Record<PlaybackRunState, string> = {
	unconfigured: 'Not Set Up',
	configured: 'Ready to Start',
	starting: 'Starting',
	running: 'Playing',
	stopping: 'Stopping',
	finished: 'Finished'
};

export const PLAYBACK_PHASE_WORDS: Record<PlaybackPhase, string> = {
	idle: 'Idle',
	reading_ahead: 'Reading the drive file ahead',
	pre_roll: 'Pre-roll: recording at zero drive',
	playing: 'Playing the drive file',
	post_roll: 'Post-roll: recording at zero drive',
	done: 'Done',
	stopping: 'Stopping: the drive ramps down',
	stopped: 'Stopped',
	zeroed: 'The drive went to zero at once',
	refused: 'The start was refused'
};

export const PLAYBACK_OUTCOME_WORDS: Record<PlaybackOutcome, string> = {
	completed: 'Completed',
	stopped: 'Stopped by the Operator',
	aborted: 'Aborted',
	response_abort: 'Aborted: A Response Passed Its Limit',
	drive_ceiling: 'Aborted: The Drive Passed Its Ceiling',
	input_overload: 'Aborted: An Input Overloaded',
	drive_underrun: 'Aborted: The Drive File Was Not Read in Time',
	reader_failed: 'Aborted: The Drive File Could Not Be Read',
	loop_faulted: 'Aborted: The Control Loop Faulted',
	loop_panicked: 'Aborted: The Control Loop Failed',
	arm_refused: 'The Start Was Refused'
};

const n = (v: number, sig = 4) => formatNumber(v, sig);

export function secs(v: number | null | undefined): string {
	return v === null || v === undefined || !Number.isFinite(v) ? '-' : `${v.toFixed(2)} s`;
}

/** Volts, to four figures. */
export function volts(v: number | null | undefined): string {
	return v === null || v === undefined || !Number.isFinite(v) ? '-' : `${n(v)} V`;
}

/** A value with its unit, to three figures, in exponent form when small. */
export function sci(v: number, unit: string): string {
	const a = Math.abs(v);
	const text = a !== 0 && (a < 1e-3 || a >= 1e5) ? v.toExponential(2) : n(v, 3);
	return `${text} ${unit}`;
}

/** The play's progress, as the run card and the header read it. */
export function progressWords(p: PlaybackProgress | null | undefined, rate: number | null | undefined): string {
	if (!p) return '-';
	const total = rate ? p.samples / rate : null;
	const pct = Math.round(p.fraction * 100);
	return `${secs(p.played_s)} of ${secs(total)} played (${pct} %), ${secs(p.buffered_s)} read ahead`;
}

/** What the run is doing, in the card's words. */
export function playbackStatus(state: PlaybackRunState | null, phase: PlaybackPhase | null, end: PlaybackEnd | null): string {
	if (!state || state === 'unconfigured') return 'Not set up';
	if (state === 'configured') return 'Set up, ready to start';
	if (state === 'finished') return end ? PLAYBACK_OUTCOME_WORDS[end.outcome] : 'Finished';
	return phase ? PLAYBACK_PHASE_WORDS[phase] : PLAYBACK_STATE_WORDS[state];
}

/** A check row's residual against its peak, in the drive's own integrated unit. */
export function residual(r: { end: number; peak: number }, unit: string): string {
	const share = r.peak > 0 ? ` (${n((100 * Math.abs(r.end)) / r.peak, 2)} % of peak)` : '';
	return `${sci(r.end, unit)} of ${sci(r.peak, unit)}${share}`;
}

/** A channel's verdict: Pass, or each fault's own words, one after another. */
export function verdict(c: Pick<PlaybackCheckChannel, 'pass' | 'note' | 'faults'>): string {
	if (c.pass) return c.note ? `Pass: ${c.note}` : 'Pass';
	const text = c.faults.map((f) => f.text.trim().replace(/\.$/, '')).join('; ');
	return text ? `${text.charAt(0).toUpperCase()}${text.slice(1)}.` : 'Refused.';
}

/** Whether a check's refusals are only about the ends, which Fix Ends can make good. */
export function endsOnly(refusals: { reason: string }[]): boolean {
	return refusals.length > 0 && refusals.every((r) => r.reason === 'ends_not_zero');
}
