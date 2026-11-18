/**
 * The MIMO Sine setup as the operator edits it, and its conversion to and from the protocol's
 * `MimoSineConfig` (schema/PROTOCOL.md, "MIMO Sine Configuration"). Pure functions only: the
 * service computes every number about the test (each reference at any frequency, the grid and the
 * tick, the test's length, the tracking check, the identification), and this module only carries
 * the operator's choices.
 *
 * Numbers are held as numbers; a field that holds no number is NaN, which `localErrors` names
 * before anything is sent. The rate is null when the usual one (16384 Hz) is wanted, a sweep's
 * start is null for the band's low edge, the identification's records null for the service's
 * count, the rotation cap null for none and the simulator's structure null for the one of the
 * run's size. Each drive's ceiling and abort have no default anywhere: the setup asks for both.
 *
 * The channels are the channel table's: every enabled control row is a control unless it is taken
 * out of the controls; every enabled drive row is a drive, in the table's order. Each control has
 * its own reference (an amplitude profile with its alarm and abort bands, and a phase against
 * frequency), kept with its control as rows are switched on and off. A limit channel is any
 * enabled input that is not a control, with its bound against frequency.
 *
 * The editor keeps every test kind's settings, so a change of kind loses nothing; the wire carries
 * the chosen one.
 */

import type { ChannelRow, LadderMode, ReportItem } from '$lib/protocol/types';
import type { LawEntry, MimoShape } from '$lib/protocol/mimo';
import type { SineDirection, SineDwellFor, SineRungFor } from '$lib/protocol/sine';
import type { MimoSineConfig, MimoSineLawEntry, MimoSinePhasePoint, MimoSineTest, MimoSineTestType } from '$lib/protocol/mimoSine';
import { lawWireParams, type LawDraft } from '../law/lawForm';
import { defaultRecord, followTitle, shakerRecord, titleNumber, type FieldErrors, type RecordDraft } from '../setup';
import { newRow, profileDraftOf, profileOf, type SineProfileDraft } from '../sine/sineSetup';

// ---------------------------------------------------------------------------------------------
// The draft

export interface MsineControlDraft {
	/** The channel table row's `index`: an enabled control row. */
	channel: number;
	/** The reference amplitude at full level with its alarm and abort bands. */
	profile: SineProfileDraft;
	/** The reference phase against frequency, degrees relative to the tone. */
	phase: MimoSinePhasePoint[];
}

export interface MsineDriveDraft {
	channel: number;
	/** Volts peak; NaN until the operator gives it. */
	ceiling_vpk: number;
	abort_vpk: number;
}

export interface MsineLimitDraft {
	/** An enabled input that is not a control. */
	channel: number;
	/** The bound at each frequency, in the profile's form. */
	profile: SineProfileDraft;
}

export interface MsineShakerDraft {
	/** The drive the shaker is on, by table index. */
	drive: number;
	shaker_text: string;
	moving_mass_lb: { armature: number; slip_table: number; drive_adapter: number; fixturing: number; test_article: number };
}

export type MsineEndBy = 'sweeps' | 'duration' | 'both';

export interface MsineDraft {
	title: string;
	controls: MsineControlDraft[];
	/** Enabled control rows the operator took out of the controls, by table index. */
	notControls: number[];
	drives: MsineDriveDraft[];
	/** True: amplitude and phase per control. False: amplitudes only, the law chooses the phases. */
	phase_specified: boolean;
	test: {
		kind: MimoSineTestType;
		/** The sweep's and the stepped grid's band. */
		band_hz: [number, number];
		sweep: {
			law: 'log' | 'linear';
			oct_per_min: number;
			hz_per_s: number;
			start_hz: number | null;
			direction: SineDirection;
			end_by: MsineEndBy;
			sweeps: number;
			duration_s: number;
		};
		dwell: { freq_hz: number; by: 'seconds' | 'cycles'; seconds: number; cycles: number };
		stepped: {
			on: 'grid' | 'list';
			steps_per_octave: number;
			spacing: 'log' | 'linear';
			/** Played in the order given. */
			frequencies_hz: number[];
			/** `computed` on a grid only. */
			by: 'computed' | 'seconds' | 'cycles';
			seconds: number;
			cycles: number;
		};
	};
	acquisition: { fs_hz: number | null; tick_ms: number };
	schedule: {
		mode: LadderMode;
		levels: number[];
		start_db: number;
		per_level: { by: 'sweeps' | 'seconds'; n: number };
		full_level: { by: 'sweeps' | 'seconds'; n: number };
	};
	identification: { level_db: number; resolution_hz: number; frames: number | null; trust_coherence: number; gate_coherence: number; review: boolean };
	law: LawDraft;
	control: {
		tracking: { kind: 'fixed' | 'proportional'; hz: number; pct: number; max_hz: number };
		law_budget: number;
		law_misses: number;
		max_step_db: number;
		max_rise_db_per_s: number;
		rotation_cap_deg: number | null;
		divergence_s: number;
		abort_ticks: number;
	};
	requests: { honour: boolean; max_hold_s: number; min_slow_factor: number };
	limits: {
		kinematic_monitor: boolean;
		channels: MsineLimitDraft[];
		notch: { smoothing_ticks: number; release_ramp_db_per_s: number; near_bound_db: number };
	};
	feasibility: { enabled: boolean; shakers: MsineShakerDraft[] };
	simulator: { structure: string | null };
	record: RecordDraft;
	metadata: { key: string; label: string; value: string }[];
}

/** The simulator's standard sine structures, by name, with what each is. */
export const STRUCTURES: { id: string; label: string; controls: number; drives: number }[] = [
	{ id: 'sine-linear-4x4', label: 'Linear, 4 controls by 4 drives', controls: 4, drives: 4 },
	{ id: 'sine-softening-4x4-mild', label: 'Softening with level, 4 by 4, mild', controls: 4, drives: 4 },
	{ id: 'sine-softening-4x4-moderate', label: 'Softening with level, 4 by 4, moderate', controls: 4, drives: 4 },
	{ id: 'sine-hardening-4x4', label: 'Hardening with level, 4 by 4', controls: 4, drives: 4 },
	{ id: 'sine-tall-6x3', label: 'Linear, 6 controls by 3 drives', controls: 6, drives: 3 },
	{ id: 'sine-wide-3x6', label: 'Linear, 3 controls by 6 drives', controls: 3, drives: 6 },
	{ id: 'sine-base-article-6x4', label: 'Base and article with a limit channel, 6 by 4', controls: 6, drives: 4 },
	{ id: 'sine-near-singular-4x4', label: 'Near-singular at one frequency, 4 by 4', controls: 4, drives: 4 }
];

export const MAX_CONTROLS = 16;
export const MAX_DRIVES = 8;
export const MAX_LIMITS = 8;

export const TEST_KINDS: { id: MimoSineTestType; label: string }[] = [
	{ id: 'sweep', label: 'Sweep' },
	{ id: 'dwell', label: 'Dwell' },
	{ id: 'stepped', label: 'Stepped Sine' }
];

/** A control's reference: 1 g flat across the band, its phase 0 degrees. */
export function newControl(channel: number, band: readonly [number, number] = [20, 2000]): MsineControlDraft {
	return {
		channel,
		profile: { label: '', rows: [newRow(band[0], 'accel_g', 1), newRow(band[1], 'accel_g', 1)], alarm_db: [1.5, 1.5], abort_db: [3, 3] },
		phase: [{ freq_hz: band[0], phase_deg: 0 }]
	};
}

export function newLimit(channel: number, band: readonly [number, number] = [20, 2000]): MsineLimitDraft {
	return { channel, profile: { label: '', rows: [newRow(band[0], 'accel_g', NaN), newRow(band[1], 'accel_g', NaN)], alarm_db: [1.5, 1.5], abort_db: [3, 3] } };
}

export function newShaker(drive: number): MsineShakerDraft {
	return { drive, shaker_text: '', moving_mass_lb: { armature: 0, slip_table: 0, drive_adapter: 0, fixturing: 0, test_article: 0 } };
}

export function defaultMsineSetup(): MsineDraft {
	return {
		title: '',
		controls: [],
		notControls: [],
		drives: [],
		phase_specified: true,
		test: {
			kind: 'sweep',
			band_hz: [20, 2000],
			sweep: { law: 'log', oct_per_min: 2, hz_per_s: 5, start_hz: null, direction: 'up', end_by: 'sweeps', sweeps: 2, duration_s: 900 },
			dwell: { freq_hz: 100, by: 'seconds', seconds: 60, cycles: 6000 },
			stepped: { on: 'grid', steps_per_octave: 3, spacing: 'log', frequencies_hz: [50, 100, 200], by: 'computed', seconds: 10, cycles: 1000 }
		},
		acquisition: { fs_hz: null, tick_ms: 10 },
		schedule: { mode: 'auto', levels: [-12, -6, 0], start_db: -12, per_level: { by: 'sweeps', n: 1 }, full_level: { by: 'sweeps', n: 2 } },
		identification: { level_db: -20, resolution_hz: 0.25, frames: null, trust_coherence: 0.5, gate_coherence: 0.5, review: true },
		law: { id: '', params: {} },
		control: {
			tracking: { kind: 'proportional', hz: 10, pct: 10, max_hz: 100 },
			law_budget: 0.25,
			law_misses: 3,
			max_step_db: 1,
			max_rise_db_per_s: 240,
			rotation_cap_deg: null,
			divergence_s: 1,
			abort_ticks: 3
		},
		requests: { honour: false, max_hold_s: 2, min_slow_factor: 0.1 },
		limits: { kinematic_monitor: true, channels: [], notch: { smoothing_ticks: 3, release_ramp_db_per_s: 20, near_bound_db: 3 } },
		feasibility: { enabled: false, shakers: [] },
		simulator: { structure: null },
		record: defaultRecord(),
		metadata: [{ key: 'run_number', label: 'Run Number', value: '' }]
	};
}

const arr = <T>(v: unknown, fallback: T[]): T[] => (Array.isArray(v) ? (v as T[]) : fallback);
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {});

function mergeProfile(base: SineProfileDraft, saved: unknown): SineProfileDraft {
	const s = obj(saved) as Partial<SineProfileDraft>;
	const out = { ...base, ...s } as SineProfileDraft;
	out.rows = Array.isArray(s.rows) && s.rows.length ? s.rows.map((r) => ({ ...newRow(), ...r })) : base.rows;
	return out;
}

/** A draft read back from storage or a file, every section filled from the defaults where it is missing. */
export function mergeMsineSetup(saved: Partial<MsineDraft> | null): MsineDraft {
	const base = defaultMsineSetup();
	if (!saved || typeof saved !== 'object') return base;
	const out = { ...base, ...saved } as MsineDraft;
	out.controls = arr<MsineControlDraft>(saved.controls, []).map((c) => {
		const n = newControl(c?.channel ?? 0);
		return { channel: c?.channel ?? 0, profile: mergeProfile(n.profile, c?.profile), phase: arr(c?.phase, n.phase) };
	});
	out.notControls = arr(saved.notControls, []);
	out.drives = arr(saved.drives, []);
	out.phase_specified = typeof saved.phase_specified === 'boolean' ? saved.phase_specified : true;
	const t = obj(saved.test) as Partial<MsineDraft['test']>;
	out.test = {
		...base.test,
		...t,
		sweep: { ...base.test.sweep, ...obj(t.sweep) },
		dwell: { ...base.test.dwell, ...obj(t.dwell) },
		stepped: { ...base.test.stepped, ...obj(t.stepped) }
	};
	if (!['sweep', 'dwell', 'stepped'].includes(out.test.kind)) out.test.kind = 'sweep';
	if (!Array.isArray(out.test.band_hz) || out.test.band_hz.length !== 2) out.test.band_hz = base.test.band_hz;
	out.test.stepped.frequencies_hz = arr(out.test.stepped.frequencies_hz, base.test.stepped.frequencies_hz);
	out.acquisition = { ...base.acquisition, ...obj(saved.acquisition) };
	const s = obj(saved.schedule) as Partial<MsineDraft['schedule']>;
	out.schedule = { ...base.schedule, ...s, per_level: { ...base.schedule.per_level, ...obj(s.per_level) }, full_level: { ...base.schedule.full_level, ...obj(s.full_level) } };
	if (!Array.isArray(out.schedule.levels) || !out.schedule.levels.length) out.schedule.levels = base.schedule.levels;
	out.identification = { ...base.identification, ...obj(saved.identification) };
	out.law = { ...base.law, ...obj(saved.law) } as LawDraft;
	const c = obj(saved.control) as Partial<MsineDraft['control']>;
	out.control = { ...base.control, ...c, tracking: { ...base.control.tracking, ...obj(c.tracking) } };
	out.requests = { ...base.requests, ...obj(saved.requests) };
	const l = obj(saved.limits) as Partial<MsineDraft['limits']>;
	out.limits = { ...base.limits, ...l, notch: { ...base.limits.notch, ...obj(l.notch) } };
	out.limits.channels = arr<MsineLimitDraft>(l.channels, []).map((ch) => ({ channel: ch?.channel ?? 0, profile: mergeProfile(newLimit(0).profile, ch?.profile) }));
	const f = obj(saved.feasibility) as Partial<MsineDraft['feasibility']>;
	out.feasibility = { enabled: f.enabled === true, shakers: arr<MsineShakerDraft>(f.shakers, []).map((x) => ({ ...newShaker(x?.drive ?? 0), ...x, moving_mass_lb: { ...newShaker(0).moving_mass_lb, ...obj(x?.moving_mass_lb) } })) };
	out.simulator = { ...base.simulator, ...obj(saved.simulator) };
	out.record = { ...base.record, ...obj(saved.record) };
	out.metadata = arr(saved.metadata, base.metadata);
	return followTitle(out, msineTitle);
}

// ---------------------------------------------------------------------------------------------
// The channels

/**
 * The draft's channels fitted to the table: every enabled control row a control (unless taken
 * out), each keeping its reference, a new one starting from the first control's; every enabled
 * drive row a drive in the table's order (its ceiling and abort kept, new ones to be given); limit
 * channels only on enabled inputs that are not controls; one shaker record per drive. Returns
 * whether anything changed.
 */
export function fitChannels(d: MsineDraft, rows: ChannelRow[]): boolean {
	// A table not read yet holds no rows: the draft keeps what it has.
	if (!rows.length) return false;
	const before = JSON.stringify([d.controls.map((c) => c.channel), d.drives, d.limits.channels.map((c) => c.channel), d.notControls, d.feasibility.shakers.map((s) => s.drive)]);
	const on = rows.filter((r) => r.enabled);
	const controlRows = on.filter((r) => r.role === 'control').map((r) => r.index);
	d.notControls = d.notControls.filter((i) => controlRows.includes(i));
	const next = controlRows.filter((i) => !d.notControls.includes(i));
	const byChannel = new Map(d.controls.map((c) => [c.channel, c]));
	const first = d.controls[0];
	const band = bandOf(d);
	d.controls = next.map((ch) => byChannel.get(ch) ?? (first ? { ...plainCopy(first), channel: ch } : newControl(ch, band)));
	const driveRows = on.filter((r) => r.role === 'drive').map((r) => r.index);
	const drives = new Map(d.drives.map((x) => [x.channel, x]));
	d.drives = driveRows.map((ch) => drives.get(ch) ?? { channel: ch, ceiling_vpk: NaN, abort_vpk: NaN });
	const inputs = new Set(on.filter((r) => r.role !== 'drive' && !next.includes(r.index)).map((r) => r.index));
	d.limits.channels = d.limits.channels.filter((l) => inputs.has(l.channel));
	const shakers = new Map(d.feasibility.shakers.map((s) => [s.drive, s]));
	d.feasibility.shakers = driveRows.map((ch) => shakers.get(ch) ?? newShaker(ch));
	return before !== JSON.stringify([d.controls.map((c) => c.channel), d.drives, d.limits.channels.map((c) => c.channel), d.notControls, d.feasibility.shakers.map((s) => s.drive)]);
}

/** A value as plain data, whether or not it is reactive state. */
function plainCopy<T>(v: T): T {
	return JSON.parse(JSON.stringify(v, (_k, x) => (typeof x === 'number' && Number.isNaN(x) ? '__NaN__' : x)), (_k, x) => (x === '__NaN__' ? NaN : x)) as T;
}

/** Make an enabled control row a control, or take it out of the controls (it may then be a limit). */
export function setControl(d: MsineDraft, rows: ChannelRow[], channel: number, on: boolean): void {
	d.notControls = on ? d.notControls.filter((i) => i !== channel) : [...new Set([...d.notControls, channel])];
	if (on) d.limits.channels = d.limits.channels.filter((l) => l.channel !== channel);
	fitChannels(d, rows);
}

/** The rows that can be limit channels: enabled inputs that are not controls. */
export function limitRows(d: MsineDraft, rows: ChannelRow[]): ChannelRow[] {
	const controls = new Set(d.controls.map((c) => c.channel));
	return rows.filter((r) => r.enabled && r.role !== 'drive' && !controls.has(r.index));
}

/** The shape the counts give, in the protocol's words. */
export function shapeOf(controls: number, drives: number): MimoShape {
	return controls === drives ? 'square' : controls > drives ? 'more_controls_than_drives' : 'fewer_controls_than_drives';
}

// ---------------------------------------------------------------------------------------------
// The law

/** Why a law of the catalog cannot run this setup, in plain words, or null when it can. */
export function lawRefusal(law: LawEntry, d: Pick<MsineDraft, 'phase_specified'> & { kind: MimoSineTestType; controls: number; drives: number }): string | null {
	const e = law as MimoSineLawEntry;
	const kind = TEST_KINDS.find((k) => k.id === d.kind)?.label.toLowerCase() ?? d.kind;
	const types = e.test_types ?? null;
	if (types && !types.includes(d.kind)) {
		const words = types.map((t) => TEST_KINDS.find((k) => k.id === t)?.label.toLowerCase() ?? t);
		const list = words.length === 1 ? `a ${words[0]} only` : `${words.slice(0, -1).map((w) => `a ${w}`).join(', ')} or a ${words[words.length - 1]}`;
		return `Not for a ${kind}: ${law.name} accepts ${list}.`;
	}
	if (!d.phase_specified && e.amplitude_only === false) return `Needs a phase on every reference: ${law.name} does not choose the phases itself.`;
	const shape = shapeOf(d.controls, d.drives);
	if (d.controls > 0 && d.drives > 0 && e.shapes && !e.shapes.includes(shape)) {
		const words: Record<MimoShape, string> = { square: 'as many controls as drives', more_controls_than_drives: 'more controls than drives', fewer_controls_than_drives: 'fewer controls than drives' };
		return `Not for ${words[shape]}: ${law.name} does not accept that shape.`;
	}
	return null;
}

// ---------------------------------------------------------------------------------------------
// To and from the protocol

/** The band the test covers: a dwell's is its one frequency, a list's from its lowest to its highest. */
export function bandOf(d: Pick<MsineDraft, 'test'>): [number, number] {
	const t = d.test;
	if (t.kind === 'dwell') return [t.dwell.freq_hz, t.dwell.freq_hz];
	if (t.kind === 'stepped' && t.stepped.on === 'list') {
		const f = t.stepped.frequencies_hz.filter(Number.isFinite);
		return f.length ? [Math.min(...f), Math.max(...f)] : [NaN, NaN];
	}
	return [t.band_hz[0], t.band_hz[1]];
}

/** The title the references and the test give: "MIMO Sine 1 g, 20 to 2000 Hz". */
export function msineTitle(d: MsineDraft): string {
	const gs = d.controls.flatMap((c) => c.profile.rows.filter((r) => r.quantity === 'accel_g').map((r) => r.value));
	const g = gs.length ? Math.max(...gs) : NaN;
	const level = Number.isFinite(g) ? ` ${titleNumber(g)} g` : '';
	const [lo, hi] = bandOf(d);
	const where =
		d.test.kind === 'dwell'
			? Number.isFinite(lo) ? `, dwell at ${titleNumber(lo)} Hz` : ''
			: Number.isFinite(lo) && Number.isFinite(hi) ? `, ${titleNumber(lo)} to ${titleNumber(hi)} Hz` : '';
	return `MIMO Sine${level}${where}`;
}

export function msineTitleOf(d: MsineDraft): string {
	return d.title.trim() || msineTitle(d);
}

const forOf = (by: 'seconds' | 'cycles', seconds: number, cycles: number): SineDwellFor => (by === 'seconds' ? { seconds } : { cycles });
const rungOf = (r: { by: 'sweeps' | 'seconds'; n: number }): SineRungFor => (r.by === 'sweeps' ? { sweeps: r.n } : { seconds: r.n });

/** The test the draft describes, the chosen kind's settings only. */
export function testOf(d: MsineDraft): MimoSineTest {
	const t = d.test;
	if (t.kind === 'dwell') return { kind: 'dwell', freq_hz: t.dwell.freq_hz, end: forOf(t.dwell.by, t.dwell.seconds, t.dwell.cycles) };
	if (t.kind === 'stepped') {
		const st = t.stepped;
		if (st.on === 'list') return { kind: 'stepped', frequencies_hz: [...st.frequencies_hz], dwell: forOf(st.by === 'cycles' ? 'cycles' : 'seconds', st.seconds, st.cycles) };
		return { kind: 'stepped', band_hz: [t.band_hz[0], t.band_hz[1]], steps_per_octave: st.steps_per_octave, spacing: st.spacing, dwell: st.by === 'computed' ? null : forOf(st.by, st.seconds, st.cycles) };
	}
	const s = t.sweep;
	return {
		kind: 'sweep',
		band_hz: [t.band_hz[0], t.band_hz[1]],
		rate: s.law === 'log' ? { law: 'log', oct_per_min: s.oct_per_min } : { law: 'linear', hz_per_s: s.hz_per_s },
		start_hz: s.start_hz ?? t.band_hz[0],
		direction: s.direction,
		end: { sweeps: s.end_by === 'duration' ? null : s.sweeps, duration_s: s.end_by === 'sweeps' ? null : s.duration_s }
	};
}

/** The configuration the draft describes, every field explicit. Call `localErrors` first. */
export function toMsineConfig(d: MsineDraft, law: LawEntry | null): MimoSineConfig {
	const c = d.control;
	const tracking = c.tracking.kind === 'fixed' ? { kind: 'fixed' as const, hz: c.tracking.hz } : { kind: 'proportional' as const, pct: c.tracking.pct, max_hz: c.tracking.max_hz };
	const r = d.record;
	const [document, clause] = [r.document.trim(), r.clause.trim()];
	const f = d.feasibility;
	const shakers = f.enabled ? f.shakers.map((s) => ({ drive: s.drive, shaker: shakerRecord(s.shaker_text) ?? {}, moving_mass_lb: { ...s.moving_mass_lb } })) : null;
	return {
		title: msineTitleOf(d),
		channels: {
			controls: d.controls.map((x) => (d.phase_specified ? { channel: x.channel, profile: profileOf(x.profile), phase: x.phase.map((p) => ({ freq_hz: p.freq_hz, phase_deg: p.phase_deg })) } : { channel: x.channel, profile: profileOf(x.profile) })),
			drives: d.drives.map((x) => ({ channel: x.channel, ceiling_vpk: x.ceiling_vpk, abort_vpk: x.abort_vpk }))
		},
		phase_specified: d.phase_specified,
		test: testOf(d),
		acquisition: { fs_hz: d.acquisition.fs_hz, tick_ms: d.acquisition.tick_ms },
		schedule: {
			mode: d.schedule.mode,
			levels_db: [...d.schedule.levels],
			start_db: d.schedule.start_db,
			per_level: rungOf(d.schedule.per_level),
			full_level: rungOf(d.schedule.full_level)
		},
		identification: { ...d.identification },
		control: {
			tracking,
			law: { id: d.law.id, params: lawWireParams(law, d.law.params) },
			law_budget: c.law_budget,
			law_misses: c.law_misses,
			max_step_db: c.max_step_db,
			max_rise_db_per_s: c.max_rise_db_per_s,
			rotation_cap_deg: c.rotation_cap_deg,
			divergence_s: c.divergence_s,
			abort_ticks: c.abort_ticks
		},
		requests: { ...d.requests },
		limits: {
			kinematic_monitor: d.limits.kinematic_monitor,
			channels: d.limits.channels.map((ch) => ({ channel: ch.channel, profile: profileOf(ch.profile) })),
			notch: { ...d.limits.notch }
		},
		feasibility: shakers ? { shakers: shakers as never } : null,
		simulator: { structure: d.simulator.structure },
		operator: r.operator.trim(),
		article: r.article.trim(),
		notes: r.notes.trim(),
		specification: document || clause ? { document, clause } : null,
		record: { streams: r.streams },
		metadata: d.metadata.map((m) => (m.label ? { key: m.key, label: m.label, value: m.value } : { key: m.key, value: m.value }))
	};
}

/**
 * The draft for a configuration the service holds (another screen applied it, or this one before
 * a reload, or a setup file). Fields the configuration leaves out take the wire's defaults; a test
 * kind it does not carry keeps the shell's.
 */
export function fromMsineConfig(c: MimoSineConfig): MsineDraft {
	const base = defaultMsineSetup();
	const t = c.test;
	const test: MsineDraft['test'] = structuredClone(base.test);
	const dwellBy = (x: SineDwellFor | null | undefined) => (x && 'cycles' in x ? { by: 'cycles' as const, cycles: x.cycles } : x && 'seconds' in x ? { by: 'seconds' as const, seconds: x.seconds } : {});
	test.kind = t.kind;
	if (t.kind === 'sweep') {
		test.band_hz = [t.band_hz[0], t.band_hz[1]];
		const sweeps = t.end.sweeps ?? null;
		const duration = t.end.duration_s ?? null;
		test.sweep = {
			law: t.rate.law,
			oct_per_min: t.rate.law === 'log' ? t.rate.oct_per_min : base.test.sweep.oct_per_min,
			hz_per_s: t.rate.law === 'linear' ? t.rate.hz_per_s : base.test.sweep.hz_per_s,
			start_hz: t.start_hz ?? null,
			direction: t.direction ?? 'up',
			end_by: sweeps !== null && duration !== null ? 'both' : duration !== null ? 'duration' : 'sweeps',
			sweeps: sweeps ?? base.test.sweep.sweeps,
			duration_s: duration ?? base.test.sweep.duration_s
		};
	} else if (t.kind === 'dwell') {
		test.dwell = { ...test.dwell, freq_hz: t.freq_hz, ...dwellBy(t.end) };
	} else if ('frequencies_hz' in t) {
		test.stepped = { ...test.stepped, on: 'list', frequencies_hz: [...t.frequencies_hz], ...dwellBy(t.dwell) };
	} else {
		test.band_hz = [t.band_hz[0], t.band_hz[1]];
		test.stepped = { ...test.stepped, on: 'grid', steps_per_octave: t.steps_per_octave, spacing: t.spacing ?? 'log', ...(t.dwell ? dwellBy(t.dwell) : { by: 'computed' as const }) };
	}
	const sch = c.schedule ?? {};
	const levels = sch.levels_db ?? [-12, -9, -6, -3, 0];
	const rung = (r: SineRungFor | undefined) => (!r ? { by: 'sweeps' as const, n: 1 } : 'sweeps' in r ? { by: 'sweeps' as const, n: r.sweeps } : { by: 'seconds' as const, n: r.seconds });
	const ctl = c.control ?? {};
	const tr = ctl.tracking;
	const lim = c.limits ?? {};
	const band: [number, number] = t.kind === 'sweep' ? t.band_hz : base.test.band_hz;
	const draft: MsineDraft = {
		title: c.title ?? '',
		controls: c.channels.controls.map((x) => ({ channel: x.channel, profile: profileDraftOf(x.profile, newControl(x.channel, band).profile), phase: x.phase?.length ? x.phase.map((p) => ({ ...p })) : newControl(x.channel, band).phase })),
		notControls: [],
		drives: c.channels.drives.map((x) => ({ ...x })),
		phase_specified: c.phase_specified ?? true,
		test,
		acquisition: { fs_hz: c.acquisition?.fs_hz ?? null, tick_ms: c.acquisition?.tick_ms ?? 10 },
		schedule: {
			mode: sch.mode ?? 'auto',
			levels: [...levels],
			start_db: sch.start_db ?? levels[0],
			per_level: rung(sch.per_level),
			full_level: rung(sch.full_level)
		},
		identification: { ...base.identification, ...(c.identification ?? {}) },
		law: { id: ctl.law?.id ?? 'matrix', params: { ...((ctl.law?.params ?? {}) as LawDraft['params']) } },
		control: {
			tracking: tr ? (tr.kind === 'fixed' ? { ...base.control.tracking, kind: 'fixed', hz: tr.hz } : { ...base.control.tracking, kind: 'proportional', pct: tr.pct, max_hz: tr.max_hz }) : { ...base.control.tracking },
			law_budget: ctl.law_budget ?? base.control.law_budget,
			law_misses: ctl.law_misses ?? base.control.law_misses,
			max_step_db: ctl.max_step_db ?? base.control.max_step_db,
			max_rise_db_per_s: ctl.max_rise_db_per_s ?? base.control.max_rise_db_per_s,
			rotation_cap_deg: ctl.rotation_cap_deg ?? null,
			divergence_s: ctl.divergence_s ?? base.control.divergence_s,
			abort_ticks: ctl.abort_ticks ?? base.control.abort_ticks
		},
		requests: { ...base.requests, ...(c.requests ?? {}) },
		limits: {
			kinematic_monitor: lim.kinematic_monitor ?? true,
			channels: (lim.channels ?? []).map((ch) => ({ channel: ch.channel, profile: profileDraftOf(ch.profile, newLimit(ch.channel).profile) })),
			notch: { ...base.limits.notch, ...(lim.notch ?? {}) }
		},
		feasibility: c.feasibility
			? { enabled: true, shakers: c.feasibility.shakers.map((s) => ({ drive: s.drive, shaker_text: JSON.stringify(s.shaker, null, 2), moving_mass_lb: { ...newShaker(0).moving_mass_lb, ...s.moving_mass_lb } })) }
			: base.feasibility,
		simulator: { structure: c.simulator?.structure ?? null },
		record: {
			operator: c.operator ?? '',
			article: c.article ?? '',
			notes: c.notes ?? '',
			document: c.specification?.document ?? '',
			clause: c.specification?.clause ?? '',
			streams: c.record?.streams ?? true
		},
		metadata: (c.metadata ?? []).map((m) => ({ key: m.key, label: m.label ?? '', value: m.value }))
	};
	return followTitle(draft, msineTitle);
}

// ---------------------------------------------------------------------------------------------
// Where a finding belongs

export type MsineSection = 'channels' | 'references' | 'test' | 'schedule' | 'law' | 'limits';

export const MSINE_SECTIONS: { id: MsineSection; label: string }[] = [
	{ id: 'channels', label: 'Station & Channels' },
	{ id: 'references', label: 'References' },
	{ id: 'test', label: 'Test' },
	{ id: 'schedule', label: 'Schedule' },
	{ id: 'law', label: 'Control' },
	{ id: 'limits', label: 'Limits' }
];

/**
 * The editor's key for a service finding. A field path loses its `config.` head and its brackets.
 * A control's profile and phase findings go under that control's tables; a profile row located by
 * `row` and `channel` goes under the control's or the limit channel's table it is on; the law's
 * path is the law panel's (`control.law.params.x` is `law.params.x`).
 */
export function msineItemKey(item: Pick<ReportItem, 'field' | 'channel' | 'row'> & { reason: string }, d?: Pick<MsineDraft, 'controls' | 'limits'>): string {
	if (item.field) {
		const path = item.field.replace(/^config\./, '').replace(/\[(\d+)\]/g, '.$1');
		let m = /^channels\.controls\.(\d+)\.(profile|phase)/.exec(path);
		if (m) return `controls.${m[1]}.${m[2] === 'profile' ? 'profile.rows' : 'phase'}`;
		m = /^channels\.drives\.(\d+)\.(ceiling_vpk|abort_vpk)$/.exec(path);
		if (m) return `drives.${m[1]}.${m[2]}`;
		if (/^channels\.drives(\.|$)/.test(path)) return 'channels.drives';
		if (/^channels(\.|$)/.test(path)) return 'channels.controls';
		m = /^schedule\.levels_db\.(\d+)$/.exec(path);
		if (m) return `schedule.levels.${m[1]}`;
		if (path === 'schedule.levels_db') return 'schedule.levels';
		if (/^schedule\.per_level(\.|$)/.test(path)) return 'schedule.per_level';
		if (/^schedule\.full_level(\.|$)/.test(path)) return 'schedule.full_level';
		if (/^test\.rate(\.|$)/.test(path)) return 'test.rate';
		if (/^test\.frequencies_hz(\.|$)/.test(path)) return 'test.frequencies_hz';
		m = /^test\.band_hz\.(\d)$/.exec(path);
		if (m) return `test.band_hz.${m[1]}`;
		m = /^limits\.channels\.(\d+)(?:\.(channel|profile))?/.exec(path);
		if (m) return `limits.channels.${m[1]}.${m[2] === 'profile' ? 'profile.rows' : 'channel'}`;
		if (/^control\.tracking(\.|$)/.test(path)) return 'control.tracking';
		if (path.startsWith('control.law.') || path === 'control.law') return path.replace(/^control\.law(\.id)?/, (_, id) => (id ? 'law.id' : 'law')).replace(/^law$/, 'law.id');
		m = /^feasibility\.shakers\.(\d+)/.exec(path);
		if (m) return `feasibility.shakers.${m[1]}`;
		if (path === 'phase_specified') return 'phase_specified';
		return path;
	}
	if (item.row !== undefined && item.row !== null && item.channel !== undefined && item.channel !== null && d) {
		const ci = d.controls.findIndex((c) => c.channel === item.channel);
		if (ci >= 0) return `controls.${ci}.profile.rows`;
		const li = d.limits.channels.findIndex((c) => c.channel === item.channel);
		if (li >= 0) return `limits.channels.${li}.profile.rows`;
	}
	if (item.reason === 'limit_unknown_channel' || item.reason === 'limit_on_control_channel') return 'limits.channels';
	if (item.reason.startsWith('structure_')) return 'simulator.structure';
	if (item.reason.startsWith('law_')) return 'law.id';
	if (item.channel !== undefined && item.channel !== null) return 'channels.controls';
	return 'setup';
}

export function msineByKey(items: { field?: string | null; channel?: number | null; row?: number; reason: string; message: string }[], d?: Pick<MsineDraft, 'controls' | 'limits'>): FieldErrors {
	const out: FieldErrors = {};
	for (const it of items) {
		const k = msineItemKey(it as Pick<ReportItem, 'field' | 'channel' | 'row'> & { reason: string }, d);
		if (!(out[k] ??= []).includes(it.message)) out[k].push(it.message);
	}
	return out;
}

/** The section a key belongs to. */
export function msineSectionOf(key: string): MsineSection {
	if (/^(controls|phase_specified)(\.|$)/.test(key)) return 'references';
	if (/^(test|acquisition\.fs_hz)(\.|$)/.test(key)) return 'test';
	if (/^(schedule|identification)(\.|$)/.test(key)) return 'schedule';
	if (/^(law|control|requests|acquisition)(\.|$)/.test(key)) return 'law';
	if (/^(limits|feasibility)(\.|$)/.test(key)) return 'limits';
	return 'channels';
}

// ---------------------------------------------------------------------------------------------
// What the editor can say before anything is sent

const NUMBER = 'Enter a number.';
const WHOLE = 'Enter a whole number.';

function add(map: FieldErrors, key: string, message: string) {
	(map[key] ??= []).push(message);
}

function need(map: FieldErrors, key: string, v: number, whole = false) {
	if (!Number.isFinite(v)) add(map, key, whole ? WHOLE : NUMBER);
	else if (whole && !Number.isInteger(v)) add(map, key, WHOLE);
}

function needPair(map: FieldErrors, key: string, v: readonly number[]) {
	if (!(Number.isFinite(v[0]) && Number.isFinite(v[1]))) add(map, key, 'Give both: below and above.');
}

function profileErrors(out: FieldErrors, p: SineProfileDraft, at: string) {
	if (p.rows.length < 2) add(out, `${at}.rows`, 'Give at least two rows.');
	p.rows.forEach((r, i) => {
		need(out, `${at}.rows.${i}.freq_hz`, r.freq_hz);
		need(out, `${at}.rows.${i}.value`, r.value);
		if (r.own_bands) {
			needPair(out, `${at}.rows.${i}.alarm_db`, r.alarm_db);
			needPair(out, `${at}.rows.${i}.abort_db`, r.abort_db);
		}
	});
	needPair(out, `${at}.alarm_db`, p.alarm_db);
	needPair(out, `${at}.abort_db`, p.abort_db);
}

/**
 * Fields that hold no number, a reference with too few rows, a phase row out of range, no control
 * or no drive, a drive's ceiling or abort not given, too many limit channels, a shaker record that
 * does not read. Everything else (each reference's coverage of the band, the tracking, the law's
 * acceptance of the test) is the service's to judge in the dry run.
 */
export function localErrors(d: MsineDraft): FieldErrors {
	const out: FieldErrors = {};
	if (!d.controls.length) add(out, 'channels.controls', 'Switch on at least one control row in the channel table.');
	if (d.controls.length > MAX_CONTROLS) add(out, 'channels.controls', `At most ${MAX_CONTROLS} controls.`);
	if (!d.drives.length) add(out, 'channels.drives', 'Switch on at least one drive row in the channel table.');
	if (d.drives.length > MAX_DRIVES) add(out, 'channels.drives', `At most ${MAX_DRIVES} drives.`);
	d.controls.forEach((c, i) => {
		profileErrors(out, c.profile, `controls.${i}.profile`);
		if (d.phase_specified) {
			if (!c.phase.length) add(out, `controls.${i}.phase`, 'Give the reference phase at one frequency at least.');
			c.phase.forEach((p, k) => {
				if (!Number.isFinite(p.freq_hz) || !Number.isFinite(p.phase_deg)) add(out, `controls.${i}.phase`, `Row ${k + 1}: ${NUMBER}`);
				else if (p.phase_deg <= -180 || p.phase_deg > 180) add(out, `controls.${i}.phase`, `Row ${k + 1}: a phase above -180 and at most 180 degrees.`);
			});
		}
	});
	d.drives.forEach((x, i) => {
		if (!Number.isFinite(x.ceiling_vpk)) add(out, `drives.${i}.ceiling_vpk`, 'Give the ceiling in volts peak. There is no default: it is your amplifier input.');
		if (!Number.isFinite(x.abort_vpk)) add(out, `drives.${i}.abort_vpk`, 'Give the abort in volts peak, above the ceiling.');
		else if (Number.isFinite(x.ceiling_vpk) && x.abort_vpk <= x.ceiling_vpk) add(out, `drives.${i}.abort_vpk`, 'The abort is above the ceiling.');
	});
	const t = d.test;
	if (t.kind === 'sweep' || (t.kind === 'stepped' && t.stepped.on === 'grid')) {
		need(out, 'test.band_hz.0', t.band_hz[0]);
		need(out, 'test.band_hz.1', t.band_hz[1]);
	}
	if (t.kind === 'sweep') {
		const s = t.sweep;
		need(out, 'test.rate', s.law === 'log' ? s.oct_per_min : s.hz_per_s);
		if (s.start_hz !== null) need(out, 'test.start_hz', s.start_hz);
		if (s.end_by !== 'duration') need(out, 'test.end.sweeps', s.sweeps, true);
		if (s.end_by !== 'sweeps') need(out, 'test.end.duration_s', s.duration_s);
	} else if (t.kind === 'dwell') {
		need(out, 'test.freq_hz', t.dwell.freq_hz);
		if (t.dwell.by === 'seconds') need(out, 'test.end.seconds', t.dwell.seconds);
		else need(out, 'test.end.cycles', t.dwell.cycles, true);
	} else {
		const st = t.stepped;
		if (st.on === 'grid') need(out, 'test.steps_per_octave', st.steps_per_octave);
		else {
			if (!st.frequencies_hz.length) add(out, 'test.frequencies_hz', 'Give at least one frequency.');
			if (st.frequencies_hz.some((f) => !Number.isFinite(f) || f <= 0)) add(out, 'test.frequencies_hz', 'Every frequency is a number above 0.');
			if (st.by === 'computed') add(out, 'test.dwell', 'A list of frequencies needs a dwell in seconds or cycles at each.');
		}
		if (st.by === 'seconds') need(out, 'test.dwell.seconds', st.seconds);
		else if (st.by === 'cycles') need(out, 'test.dwell.cycles', st.cycles, true);
	}
	if (d.acquisition.fs_hz !== null) need(out, 'acquisition.fs_hz', d.acquisition.fs_hz);
	need(out, 'acquisition.tick_ms', d.acquisition.tick_ms);
	const sch = d.schedule;
	if (!sch.levels.length) add(out, 'schedule.levels', 'Give at least one level.');
	sch.levels.forEach((l, i) => need(out, `schedule.levels.${i}`, l));
	need(out, 'schedule.start_db', sch.start_db);
	for (const k of ['per_level', 'full_level'] as const) {
		need(out, `schedule.${k}`, sch[k].n, sch[k].by === 'sweeps');
		if (sch.mode === 'auto' && sch[k].by === 'sweeps' && t.kind !== 'sweep') add(out, `schedule.${k}`, 'Sweeps count in a sweep test only; give seconds.');
	}
	const id = d.identification;
	for (const k of ['level_db', 'resolution_hz', 'trust_coherence', 'gate_coherence'] as const) need(out, `identification.${k}`, id[k]);
	if (id.frames !== null) need(out, 'identification.frames', id.frames, true);
	const c = d.control;
	if (c.tracking.kind === 'fixed') need(out, 'control.tracking', c.tracking.hz);
	else {
		need(out, 'control.tracking', c.tracking.pct);
		need(out, 'control.tracking', c.tracking.max_hz);
	}
	for (const k of ['law_budget', 'max_step_db', 'max_rise_db_per_s', 'divergence_s'] as const) need(out, `control.${k}`, c[k]);
	need(out, 'control.law_misses', c.law_misses, true);
	need(out, 'control.abort_ticks', c.abort_ticks, true);
	if (c.rotation_cap_deg !== null) need(out, 'control.rotation_cap_deg', c.rotation_cap_deg);
	need(out, 'requests.max_hold_s', d.requests.max_hold_s);
	need(out, 'requests.min_slow_factor', d.requests.min_slow_factor);
	const lim = d.limits;
	if (lim.channels.length > MAX_LIMITS) add(out, 'limits.channels', `At most ${MAX_LIMITS} limit channels.`);
	lim.channels.forEach((ch, i) => profileErrors(out, ch.profile, `limits.channels.${i}.profile`));
	need(out, 'limits.notch.smoothing_ticks', lim.notch.smoothing_ticks, true);
	need(out, 'limits.notch.release_ramp_db_per_s', lim.notch.release_ramp_db_per_s);
	need(out, 'limits.notch.near_bound_db', lim.notch.near_bound_db);
	if (d.feasibility.enabled) {
		d.feasibility.shakers.forEach((s, i) => {
			if (!s.shaker_text.trim()) add(out, `feasibility.shakers.${i}`, `Drive ${s.drive}: load or paste the shaker record, or turn the check off.`);
			else if (!shakerRecord(s.shaker_text)) add(out, `feasibility.shakers.${i}`, `Drive ${s.drive}: the shaker record does not read as a record (JSON object).`);
			if (!Object.values(s.moving_mass_lb).every(Number.isFinite)) add(out, `feasibility.shakers.${i}`, `Drive ${s.drive}: ${NUMBER}`);
		});
	}
	if (d.record.clause.trim() && !d.record.document.trim()) add(out, 'specification.document', 'Name the document this clause is from.');
	return out;
}
