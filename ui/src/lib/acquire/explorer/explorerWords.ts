/**
 * Plain words for an Explorer run: what the tone is doing ("Quiet: the tone is at 0 V", "Rising to
 * 0.200 V pk", "Holding 251.3 Hz at 0.200 V pk", "Sweeping up at 4 oct/min", "Parked at the top of
 * the band, 2000 Hz", "Stopping: the drive is ramping down"), how a run ended and what tripped it,
 * the steering answer, and the numbers in their units. Display only: every number is the
 * service's, and nothing here computes one beyond placing two of them side by side.
 */

import type {
	ExplorerEnd,
	ExplorerLiveHead,
	ExplorerOutcome,
	ExplorerPhase,
	ExplorerRunState,
	ExplorerState,
	ExplorerStopCause,
	ExplorerSweep,
	ExplorerTrip
} from '$lib/protocol/explorer';
import type { Status } from '$lib/protocol/types';
import { formatNumber } from '$lib/viz/format';
import type { LevelStatus } from '../words';

export const EXPLORER_STATE_WORDS: Record<ExplorerRunState, string> = {
	unconfigured: 'Not Set Up',
	configured: 'Ready to Start',
	starting: 'Starting',
	running: 'Running',
	stopping: 'Stopping',
	finished: 'Finished'
};

export const EXPLORER_PHASE_WORDS: Record<ExplorerPhase, string> = {
	idle: 'Idle',
	priming: 'The station is opening; nothing is driven',
	quiet: 'Quiet: the tone is at 0 V',
	ramping: 'The level is moving',
	sweeping: 'Sweeping',
	at_edge: 'Parked at the band edge',
	steady: 'Holding',
	ramping_down: 'Ramping down',
	stopped: 'Stopped',
	aborted: 'Aborted',
	faulted: 'Stopped on a fault'
};

export const EXPLORER_OUTCOME_WORDS: Record<ExplorerOutcome, string> = {
	operator_stop: 'Stopped by the Operator',
	safe_stop: 'Stopped: Nobody Held Control',
	limit_stop: 'Stopped: Motion Limit',
	operator_abort: 'Aborted by the Operator',
	aborted: 'Aborted by a Protection',
	fault: 'Stopped on a Fault',
	arm_refused: 'The Station Refused to Arm'
};

/** Only the operator's own stop reads as a plain end; every other outcome reads as an abort. */
export function explorerOutcomeTone(outcome: ExplorerOutcome): Status {
	return outcome === 'operator_stop' ? 'info' : 'fail';
}

export const EXPLORER_STOP_WORDS: Record<ExplorerStopCause, string> = {
	operator: 'The operator stopped the run',
	safe_stop: 'Nobody held control',
	shutdown: 'The control service is shutting down',
	limit: 'The derived motion passed its limit'
};

export const EXPLORER_TRIP_WORDS: Record<ExplorerTrip['kind'], string> = {
	watchdog: 'Input Abort Limit',
	drive_vpk: 'Drive Over the Ceiling',
	limit: 'Motion Limit'
};

const AXIS_UNIT: Record<string, string> = { accel: 'g', velocity: 'in/s', displacement: 'mil' };
const AXIS_WORD: Record<string, string> = { accel: 'Acceleration', velocity: 'Velocity', displacement: 'Displacement' };

const num = (v: number, sig = 4) => formatNumber(v, sig);

/** A frequency to a tenth of a hertz: "251.3 Hz". */
export function hz(v: number | null | undefined): string {
	return v === null || v === undefined || !Number.isFinite(v) ? '-' : `${v.toFixed(1)} Hz`;
}

/** A drive level in volts peak to the millivolt: "0.200 V pk". */
export function vpk(v: number | null | undefined): string {
	return v === null || v === undefined || !Number.isFinite(v) ? '-' : `${v.toFixed(3)} V pk`;
}

/** A sweep rate: "4 oct/min". */
export function octPerMin(v: number | null | undefined): string {
	return v === null || v === undefined || !Number.isFinite(v) ? '-' : `${num(v, 3)} oct/min`;
}

/** A step in octaves as the operator reads it: "1/6 oct", "1/96 oct", "0.25 oct". */
export function octaveStep(oct: number): string {
	if (!Number.isFinite(oct) || oct <= 0) return '-';
	const inv = 1 / oct;
	if (Math.abs(inv - Math.round(inv)) < 1e-6 && Math.round(inv) > 1) return `1/${Math.round(inv)} oct`;
	return `${num(oct, 3)} oct`;
}

/** A step in dB: "0.5 dB". */
export function dbStep(v: number): string {
	return Number.isFinite(v) ? `${num(v, 3)} dB` : '-';
}

/** The sweep in words: "Up at 4 oct/min", "Parked", "Halted"; null without one. */
export function sweepWords(sweep: ExplorerSweep | null | undefined, frequency?: number | null, band?: readonly [number, number] | null): string | null {
	if (!sweep) return null;
	const dir = sweep.direction === 'up' ? 'Up' : 'Down';
	if (!sweep.halted) return `${dir} at ${octPerMin(sweep.rate_oct_per_min)}`;
	const edge = band ? (sweep.direction === 'up' ? band[1] : band[0]) : null;
	if (edge !== null && frequency !== null && frequency !== undefined && Math.abs(frequency - edge) < 1e-6 * Math.max(1, edge)) return `Parked at the ${sweep.direction === 'up' ? 'top' : 'bottom'} of the band`;
	return 'Halted by hand';
}

/** What the level card reads: the state, with the live frame laid over it. */
export type ExplorerCardRun = Pick<ExplorerState, 'state' | 'phase' | 'frequency_hz' | 'target_hz' | 'drive_vpk' | 'target_vpk' | 'sweep' | 'band_hz' | 'stall' | 'end'>;

/** The level card's status line for an Explorer run. */
export function explorerLevelStatus(s: ExplorerCardRun | null): LevelStatus {
	const state = s?.state ?? 'unconfigured';
	switch (state) {
		case 'unconfigured':
			return { mark: 'idle', text: 'Not set up', tone: 'info' };
		case 'configured':
			return { mark: 'idle', text: 'Set up, ready to start', tone: 'info' };
		case 'starting':
			return { mark: 'armed', text: 'Starting: the station is opening, nothing driven', tone: 'info' };
		case 'stopping':
			return { mark: 'stop', text: 'Stopping: the drive is ramping down', tone: 'info' };
		case 'finished': {
			const outcome = s?.end?.outcome;
			if (!outcome) return { mark: 'ended', text: 'Finished', tone: 'info' };
			const tone = explorerOutcomeTone(outcome);
			return { mark: 'ended', text: EXPLORER_OUTCOME_WORDS[outcome], tone };
		}
		case 'running':
			break;
	}
	if (s?.stall) return { mark: 'hold', text: `The service's loop has missed its periods for ${num(s.stall.stale_s, 3)} s; Abort stays open`, tone: 'warn' };
	const phase = s?.phase;
	const target = s?.target_vpk ?? null;
	const drive = s?.drive_vpk ?? null;
	switch (phase) {
		case 'quiet':
			return { mark: 'idle', text: 'Quiet: the tone is at 0 V', tone: 'info' };
		case 'ramping':
			if (target === 0) return { mark: 'ramp', text: 'Falling to 0 V', tone: 'info' };
			if (target !== null && drive !== null && target < drive) return { mark: 'ramp', text: `Falling to ${vpk(target)}`, tone: 'info' };
			return { mark: 'ramp', text: `Rising to ${vpk(target)}`, tone: 'info' };
		case 'sweeping':
			return { mark: 'control', text: `Sweeping ${s?.sweep?.direction === 'down' ? 'down' : 'up'} at ${octPerMin(s?.sweep?.rate_oct_per_min)}`, tone: 'pass' };
		case 'at_edge': {
			const up = s?.sweep?.direction !== 'down';
			const edge = s?.band_hz ? (up ? s.band_hz[1] : s.band_hz[0]) : (s?.frequency_hz ?? null);
			return { mark: 'hold', text: `Parked at the ${up ? 'top' : 'bottom'} of the band, ${num(edge ?? NaN, 5)} Hz`, tone: 'pass' };
		}
		default:
			return { mark: 'control', text: `Holding ${hz(s?.frequency_hz)} at ${vpk(drive)}`, tone: 'pass' };
	}
}

/** The trip's facts as labeled values. */
export function explorerTripFacts(trip: ExplorerTrip): { label: string; value: string }[] {
	const at = { label: 'At', value: hz(trip.at_hz) };
	switch (trip.kind) {
		case 'watchdog':
		case 'limit': {
			const unit = AXIS_UNIT[trip.axis] ?? '';
			return [at, { label: 'Channel', value: String(trip.channel) }, { label: AXIS_WORD[trip.axis] ?? trip.axis, value: `${num(trip.value)} ${unit}`.trim() }, { label: 'Limit', value: `${num(trip.limit)} ${unit}`.trim() }];
		}
		case 'drive_vpk':
			return [at, { label: 'Sample', value: vpk(trip.vpk) }, { label: 'Ceiling', value: vpk(trip.ceiling_vpk) }];
	}
}

/** How the run ended, in a line beside the outcome: the tone when the end began and the ramp. */
export function endWords(end: ExplorerEnd): string | null {
	const parts: string[] = [];
	if (end.at_hz !== null && end.vpk !== null) parts.push(`The tone was at ${hz(end.at_hz)} and ${vpk(end.vpk)}`);
	if (end.ramp_s !== null) parts.push(`the drive ramped down in ${num(end.ramp_s, 4)} s of station time`);
	if (!parts.length) return null;
	const s = parts.join('; ');
	return `${s.charAt(0).toUpperCase()}${s.slice(1)}.`;
}

/** What a steer was answered with, in words: "Answered 2000.0 Hz, clamped to the band; 0.200 V pk". */
export function answerWords(a: { hz?: number; vpk?: number; clamped?: { hz?: boolean; vpk?: boolean } | boolean; rate_oct_per_min?: number; direction?: string }, parts: { hz: boolean; vpk: boolean }): string {
	if (a.rate_oct_per_min !== undefined) {
		const clamped = a.clamped === true;
		return `Sweeping ${a.direction === 'down' ? 'down' : 'up'} at ${octPerMin(a.rate_oct_per_min)}${clamped ? ', clamped to the fastest sweep' : ''}`;
	}
	const c = typeof a.clamped === 'object' ? a.clamped : {};
	const out: string[] = [];
	if (parts.hz && a.hz !== undefined) out.push(`${hz(a.hz)}${c.hz ? ', clamped to the band' : ''}`);
	if (parts.vpk && a.vpk !== undefined) out.push(`${vpk(a.vpk)}${c.vpk ? ', clamped to the ceiling' : ''}`);
	if (!out.length && a.hz !== undefined) out.push(`holding ${hz(a.hz)}`);
	return `Answered ${out.join('; ')}`;
}

/** The control row's motion in a line, or null when the service sends none. */
export function motionWords(c: ExplorerLiveHead['control']): string | null {
	if (!c || c.velocity_ips === null || c.displacement_mil === null) return null;
	return `${num(c.velocity_ips, 3)} in/s, ${num(c.displacement_mil, 3)} mil${c.settling ? ' (settling)' : ''}`;
}

/** A refusal's state fact in Explorer's words. */
export function explorerStateFact(detail: Record<string, unknown> | undefined): string | null {
	if (!detail || typeof detail.state !== 'string') return null;
	const st = EXPLORER_STATE_WORDS[detail.state as ExplorerRunState] ?? detail.state;
	const ph = typeof detail.phase === 'string' ? (EXPLORER_PHASE_WORDS[detail.phase as ExplorerPhase] ?? detail.phase) : null;
	return `State: ${st}${ph ? `, ${ph}` : ''}`;
}
