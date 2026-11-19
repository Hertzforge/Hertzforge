/**
 * Plain words for a Recorder run: what it is doing ("Monitoring: nothing is being saved", "Armed:
 * waiting for row 1 to rise through 0.250 g (pre-window 0.50 s of 0.50 s)", "Recording rec_0002:
 * 0.42 s of 1.00 s"), what it has saved ("Saved rec_0002: 1.50 s, no gaps"), how far the disk is
 * behind, what a write fault left, how the run ended and where its recordings are, and the numbers
 * in their units. Display only: every number is the service's, and nothing here computes one
 * beyond placing two of them side by side or turning frames into seconds at the service's rate.
 */

import type {
	RecorderEnd,
	RecorderMark,
	RecorderOutcome,
	RecorderPhase,
	RecorderRecording,
	RecorderRunState,
	RecorderSlope,
	RecorderState,
	RecorderStopCause,
	RecorderTriggerMode
} from '$lib/protocol/recorder';
import type { Status } from '$lib/protocol/types';
import { formatNumber } from '$lib/viz/format';
import type { LevelStatus } from '../words';

export const RECORDER_STATE_WORDS: Record<RecorderRunState, string> = {
	unconfigured: 'Not Set Up',
	configured: 'Ready to Start',
	starting: 'Starting',
	running: 'Running',
	stopping: 'Stopping',
	finished: 'Finished'
};

export const RECORDER_PHASE_WORDS: Record<RecorderPhase, string> = {
	idle: 'Idle',
	opening: 'The station is opening; nothing is recorded',
	monitoring: 'Monitoring: nothing is being saved',
	armed: 'Armed',
	recording: 'Recording',
	closing: 'Finishing the recordings',
	stopped: 'Stopped',
	aborted: 'Aborted',
	faulted: 'Stopped on a fault'
};

export const RECORDER_OUTCOME_WORDS: Record<RecorderOutcome, string> = {
	operator_stop: 'Stopped by the Operator',
	safe_stop: 'Stopped: Nobody Held Control',
	operator_abort: 'Aborted by the Operator',
	write_failed: 'Stopped: A Write Failed',
	write_behind: 'Stopped: The Disk Fell Behind',
	fault: 'Stopped on a Fault',
	arm_refused: 'The Station Refused to Start'
};

export const RECORDER_STOP_WORDS: Record<RecorderStopCause, string> = {
	operator: 'The operator stopped the run',
	safe_stop: 'Nobody held control',
	shutdown: 'The control service is shutting down'
};

export const RECORDER_MODE_WORDS: Record<RecorderTriggerMode, string> = {
	start: 'From the start',
	manual: 'Manual',
	level: 'Level'
};

/** Only the operator's own stop reads as a plain end; every other outcome reads as an abort. */
export function recorderOutcomeTone(outcome: RecorderOutcome): Status {
	return outcome === 'operator_stop' ? 'info' : 'fail';
}

const num = (v: number, sig = 4) => formatNumber(v, sig);
const isNum = (v: number | null | undefined): v is number => v !== null && v !== undefined && Number.isFinite(v);

/** Seconds to a fixed number of places: "0.42 s". */
export function secs(v: number | null | undefined, places = 2): string {
	return isNum(v) ? `${v.toFixed(places)} s` : '-';
}

/** A rate: "12800 Hz", "8533.33 Hz". */
export function rateWords(v: number | null | undefined): string {
	return isNum(v) ? `${num(v, 6)} Hz` : '-';
}

/** A level in a row's unit, to the thousandth: "0.250 g". */
export function levelWords(v: number | null | undefined, unit: string): string {
	return isNum(v) ? `${v.toFixed(3)} ${unit}`.trim() : '-';
}

/** Bytes in binary units: "1.00 GiB", "512 MiB". */
export function bytesWords(b: number | null | undefined): string {
	if (!isNum(b)) return '-';
	const units = ['bytes', 'kiB', 'MiB', 'GiB', 'TiB'];
	let v = b;
	let i = 0;
	while (Math.abs(v) >= 1024 && i < units.length - 1) {
		v /= 1024;
		i += 1;
	}
	return i === 0 ? `${Math.round(v)} bytes` : `${num(v, 3)} ${units[i]}`;
}

/** A long stretch of time in hours and minutes: "144 h 10 min", "12 min", "40 s". */
export function spanWords(s: number | null | undefined): string {
	if (!isNum(s)) return '-';
	if (s < 60) return `${Math.floor(s)} s`;
	const m = Math.floor(s / 60);
	if (m < 60) return `${m} min`;
	return `${Math.floor(m / 60)} h ${m % 60} min`;
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** What the card reads: the state, with the live frame laid over it. */
export type RecorderCardRun = Pick<RecorderState, 'state' | 'phase' | 'mode' | 'trigger' | 'recording' | 'grid'>;

/** What the card needs beside the run: the trigger's row and slope, from the configuration the run holds. */
export interface RecorderCardSetup {
	row: { index: number; unit: string } | null;
	level: number | null;
	slope: RecorderSlope;
}

/** Frames as seconds at the run's own rate. */
const frameSecs = (frames: number, fs: number | null | undefined) => (isNum(fs) && fs > 0 ? frames / fs : NaN);

/** The pre-window held and asked, in seconds: "pre-window 0.50 s of 0.50 s". */
function preWords(run: RecorderCardRun): string {
	const t = run.trigger;
	const fs = run.grid?.fs_hz;
	if (!t) return '';
	return `pre-window ${secs(frameSecs(t.held_frames, fs))} of ${secs(frameSecs(t.pre_frames, fs))}`;
}

/** The card's status line for a Recorder run. */
export function recorderCardStatus(s: RecorderCardRun | null, setup: RecorderCardSetup | null, end?: RecorderEnd | null): LevelStatus {
	const state = s?.state ?? 'unconfigured';
	switch (state) {
		case 'unconfigured':
			return { mark: 'idle', text: 'Not set up', tone: 'info' };
		case 'configured':
			return { mark: 'idle', text: 'Set up, ready to start', tone: 'info' };
		case 'starting':
			return { mark: 'armed', text: 'Starting: the station is opening, nothing is recorded', tone: 'info' };
		case 'stopping':
			return { mark: 'stop', text: 'Stopping: the recordings are being finished', tone: 'info' };
		case 'finished': {
			const outcome = end?.outcome;
			if (!outcome) return { mark: 'ended', text: 'Finished', tone: 'info' };
			return { mark: 'ended', text: RECORDER_OUTCOME_WORDS[outcome], tone: recorderOutcomeTone(outcome) };
		}
		case 'running':
			break;
	}
	const run = s as RecorderCardRun;
	switch (run.phase) {
		case 'recording': {
			const r = run.recording;
			if (!r) return { mark: 'control', text: 'Recording', tone: 'pass' };
			return { mark: 'control', text: r.length_s === null ? `Recording ${r.name}: ${secs(r.seconds)}, until cut` : `Recording ${r.name}: ${secs(r.seconds)} of ${secs(r.length_s)}`, tone: 'pass' };
		}
		case 'armed': {
			const pre = preWords(run);
			if (run.mode === 'level' && setup?.row) {
				const hold = run.trigger?.holdoff_left_s ?? 0;
				const waits = `waiting for row ${setup.row.index} to ${setup.slope === 'falling' ? 'fall' : 'rise'} through ${levelWords(setup.level, setup.row.unit)}`;
				return { mark: 'armed', text: hold > 0 ? `Armed: holding off ${secs(hold)}, then ${waits} (${pre})` : `Armed: ${waits} (${pre})`, tone: 'info' };
			}
			return { mark: 'armed', text: `Armed: Record begins with the pre-window (${pre})`, tone: 'info' };
		}
		default:
			return { mark: 'hold', text: 'Monitoring: nothing is being saved', tone: 'info' };
	}
}

/** A saved recording in a line: "Saved rec_0002: 1.50 s, no gaps". */
export function savedWords(r: Pick<RecorderRecording, 'name' | 'seconds' | 'gaps' | 'gap_free'>): string {
	const missing = r.gaps.reduce((n, g) => n + g.dropped_frames, 0);
	return `Saved ${r.name}: ${secs(r.seconds)}, ${r.gap_free ? 'no gaps' : `${plural(r.gaps.length, 'gap', 'gaps')}, ${plural(missing, 'frame', 'frames')} missing`}`;
}

/** The disk's backlog, once it is more than a writer's chunk: "Writing 0.3 s behind the inputs". */
export function backlogWords(backlog_s: number | null | undefined, over = 0.25): string | null {
	return isNum(backlog_s) && backlog_s >= over ? `Writing ${backlog_s.toFixed(1)} s behind the inputs` : null;
}

/** A write fault and what it left: "Could not write: <the fault's words>. rec_0003 holds 48.2 s." */
export function faultWords(fault: string, held: Pick<RecorderRecording, 'name' | 'seconds'> | null): string {
	const words = fault.trim().replace(/[.\s]+$/, '');
	return held ? `Could not write: ${words}. ${held.name} holds ${held.seconds.toFixed(1)} s.` : `Could not write: ${words}.`;
}

/** How a run ended and where its recordings are: "Stopped: 6 recordings in <folder>". */
export function endWords(end: RecorderEnd, folder: string | null): string {
	const lead =
		end.outcome === 'operator_abort'
			? 'Aborted'
			: end.outcome === 'operator_stop' || end.outcome === 'safe_stop'
				? 'Stopped'
				: end.outcome === 'arm_refused'
					? 'Not started'
					: 'Ended';
	const saved = plural(end.recordings, 'recording', 'recordings');
	return folder ? `${lead}: ${saved} in ${folder}` : `${lead}: ${saved}`;
}

const AXIS_WORD: Record<string, string> = { accel: 'acceleration', velocity: 'velocity', displacement: 'displacement' };
const AXIS_UNIT: Record<string, string> = { accel: 'g', velocity: 'in/s', displacement: 'mil' };

/** A mark in a line, at its station time: "Row 1 passed its acceleration limit of 0.4 g (0.5 g) at 2.501 s". */
export function markWords(m: RecorderMark, fs: number | null | undefined, unit = ''): string {
	const at = isNum(fs) && fs > 0 ? ` at ${(m.station_frame / fs).toFixed(3)} s` : ` at station frame ${m.station_frame}`;
	const inRec = m.recording ? `, in ${m.recording}` : '';
	if (m.kind === 'limit') {
		const u = AXIS_UNIT[m.axis] ?? '';
		return `Row ${m.row} passed its ${AXIS_WORD[m.axis] ?? m.axis} limit of ${num(m.limit)} ${u} (${num(m.value)} ${u})${at}${inRec}`.replace(/\s+\)/g, ')');
	}
	return `Row ${m.row} reached ${num(m.value)} ${unit}, over 0.9 of its range (${num(m.level)} ${unit})${at}${inRec}`.replace(/\s+\)/g, ')').replace(/\s+,/g, ',');
}

/** A recording's facts as labeled values, in the list. */
export function recordingFacts(r: RecorderRecording): { label: string; value: string }[] {
	const out = [
		{ label: 'Trigger', value: RECORDER_MODE_WORDS[r.trigger] ?? r.trigger },
		{ label: 'Length', value: `${r.frames} frames, ${secs(r.seconds, 3)}` },
		{ label: 'Rate', value: r.fs_hz !== r.requested_fs_hz ? `${rateWords(r.fs_hz)} (asked ${rateWords(r.requested_fs_hz)})` : rateWords(r.fs_hz) },
		{ label: 'Pre-window', value: r.pre_frames < r.pre_frames_asked ? `${r.pre_frames} of ${r.pre_frames_asked} frames` : `${r.pre_frames} frames` },
		{ label: 'Station frames', value: `${r.first_station_frame} first, ${r.trigger_station_frame} trigger` },
		{ label: 'Group delay', value: `${num(r.group_delay_frames, 4)} frames` },
		{ label: 'Gaps', value: r.gap_free ? 'None' : r.gaps.map((g) => `${g.dropped_frames} at ${g.at_frame}`).join(', ') },
		{ label: 'Marks', value: String(r.marks) },
		{ label: 'Size', value: bytesWords(r.bytes) }
	];
	if (r.fault) out.push({ label: 'Fault', value: r.fault });
	return out;
}

/** A refusal's state fact in the Recorder's own words. */
export function recorderStateFact(detail: Record<string, unknown> | undefined): string | null {
	if (!detail || typeof detail.state !== 'string') return null;
	const st = RECORDER_STATE_WORDS[detail.state as RecorderRunState] ?? detail.state;
	const ph = typeof detail.phase === 'string' ? (RECORDER_PHASE_WORDS[detail.phase as RecorderPhase] ?? detail.phase) : null;
	return `State: ${st}${ph ? `, ${ph}` : ''}`;
}

/** A `wrong_mode` refusal's fact: the run's trigger mode. */
export function recorderModeFact(detail: Record<string, unknown> | undefined): string | null {
	if (!detail || typeof detail.mode !== 'string') return null;
	return `Trigger: ${RECORDER_MODE_WORDS[detail.mode as RecorderTriggerMode] ?? detail.mode}`;
}

/** A `not_enough_space` refusal's facts: the free space and the floor. */
export function recorderSpaceFacts(detail: Record<string, unknown> | undefined): string[] {
	if (!detail) return [];
	const out: string[] = [];
	if (typeof detail.free_bytes === 'number') out.push(`Free: ${bytesWords(detail.free_bytes)}`);
	if (typeof detail.floor_bytes === 'number') out.push(`Floor: ${bytesWords(detail.floor_bytes)}`);
	return out;
}
