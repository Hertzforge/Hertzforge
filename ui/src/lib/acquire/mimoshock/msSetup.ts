/**
 * The MIMO Shock setup as the operator edits it, and its conversion to and from the protocol's
 * `MimoShockConfig` (schema/PROTOCOL.md, "MIMO Shock Configuration"). Pure functions only: the
 * service computes every number about the targets, the grid and the plant, and this module only
 * carries the operator's choices.
 *
 * Numbers are held as numbers; a field that holds no number is NaN, which `localErrors` names
 * before anything is sent. A choice the wire leaves to the service (the record length, the
 * identification frames, the absolute abort line, the simulator's plant) is null. Times the
 * operator types in milliseconds (the timing window and the held delays) go out in seconds.
 *
 * The channels are the channel table's: every enabled control row is a control unless it is made
 * a limit; every enabled drive row is a drive, in the table's order, each with its own ceiling and
 * abort; enabled monitor rows may be limits. Each control has its own target, in the controls'
 * order, so the targets follow the controls as rows are switched on and off.
 */

import type { ChannelRow, LadderMode, ReportItem } from '$lib/protocol/types';
import type { CompensationShape, ShockPolarity, ShockShape, ShockStandardName } from '$lib/protocol/shock';
import type { SrsEngine } from '$lib/protocol/srs';
import type { MimoShockConfig, MimoShockLawEntry, MimoShockTargetKind, MimoShockVerdictMode } from '$lib/protocol/mimoShock';
import { lawWireParams, type LawDraft } from '../law/lawForm';
import type { FieldErrors } from '../setup';

// ---------------------------------------------------------------------------------------------
// The draft

export interface MsPulseDraft {
	shape: ShockShape;
	amplitude_g: number;
	duration_ms: number;
	polarity: ShockPolarity;
	/** Trapezoid only, fractions of the duration; kept for the other shapes, sent as null. */
	rise_frac: number;
	fall_frac: number;
	compensation: { shape: CompensationShape; pre_frac: number; post_frac: number; symmetric: boolean };
}

export interface MsPointDraft {
	freq_hz: number;
	accel_g: number;
}

export interface MsSrsControlDraft {
	points: MsPointDraft[];
	engine: SrsEngine;
	max_iters: number;
	tol_db: number;
	max_gain_db_per_iter: number;
	polarity: ShockPolarity;
}

export interface MsDriveDraft {
	channel: number;
	ceiling_vpk: number;
	abort_vpk: number;
}

export interface MsLimitDraft {
	channel: number;
	abort_peak: number;
}

export interface MsDraft {
	title: string;
	/** The controls, by table index, in the table's order. */
	controls: number[];
	/** Enabled control rows the operator made limits instead, by table index. */
	notControls: number[];
	drives: MsDriveDraft[];
	limits: MsLimitDraft[];
	kind: MimoShockTargetKind;
	standard: ShockStandardName;
	/** One per control, in the controls' order. */
	pulses: MsPulseDraft[];
	srs: {
		bands: { f_lo_hz: number; f_hi_hz: number; bands_per_octave: number; q: number };
		tolerance_db: [number, number];
		/** One per control, in the controls' order. */
		controls: MsSrsControlDraft[];
	};
	timing: { mode: 'free' | 'hold'; window_ms: number; delays_ms: number[] };
	verdict: MimoShockVerdictMode;
	acquisition: { fs_hz: number; record_len: number | null; band_hz: [number, number]; lead_s: number };
	schedule: { mode: LadderMode; levels_db: number[]; max_pulses_per_level: number; full_level_pulses: number };
	spacing: { cooling_s: number; settling_s: number; compute_s: number };
	identification: { drive_vrms: number; frames: number | null; coherence_floor: number; review: boolean };
	/** The law and its parameters; the id empty until the catalog has named its first law. */
	law: LawDraft;
	control: {
		law_budget: number;
		law_misses: number;
		max_pulses: number;
		max_growth_db: number;
		max_band_growth_db: number;
		divergence_pulses: number;
		divergence_drive_ratio: number;
		first_drive_ratio: number;
		/** dB over the full-level target's peak; null: the service's default line. */
		abort_db: number | null;
		max_velocity_ips: number | null;
		max_displacement_in: number | null;
		/** The abort lines scaled to the level also stop the run. */
		scaled_aborts: boolean;
	};
	simulator: { plant: string | null };
	capture: { auto_start: boolean; run_name: string };
	metadata: { key: string; label: string; value: string }[];
}

export const SHAPES: { id: ShockShape; label: string }[] = [
	{ id: 'half_sine', label: 'Half-sine' },
	{ id: 'haversine', label: 'Haversine' },
	{ id: 'terminal_peak_sawtooth', label: 'Terminal-peak sawtooth' },
	{ id: 'initial_peak_sawtooth', label: 'Initial-peak sawtooth' },
	{ id: 'trapezoid', label: 'Trapezoid' },
	{ id: 'rectangle', label: 'Rectangle' }
];

export const COMPENSATIONS: { id: CompensationShape; label: string }[] = [
	{ id: 'rectangular', label: 'Rectangular' },
	{ id: 'rounded_rectangular', label: 'Rounded rectangular' },
	{ id: 'half_sine', label: 'Half-sine' }
];

export const STANDARDS: { id: ShockStandardName; label: string }[] = [
	{ id: 'mil_std_810_516', label: 'MIL-STD-810, Method 516' },
	{ id: 'iec_60068_2_27', label: 'IEC 60068-2-27' }
];

/** The simulator's plants, by name, with what each is. */
export const PLANTS: { id: string; label: string }[] = [
	{ id: 'shock-linear-4x4', label: 'Linear, 4 controls by 4 drives' },
	{ id: 'shock-jointed-4x4-moderate', label: 'Jointed, 4 by 4, moderate' },
	{ id: 'shock-jointed-4x4-severe', label: 'Jointed, 4 by 4, severe' },
	{ id: 'shock-jointed-6x3', label: 'Jointed, 6 controls by 3 drives' }
];

export const RECORD_LENGTHS = [1024, 2048, 4096, 8192, 16384];

export const MAX_CONTROLS = 16;
export const MAX_DRIVES = 8;

export function defaultPulse(): MsPulseDraft {
	return {
		shape: 'half_sine',
		amplitude_g: 20,
		duration_ms: 11,
		polarity: 'positive',
		rise_frac: 0.1,
		fall_frac: 0.1,
		compensation: { shape: 'rectangular', pre_frac: 4, post_frac: 4, symmetric: true }
	};
}

export function defaultSrsControl(): MsSrsControlDraft {
	return {
		points: [
			{ freq_hz: 50, accel_g: 4 },
			{ freq_hz: 300, accel_g: 24 },
			{ freq_hz: 2500, accel_g: 24 }
		],
		engine: 'wavsyn',
		max_iters: 50,
		tol_db: 1.5,
		max_gain_db_per_iter: 3,
		polarity: 'positive'
	};
}

export function defaultMsSetup(): MsDraft {
	return {
		title: 'MIMO Half-Sine',
		controls: [],
		notControls: [],
		drives: [],
		limits: [],
		kind: 'waveform',
		standard: 'mil_std_810_516',
		pulses: [],
		srs: { bands: { f_lo_hz: 50, f_hi_hz: 2000, bands_per_octave: 3, q: 10 }, tolerance_db: [-3, 3], controls: [] },
		timing: { mode: 'free', window_ms: 2, delays_ms: [] },
		verdict: 'all',
		acquisition: { fs_hz: 12800, record_len: null, band_hz: [5, 2500], lead_s: 0.05 },
		schedule: { mode: 'auto', levels_db: [-12, -9, -6, -3, -1.5, 0], max_pulses_per_level: 3, full_level_pulses: 3 },
		spacing: { cooling_s: 1, settling_s: 0.5, compute_s: 0.25 },
		identification: { drive_vrms: 0.05, frames: null, coherence_floor: 0.5, review: true },
		law: { id: '', params: {} },
		control: {
			law_budget: 0.25,
			law_misses: 3,
			max_pulses: 100,
			max_growth_db: 6,
			max_band_growth_db: 10,
			divergence_pulses: 3,
			divergence_drive_ratio: 1.3,
			first_drive_ratio: 2,
			abort_db: null,
			max_velocity_ips: null,
			max_displacement_in: null,
			scaled_aborts: true
		},
		simulator: { plant: null },
		capture: { auto_start: false, run_name: 'mimo_shock' },
		metadata: [{ key: 'run_number', label: 'Run Number', value: '' }]
	};
}

const arr = <T>(v: unknown, fallback: T[]): T[] => (Array.isArray(v) ? (v as T[]) : fallback);

/** A draft read back from storage, every section filled from the defaults where it is missing. */
export function mergeMsSetup(saved: Partial<MsDraft> | null): MsDraft {
	const base = defaultMsSetup();
	if (!saved || typeof saved !== 'object') return base;
	const out = { ...base, ...saved } as MsDraft;
	out.controls = arr(saved.controls, []);
	out.notControls = arr(saved.notControls, []);
	out.drives = arr(saved.drives, []);
	out.limits = arr(saved.limits, []);
	out.pulses = arr<MsPulseDraft>(saved.pulses, []).map((p) => ({ ...defaultPulse(), ...p, compensation: { ...defaultPulse().compensation, ...(p?.compensation ?? {}) } }));
	out.srs = { ...base.srs, ...(saved.srs ?? {}) };
	out.srs.bands = { ...base.srs.bands, ...(saved.srs?.bands ?? {}) };
	out.srs.controls = arr<MsSrsControlDraft>(saved.srs?.controls, []).map((c) => ({ ...defaultSrsControl(), ...c, points: arr(c?.points, defaultSrsControl().points) }));
	out.timing = { ...base.timing, ...(saved.timing ?? {}) };
	out.timing.delays_ms = arr(out.timing.delays_ms, []);
	out.acquisition = { ...base.acquisition, ...(saved.acquisition ?? {}) };
	out.schedule = { ...base.schedule, ...(saved.schedule ?? {}) };
	out.schedule.levels_db = arr(out.schedule.levels_db, base.schedule.levels_db);
	out.spacing = { ...base.spacing, ...(saved.spacing ?? {}) };
	out.identification = { ...base.identification, ...(saved.identification ?? {}) };
	out.law = { ...base.law, ...(saved.law ?? {}) };
	out.control = { ...base.control, ...(saved.control ?? {}) };
	out.simulator = { ...base.simulator, ...(saved.simulator ?? {}) };
	out.capture = { ...base.capture, ...(saved.capture ?? {}) };
	out.metadata = arr(saved.metadata, base.metadata);
	return out;
}

// ---------------------------------------------------------------------------------------------
// The channels

/**
 * The draft's channels fitted to the table: every enabled control row a control (unless made a
 * limit), every enabled drive row a drive in the table's order (its ceiling and abort kept, new
 * ones to be given), limits only on enabled rows that are not controls or drives, and one target
 * and one held delay per control, each kept with its control. Returns whether anything changed.
 */
export function fitChannels(d: MsDraft, rows: ChannelRow[]): boolean {
	// A table not read yet holds no rows: the draft keeps what it has (its drives' ceilings, its targets).
	if (!rows.length) return false;
	const before = JSON.stringify([d.controls, d.drives, d.limits, d.pulses.length, d.srs.controls.length, d.timing.delays_ms, d.notControls]);
	const on = rows.filter((r) => r.enabled);
	const controlRows = on.filter((r) => r.role === 'control').map((r) => r.index);
	d.notControls = d.notControls.filter((i) => controlRows.includes(i));
	const next = controlRows.filter((i) => !d.notControls.includes(i));
	// Each control's target and delay follow it; a new control starts from the first one's.
	const byChannel = new Map(d.controls.map((c, i) => [c, { p: d.pulses[i], s: d.srs.controls[i], t: d.timing.delays_ms[i] }]));
	const first = { p: d.pulses[0] ?? defaultPulse(), s: d.srs.controls[0] ?? defaultSrsControl() };
	d.pulses = next.map((c) => structuredClone($snap(byChannel.get(c)?.p ?? first.p)));
	d.srs.controls = next.map((c) => structuredClone($snap(byChannel.get(c)?.s ?? first.s)));
	d.timing.delays_ms = next.map((c) => byChannel.get(c)?.t ?? 0);
	d.controls = next;
	const driveRows = on.filter((r) => r.role === 'drive').map((r) => r.index);
	const kept = new Map(d.drives.map((x) => [x.channel, x]));
	d.drives = driveRows.map((c) => kept.get(c) ?? { channel: c, ceiling_vpk: NaN, abort_vpk: NaN });
	const limitable = new Set(on.filter((r) => r.role === 'monitor' || (r.role === 'control' && d.notControls.includes(r.index))).map((r) => r.index));
	d.limits = d.limits.filter((l) => limitable.has(l.channel));
	return JSON.stringify([d.controls, d.drives, d.limits, d.pulses.length, d.srs.controls.length, d.timing.delays_ms, d.notControls]) !== before;
}

/** A plain copy of a value that may be a reactive proxy. */
function $snap<T>(v: T): T {
	return JSON.parse(JSON.stringify(v)) as T;
}

/** The rows a limit can be put on: enabled monitor rows, and control rows taken out of the controls. */
export function limitRows(d: MsDraft, rows: ChannelRow[]): ChannelRow[] {
	return rows.filter((r) => r.enabled && (r.role === 'monitor' || (r.role === 'control' && d.notControls.includes(r.index))));
}

/** Make an enabled control row a control again, or take it out of the controls (a limit or nothing). */
export function setControl(d: MsDraft, rows: ChannelRow[], channel: number, control: boolean) {
	d.notControls = control ? d.notControls.filter((c) => c !== channel) : [...new Set([...d.notControls, channel])];
	if (control) d.limits = d.limits.filter((l) => l.channel !== channel);
	fitChannels(d, rows);
}

/** Copy the first control's target to every other control. */
export function sameTargetForAll(d: MsDraft) {
	if (d.kind === 'waveform' && d.pulses.length) d.pulses = d.pulses.map(() => $snap(d.pulses[0]));
	if (d.kind === 'srs' && d.srs.controls.length) d.srs.controls = d.srs.controls.map(() => $snap(d.srs.controls[0]));
}

// ---------------------------------------------------------------------------------------------
// To and from the protocol

const sec = (ms: number) => (Number.isFinite(ms) ? Math.round(ms * 1e6) / 1e9 : ms);
const ms = (s: number) => Math.round(s * 1e9) / 1e6;

/** The configuration the draft describes, every field explicit. Call `localErrors` first. */
export function toMsConfig(d: MsDraft, law: MimoShockLawEntry | null): MimoShockConfig {
	const target: MimoShockConfig['target'] =
		d.kind === 'waveform'
			? {
					kind: 'waveform',
					standard: d.standard,
					pulses: d.pulses.map((p) => ({
						shape: p.shape,
						amplitude_g: p.amplitude_g,
						duration_ms: p.duration_ms,
						polarity: p.polarity,
						rise_frac: p.shape === 'trapezoid' ? p.rise_frac : null,
						fall_frac: p.shape === 'trapezoid' ? p.fall_frac : null,
						compensation: { ...p.compensation }
					}))
				}
			: {
					kind: 'srs',
					bands: { ...d.srs.bands },
					tolerance_db: [d.srs.tolerance_db[0], d.srs.tolerance_db[1]],
					controls: d.srs.controls.map((c) => ({
						target: { kind: 'breakpoints', points: c.points.map((p) => [p.freq_hz, p.accel_g] as [number, number]) },
						synthesis: { engine: c.engine, components: null, max_iters: c.max_iters, tol_db: c.tol_db, max_gain_db_per_iter: c.max_gain_db_per_iter },
						polarity: c.polarity
					}))
				};
	return {
		title: d.title.trim(),
		channels: {
			controls: d.controls.map((channel) => ({ channel })),
			drives: d.drives.map((x) => ({ ...x })),
			limits: d.limits.map((x) => ({ ...x }))
		},
		target,
		timing: { mode: d.timing.mode, window_s: sec(d.timing.window_ms), delays_s: d.timing.mode === 'hold' ? d.timing.delays_ms.map(sec) : null },
		acquisition: { fs_hz: d.acquisition.fs_hz, record_len: d.acquisition.record_len, band_hz: [d.acquisition.band_hz[0], d.acquisition.band_hz[1]], lead_s: d.acquisition.lead_s },
		schedule: { ...d.schedule, levels_db: [...d.schedule.levels_db] },
		spacing: { ...d.spacing },
		identification: { ...d.identification },
		verdict: d.verdict,
		control: {
			law: { id: d.law.id, params: lawWireParams(law, d.law.params) },
			...d.control
		},
		simulator: { plant: d.simulator.plant },
		capture: { ...d.capture },
		metadata: d.metadata.map((m) => ({ ...m }))
	};
}

/** The draft for a configuration the service holds; what it leaves out takes the defaults. */
export function fromMsConfig(c: MimoShockConfig): MsDraft {
	const base = defaultMsSetup();
	const t = c.target;
	const n = c.channels.controls.length;
	const pulses: MsPulseDraft[] =
		t.kind === 'waveform'
			? t.pulses.map((p) => ({
					shape: p.shape,
					amplitude_g: p.amplitude_g,
					duration_ms: p.duration_ms,
					polarity: p.polarity ?? 'positive',
					rise_frac: p.rise_frac ?? 0.1,
					fall_frac: p.fall_frac ?? 0.1,
					compensation: p.compensation ? { ...p.compensation } : { ...defaultPulse().compensation }
				}))
			: Array.from({ length: n }, defaultPulse);
	const srsControls: MsSrsControlDraft[] =
		t.kind === 'srs'
			? t.controls.map((x) => ({
					points: x.target.kind === 'breakpoints' ? x.target.points.map(([f, a]) => ({ freq_hz: f, accel_g: a })) : [{ freq_hz: t.bands.f_lo_hz, accel_g: x.target.accel_g }, { freq_hz: t.bands.f_hi_hz, accel_g: x.target.accel_g }],
					engine: x.synthesis?.engine ?? 'wavsyn',
					max_iters: x.synthesis?.max_iters ?? 50,
					tol_db: x.synthesis?.tol_db ?? 1.5,
					max_gain_db_per_iter: x.synthesis?.max_gain_db_per_iter ?? 3,
					polarity: x.polarity ?? 'positive'
				}))
			: Array.from({ length: n }, defaultSrsControl);
	const timing = c.timing ?? { mode: 'free' };
	const { law: wireLaw, ...rest } = c.control ?? {};
	return {
		title: c.title,
		controls: c.channels.controls.map((x) => x.channel),
		notControls: [],
		drives: c.channels.drives.map((x) => ({ ...x })),
		limits: (c.channels.limits ?? []).map((x) => ({ ...x })),
		kind: t.kind,
		standard: t.kind === 'waveform' ? (t.standard ?? 'mil_std_810_516') : base.standard,
		pulses,
		srs:
			t.kind === 'srs'
				? {
						bands: { f_lo_hz: t.bands.f_lo_hz, f_hi_hz: t.bands.f_hi_hz, bands_per_octave: t.bands.bands_per_octave ?? 3, q: t.bands.q ?? 10 },
						tolerance_db: t.tolerance_db ? [t.tolerance_db[0], t.tolerance_db[1]] : base.srs.tolerance_db,
						controls: srsControls
					}
				: { ...base.srs, controls: srsControls },
		timing: {
			mode: timing.mode,
			window_ms: ms(timing.window_s ?? 0.002),
			delays_ms: timing.delays_s ? timing.delays_s.map(ms) : Array.from({ length: n }, () => 0)
		},
		verdict: c.verdict ?? 'all',
		acquisition: {
			fs_hz: c.acquisition?.fs_hz ?? base.acquisition.fs_hz,
			record_len: c.acquisition?.record_len ?? null,
			band_hz: c.acquisition?.band_hz ? [c.acquisition.band_hz[0], c.acquisition.band_hz[1]] : base.acquisition.band_hz,
			lead_s: c.acquisition?.lead_s ?? base.acquisition.lead_s
		},
		schedule: {
			mode: c.schedule.mode,
			levels_db: [...c.schedule.levels_db],
			max_pulses_per_level: c.schedule.max_pulses_per_level ?? base.schedule.max_pulses_per_level,
			full_level_pulses: c.schedule.full_level_pulses ?? base.schedule.full_level_pulses
		},
		spacing: { ...base.spacing, ...(c.spacing ?? {}) },
		identification: { ...base.identification, ...(c.identification ?? {}) },
		law: { id: wireLaw?.id ?? '', params: { ...((wireLaw?.params ?? {}) as LawDraft['params']) } },
		control: { ...base.control, ...rest },
		simulator: { plant: c.simulator?.plant ?? null },
		capture: { ...base.capture, ...(c.capture ?? {}) },
		metadata: (c.metadata ?? []).map((m) => ({ ...m }))
	};
}

// ---------------------------------------------------------------------------------------------
// Where a finding belongs

export type MsSection = 'channels' | 'target' | 'timing' | 'ladder' | 'law' | 'limits';

export const MS_SECTIONS: { id: MsSection; label: string }[] = [
	{ id: 'channels', label: 'Station & Channels' },
	{ id: 'target', label: 'Targets' },
	{ id: 'timing', label: 'Timing & Verdict' },
	{ id: 'ladder', label: 'Ladder & Identification' },
	{ id: 'law', label: 'Control Law' },
	{ id: 'limits', label: 'Limits & Aborts' }
];

/**
 * The editor's key for a service finding. A field path loses its `config.` head and its brackets;
 * a per-control target is the draft's (`target.pulses[1].amplitude_g` is
 * `pulses.1.amplitude_g`, `target.controls[2].target` is `srs.controls.2.points`); the law's path
 * is the law panel's; a finding on a channel is the channels'.
 */
export function msItemKey(item: Pick<ReportItem, 'field' | 'channel'> & { reason: string }): string {
	if (item.field) {
		const path = item.field.replace(/^config\./, '').replace(/\[(\d+)\]/g, '.$1');
		if (path.startsWith('control.law')) return path.replace(/^control\.law(\.id)?/, (_, id) => (id ? 'law.id' : 'law')).replace(/^law$/, 'law.id');
		if (path === 'target.pulses') return 'pulses';
		let m = /^target\.pulses\.(\d+)(\..*)?$/.exec(path);
		if (m) return `pulses.${m[1]}${m[2] ?? ''}`;
		if (path === 'target.controls') return 'srs.controls';
		m = /^target\.controls\.(\d+)(\..*)?$/.exec(path);
		if (m) return m[2]?.startsWith('.target') || !m[2] ? `srs.controls.${m[1]}.points` : `srs.controls.${m[1]}${m[2].replace(/^\.synthesis/, '')}`;
		if (path.startsWith('target.bands')) return path.replace(/^target\./, 'srs.');
		if (path.startsWith('target.tolerance_db')) return 'srs.tolerance_db';
		if (path.startsWith('target.standard')) return 'standard';
		if (path === 'target' || path === 'target.kind') return 'kind';
		if (path.startsWith('timing.window_s')) return 'timing.window_ms';
		m = /^timing\.delays_s(\.(\d+))?$/.exec(path);
		if (m) return m[2] ? `timing.delays_ms.${m[2]}` : 'timing.delays_ms';
		if (path.startsWith('schedule.levels_db')) return 'schedule.levels_db';
		if (path.startsWith('channels.controls')) return 'channels.controls';
		m = /^channels\.drives\.(\d+)\.(ceiling_vpk|abort_vpk)$/.exec(path);
		if (m) return `drives.${m[1]}.${m[2]}`;
		if (path.startsWith('channels.drives')) return 'channels.drives';
		m = /^channels\.limits\.(\d+)\.abort_peak$/.exec(path);
		if (m) return `limits.${m[1]}.abort_peak`;
		if (path.startsWith('channels')) return 'channels';
		return path;
	}
	if (item.channel !== undefined && item.channel !== null) return 'channels';
	return 'title';
}

export function msByKey(items: { field?: string | null; channel?: number | null; reason: string; message: string }[]): FieldErrors {
	const out: FieldErrors = {};
	for (const it of items) {
		const k = msItemKey(it as Pick<ReportItem, 'field' | 'channel'> & { reason: string });
		if (!(out[k] ??= []).includes(it.message)) out[k].push(it.message);
	}
	return out;
}

/** The section a key belongs to. */
export function msSectionOf(key: string): MsSection {
	if (/^(channels|drives|station|table|simulator|capture|metadata)(\.|$)/.test(key)) return 'channels';
	if (/^(title|kind|standard|pulses|srs)(\.|$)/.test(key)) return 'target';
	if (/^(timing|verdict)(\.|$)/.test(key)) return 'timing';
	if (/^(schedule|spacing|identification|acquisition)(\.|$)/.test(key)) return 'ladder';
	if (/^law(\.|$)/.test(key)) return 'law';
	if (/^(limits|control)(\.|$)/.test(key)) return 'limits';
	return 'target';
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

function needOptional(map: FieldErrors, key: string, v: number | null) {
	if (v !== null && !Number.isFinite(v)) add(map, key, NUMBER);
}

/**
 * What the editor can name before the service is asked: fields that hold no number, a title that
 * is empty, too few or too many controls or drives, a drive without its ceiling and abort (there
 * is no default: they are the amplifier inputs), a limit without its abort, levels that do not
 * rise to full level, a hold without its delays, and the global verdict on a pulse target.
 */
export function localErrors(d: MsDraft): FieldErrors {
	const out: FieldErrors = {};
	if (!d.title.trim()) add(out, 'title', 'Give the test a title.');
	if (!d.controls.length) add(out, 'channels.controls', 'Switch on at least one control row in the channel table.');
	else if (d.controls.length > MAX_CONTROLS) add(out, 'channels.controls', `At most ${MAX_CONTROLS} controls; this table has ${d.controls.length}.`);
	if (!d.drives.length) add(out, 'channels.drives', 'Switch on at least one drive row in the channel table.');
	else if (d.drives.length > MAX_DRIVES) add(out, 'channels.drives', `At most ${MAX_DRIVES} drives; this table has ${d.drives.length}.`);
	d.drives.forEach((x, i) => {
		if (!Number.isFinite(x.ceiling_vpk)) add(out, `drives.${i}.ceiling_vpk`, "Give the drive's ceiling in volts peak. There is no default: it is your amplifier input.");
		else if (x.ceiling_vpk <= 0 || x.ceiling_vpk > 10) add(out, `drives.${i}.ceiling_vpk`, 'The ceiling is above 0 and at most 10 V pk.');
		if (!Number.isFinite(x.abort_vpk)) add(out, `drives.${i}.abort_vpk`, "Give the drive's abort in volts peak, above its ceiling.");
		else if (Number.isFinite(x.ceiling_vpk) && x.abort_vpk <= x.ceiling_vpk) add(out, `drives.${i}.abort_vpk`, 'The abort is above the ceiling.');
		else if (x.abort_vpk > 10) add(out, `drives.${i}.abort_vpk`, 'The abort is at most 10 V pk.');
	});
	d.limits.forEach((l, i) => {
		if (!Number.isFinite(l.abort_peak)) add(out, `limits.${i}.abort_peak`, 'Give the peak this limit stops the run at.');
		else if (l.abort_peak <= 0) add(out, `limits.${i}.abort_peak`, 'The abort peak is above 0.');
	});
	if (d.kind === 'waveform') {
		d.pulses.forEach((p, i) => {
			need(out, `pulses.${i}.amplitude_g`, p.amplitude_g);
			need(out, `pulses.${i}.duration_ms`, p.duration_ms);
			if (p.shape === 'trapezoid') {
				need(out, `pulses.${i}.rise_frac`, p.rise_frac);
				need(out, `pulses.${i}.fall_frac`, p.fall_frac);
			}
			need(out, `pulses.${i}.compensation.pre_frac`, p.compensation.pre_frac);
			if (!p.compensation.symmetric) need(out, `pulses.${i}.compensation.post_frac`, p.compensation.post_frac);
		});
		if (d.verdict === 'global') add(out, 'verdict', 'The global verdict judges an SRS target; choose every control for a pulse target.');
	} else {
		const b = d.srs.bands;
		for (const k of ['f_lo_hz', 'f_hi_hz', 'q'] as const) need(out, `srs.bands.${k}`, b[k]);
		need(out, 'srs.bands.bands_per_octave', b.bands_per_octave, true);
		const [lo, hi] = d.srs.tolerance_db;
		if (!Number.isFinite(lo) || !Number.isFinite(hi)) add(out, 'srs.tolerance_db', 'Give both edges of the tolerance.');
		else if (!(lo < 0 && hi > 0)) add(out, 'srs.tolerance_db', 'The tolerance runs from below 0 dB to above it.');
		d.srs.controls.forEach((c, i) => {
			if (c.points.length < 2) add(out, `srs.controls.${i}.points`, 'Give at least two points.');
			c.points.forEach((p, j) => {
				need(out, `srs.controls.${i}.points.${j}.freq_hz`, p.freq_hz);
				need(out, `srs.controls.${i}.points.${j}.accel_g`, p.accel_g);
				if (j > 0 && Number.isFinite(p.freq_hz) && p.freq_hz <= c.points[j - 1].freq_hz) add(out, `srs.controls.${i}.points.${j}.freq_hz`, 'Frequencies rise from point to point.');
			});
			need(out, `srs.controls.${i}.max_iters`, c.max_iters, true);
			need(out, `srs.controls.${i}.tol_db`, c.tol_db);
			need(out, `srs.controls.${i}.max_gain_db_per_iter`, c.max_gain_db_per_iter);
		});
	}
	need(out, 'timing.window_ms', d.timing.window_ms);
	if (d.timing.mode === 'hold') {
		if (d.timing.delays_ms.length !== d.controls.length) add(out, 'timing.delays_ms', 'Give one delay for every control.');
		d.timing.delays_ms.forEach((v, i) => need(out, `timing.delays_ms.${i}`, v));
	}
	need(out, 'acquisition.fs_hz', d.acquisition.fs_hz);
	if (!(Number.isFinite(d.acquisition.band_hz[0]) && Number.isFinite(d.acquisition.band_hz[1]))) add(out, 'acquisition.band_hz', 'Give both edges of the band.');
	need(out, 'acquisition.lead_s', d.acquisition.lead_s);
	const s = d.schedule;
	if (!s.levels_db.length) add(out, 'schedule.levels_db', 'Give at least one level.');
	else if (s.levels_db.some((v) => !Number.isFinite(v))) add(out, 'schedule.levels_db', 'Every level is a number, in dB.');
	else if (s.levels_db.some((v, i) => i > 0 && v <= s.levels_db[i - 1])) add(out, 'schedule.levels_db', 'The levels rise from one to the next.');
	else if (Math.abs(s.levels_db[s.levels_db.length - 1]) > 1e-9) add(out, 'schedule.levels_db', 'The last level is full level, 0 dB.');
	need(out, 'schedule.max_pulses_per_level', s.max_pulses_per_level, true);
	need(out, 'schedule.full_level_pulses', s.full_level_pulses, true);
	for (const k of ['cooling_s', 'settling_s', 'compute_s'] as const) need(out, `spacing.${k}`, d.spacing[k]);
	need(out, 'identification.drive_vrms', d.identification.drive_vrms);
	need(out, 'identification.coherence_floor', d.identification.coherence_floor);
	if (d.identification.frames !== null) need(out, 'identification.frames', d.identification.frames, true);
	const c = d.control;
	for (const k of ['law_budget', 'max_growth_db', 'max_band_growth_db', 'divergence_drive_ratio', 'first_drive_ratio'] as const) need(out, `control.${k}`, c[k]);
	for (const k of ['law_misses', 'max_pulses', 'divergence_pulses'] as const) need(out, `control.${k}`, c[k], true);
	for (const k of ['abort_db', 'max_velocity_ips', 'max_displacement_in'] as const) needOptional(out, `control.${k}`, c[k]);
	if (d.capture.auto_start && !d.capture.run_name.trim()) add(out, 'capture.run_name', 'Name the capture, or turn auto start off.');
	return out;
}

/** "Levels in dB" as the operator types them: "-12, -9, -6" into numbers (NaN where not one). */
export function parseLevels(text: string): number[] {
	return text
		.split(/[,\s;]+/)
		.filter((x) => x.trim() !== '')
		.map((x) => Number(x));
}
