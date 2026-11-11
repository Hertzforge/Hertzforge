/**
 * Plain words for a Mixed Mode run: what the run is doing ("Floor alone at -12.0 dB", "Every part
 * at full level, 0.0 dB"), what each part is doing ("Sweeping up at 1 oct/min, sweep 2 of 4"), a
 * part's measured value against its target, how a run ended and on which part it tripped. Display
 * only: every number is the service's, and nothing here computes one beyond placing two of them side
 * by side.
 */

import type { MixedConfig, MixedDerived, MixedLawFaultReason, MixedLiveHead, MixedOutcome, MixedPart, MixedPartKind, MixedPartRow, MixedPhase, MixedSchedule, MixedState, MixedTrip } from '$lib/protocol/mixed';
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

/**
 * What the parts table reads beside each row: every part's row now, the configuration the run holds
 * and parts time. The row's own fields win where it carries them; the setup fills in what a row
 * leaves out (a follower's leader and ratio, a sweep's stated rate, a part's window).
 */
export interface PartContext {
	rows?: readonly MixedPartRow[];
	config?: MixedConfig | null;
	parts_s?: number | null;
}

const usable = (v: number | null | undefined): v is number => typeof v === 'number' && Number.isFinite(v) && v !== 0;

function configPart(ctx: PartContext, id: string): MixedPart | undefined {
	return ctx.config?.parts.find((p) => p.id === id);
}

function scheduleOf(ctx: PartContext, id: string): MixedSchedule | null {
	const p = configPart(ctx, id);
	return p && p.kind !== 'floor' ? p.schedule : null;
}

/** A follower's leader and ratio: the row's own when the service sends them, the setup's otherwise. */
export function partFollows(row: MixedPartRow, ctx: PartContext = {}): { of: string; ratio: number } | null {
	if (row.follows) return row.follows;
	const s = scheduleOf(ctx, row.id);
	return s?.kind === 'follow' ? { of: s.of, ratio: s.ratio } : null;
}

/** The part at the root of a follower's chain and the product of the ratios along it. */
function followRoot(row: MixedPartRow, ctx: PartContext): { id: string; ratio: number } {
	let id = row.id;
	let ratio = 1;
	let at: MixedPartRow | undefined = row;
	for (let n = 0; n < 32; n++) {
		const f: { of: string; ratio: number } | null = at ? partFollows(at, ctx) : (() => {
			const s = scheduleOf(ctx, id);
			return s?.kind === 'follow' ? { of: s.of, ratio: s.ratio } : null;
		})();
		if (!f) break;
		ratio *= f.ratio;
		id = f.of;
		at = ctx.rows?.find((r) => r.id === id);
	}
	return { id, ratio };
}

function nameOf(ctx: PartContext, id: string): string {
	return ctx.rows?.find((r) => r.id === id)?.name ?? configPart(ctx, id)?.name ?? id;
}

/**
 * A sweeping part's rate in the unit its schedule states: the row's field in that unit when it holds
 * a rate, the other field converted at the frequency now when only that one does, and the schedule's
 * stated rate otherwise. Never a zero read from a field the service left empty. Without a schedule,
 * whichever field holds a rate; null when neither does.
 */
export function partRate(row: MixedPartRow, schedule: MixedSchedule | null = null): string | null {
	const f = row.freq_hz;
	const oct = (v: number) => `${num(Math.abs(v), 3)} oct/min`;
	const lin = (v: number) => `${num(Math.abs(v), 3)} Hz/s`;
	const law = schedule?.kind === 'sweep' ? schedule.rate.law : null;
	if (law === 'log' || (law === null && usable(row.rate_oct_per_min))) {
		if (usable(row.rate_oct_per_min)) return oct(row.rate_oct_per_min);
		if (usable(row.rate_hz_per_s) && usable(f)) return oct((row.rate_hz_per_s / (f * Math.LN2)) * 60);
		return schedule?.kind === 'sweep' && schedule.rate.law === 'log' ? oct(schedule.rate.oct_per_min) : null;
	}
	if (usable(row.rate_hz_per_s)) return lin(row.rate_hz_per_s);
	if (law === 'linear' && usable(row.rate_oct_per_min) && usable(f)) return lin((row.rate_oct_per_min * f * Math.LN2) / 60);
	return schedule?.kind === 'sweep' && schedule.rate.law === 'linear' ? lin(schedule.rate.hz_per_s) : null;
}

/** A follower's motion from its root: "sweeping up at 0.3 oct/min" (a log rate is the same at every ratio; a linear one scales). */
function rootMotion(row: MixedPartRow, ctx: PartContext): string | null {
	const root = followRoot(row, ctx);
	const r = ctx.rows?.find((x) => x.id === root.id);
	if (!r?.direction) return null;
	const s = scheduleOf(ctx, root.id);
	if (s?.kind === 'sweep' && s.rate.law === 'linear') {
		const own = usable(r.rate_hz_per_s) ? r.rate_hz_per_s : s.rate.hz_per_s;
		return `sweeping ${r.direction} at ${num(Math.abs(own * root.ratio), 3)} Hz/s`;
	}
	const rate = partRate(r, s);
	return `sweeping ${r.direction}${rate ? ` at ${rate}` : ''}`;
}

/** A part's frequency now: the row's, or for a follower whose row has none, its leader's times its ratio. */
export function partFreq(row: MixedPartRow, ctx: PartContext = {}): number | null {
	if (row.kind === 'floor') return null;
	if (row.freq_hz !== null) return row.freq_hz;
	const f = partFollows(row, ctx);
	if (!f) return null;
	const lead = ctx.rows?.find((r) => r.id === f.of);
	const lf = lead ? partFreq(lead, ctx) : null;
	return lf === null ? null : lf * f.ratio;
}

/** Whether a part that is not playing has ended, and how: its window closed, or its schedule ran out. */
function partEnd(row: MixedPartRow, ctx: PartContext): { why: 'window' | 'schedule'; at_s?: number } | 'waiting' | null {
	if (row.on || !row.enabled) return null;
	const p = configPart(ctx, row.id);
	const w = p?.window;
	const t = ctx.parts_s;
	if (typeof t === 'number' && typeof w?.off_s === 'number' && t >= w.off_s) return { why: 'window', at_s: w.off_s };
	if (row.done) return { why: 'schedule' };
	if (row.sweeps && row.sweeps.of !== null && row.sweeps.done >= row.sweeps.of) return { why: 'schedule' };
	if (typeof t === 'number' && typeof w?.on_s === 'number' && t < w.on_s) return 'waiting';
	if (typeof t === 'number' && p) return { why: 'schedule' };
	return 'waiting';
}

/** What a part is doing now, in words: "Sweeping up at 1 oct/min, sweep 2 of 4", "Follows Rotor at 2×", "Standing", "Off". */
export function partMotion(row: MixedPartRow, held: boolean, ctx: PartContext = {}): string {
	if (row.kind === 'floor') return row.settled ? 'Equalized' : 'Equalizing';
	if (!row.on) {
		if (!row.enabled) return 'Off';
		const end = partEnd(row, ctx);
		if (end === 'waiting' || end === null) {
			const on = configPart(ctx, row.id)?.window?.on_s;
			return typeof on === 'number' && on > 0 ? `Waiting for its window, from ${num(on, 4)} s` : 'Waiting for its window';
		}
		return end.why === 'window' ? `Its window closed at ${num(end.at_s ?? 0, 4)} s` : 'Its schedule ended';
	}
	if (row.scale < 0.999) return row.enabled ? 'Ramping in' : 'Ramping out';
	if (held) return 'Held';
	const follows = partFollows(row, ctx);
	if (follows) {
		const moving = rootMotion(row, ctx);
		return `Follows ${nameOf(ctx, follows.of)} at ${num(follows.ratio, 4)}×${moving ? `, ${moving}` : ''}`;
	}
	const rate = partRate(row, scheduleOf(ctx, row.id));
	const sw = row.sweeps;
	const count = sw ? `, sweep ${sw.done + 1}${sw.of !== null ? ` of ${sw.of}` : ''}` : '';
	if (row.direction) return `Sweeping ${row.direction}${rate ? ` at ${rate}` : ''}${count}`;
	return 'Standing';
}

/** The state column's word for a part that is not playing: "Off", "Waiting" or "Ended"; null while it plays. */
export function partStateWord(row: MixedPartRow, ctx: PartContext = {}): 'Off' | 'Waiting' | 'Ended' | null {
	if (row.kind === 'floor' || row.on) return null;
	if (!row.enabled) return 'Off';
	const end = partEnd(row, ctx);
	return end === null || end === 'waiting' ? 'Waiting' : 'Ended';
}

/** Degrees of freedom in words, only as the row carries them. */
function dofWords(dof: number | null, floor: boolean): string {
	if (dof === null || !Number.isFinite(dof)) return '';
	if (dof <= 0) return floor ? 'no clean frame averaged yet' : 'no frame averaged yet';
	// The floor's figure is the fewest of any clean line (lines differ by how many clean frames each has).
	return floor ? `at least ${num(dof, 3)} degrees of freedom on every clean line` : `${num(dof, 3)} degrees of freedom`;
}

/** The line under a part's motion: the floor's lines and degrees of freedom, a narrowband's width, estimate and gain, a law's misses. */
export function partDetail(row: MixedPartRow, ctx: PartContext = {}): string {
	const law = row.law && (row.law.misses || row.law.capped) ? `${row.law.misses} unusable in a row, a cap bound ${row.law.capped} times` : '';
	if (row.kind === 'tone') return law;
	if (row.kind === 'floor') {
		const l = row.lines;
		const lines = l ? [`${l.clean} of ${l.in_band} lines clean, ${l.alarm} in alarm, ${l.abort} over abort`, dofWords(row.dof, true)].filter(Boolean).join('; ') : '';
		return [lines, law].filter(Boolean).join('; ');
	}
	const p = configPart(ctx, row.id);
	const share = p?.kind === 'narrowband' && typeof p.width_share === 'number' ? `, ${num(p.width_share * 100, 3)}% of its center` : '';
	return [row.width_hz !== null ? `${num(row.width_hz, 3)} Hz wide${share}` : '', dofWords(row.dof, false), row.gain_db !== null ? `gain ${errDb(row.gain_db)}` : ''].filter(Boolean).join(', ');
}

/**
 * A part's drive: a tone's V peak, the floor's or a narrowband's V rms. A narrowband that plays with
 * a drive of exactly 0 has had no share of the random part reported yet, so it shows none.
 */
export function partDrive(row: MixedPartRow): string {
	if (row.drive_vpk !== null) return `${num(row.drive_vpk, 3)} V pk`;
	if (row.drive_vrms === null) return '-';
	if (row.kind === 'narrowband' && row.on && row.drive_vrms === 0) return '-';
	return `${num(row.drive_vrms, 3)} V rms`;
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
		case 'band_abort': {
			// Only what the service reported: a fact it left null is not printed.
			const unit = trip.kind === 'abort_band' ? 'g' : 'g²/Hz';
			const measured = trip.kind === 'abort_band' ? trip.measured_g : trip.measured_g2_hz;
			const limit = trip.kind === 'abort_band' ? trip.limit_g : trip.limit_g2_hz;
			const out = [...at, { label: 'Side', value: trip.side === 'high' ? 'Over its target' : 'Under its target' }];
			if (typeof trip.error_db === 'number') out.push({ label: 'Error', value: errDb(trip.error_db) });
			if (typeof measured === 'number') out.push({ label: 'Measured', value: `${num(measured)} ${unit}` });
			if (typeof limit === 'number') out.push({ label: trip.side === 'high' ? 'Upper abort' : 'Lower abort', value: `${num(limit)} ${unit}` });
			if (trip.kind === 'band_abort' && typeof trip.dof === 'number') out.push({ label: 'Degrees of freedom', value: String(trip.dof) });
			return out;
		}
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
