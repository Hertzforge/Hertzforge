/**
 * MIMO Mixed Mode in mock mode: a made-up run that walks every state the screens show. Configure
 * and its dry run check what the wire names (`$lib/protocol/mimoMixed`); the reference is built
 * from the breakpoint tables; Start identifies and holds for review with the gate, the prediction
 * and the budget forecast; Proceed plays the floor alone until it equalizes, then brings the tones
 * and narrowbands in and climbs the level schedule. Every number is made up so that every panel
 * can be seen: it is not a measurement and nothing here controls anything.
 *
 * The story: at full level the article softens under the first tone, its drive on the first
 * shaker climbs, and that shaker runs out of room; the ceiling rule scales the tones down (every
 * tone by one factor under `common_scale`, the last tone under `in_turn`) and the tones show as
 * limiting until the run completes and ramps down. The run's clock is compressed: identification
 * takes a second and a half, the floor a few seconds to equalize, each level a few seconds.
 */

import type { ChannelRow, DataSource, LogEntry, RefusalReason, RunRecordRef, Series, SpectrumView, StationRef, View } from '../types';
import { packedIndex, type LawEntry, type MimoBuildBreakpointsParams, type MimoReferenceResult } from '../mimo';
import type { MixedPartKind, MixedSchedule } from '../mixed';
import type {
	MimoMixedConfig,
	MimoMixedDerived,
	MimoMixedDriveRowLive,
	MimoMixedEnd,
	MimoMixedLiveHead,
	MimoMixedOutcome,
	MimoMixedPartRow,
	MimoMixedPhase,
	MimoMixedPrediction,
	MimoMixedQuantity,
	MimoMixedReportItem,
	MimoMixedRunState,
	MimoMixedState,
	MimoMixedSubscribePlot,
	MimoMixedBandPart,
	MimoMixedTonePart
} from '../mimoMixed';
import { MockError, type MockConnection, type MockMethod } from './socket';

export interface MimoMixedMockHost {
	speed: number;
	timeoutS: number;
	own(conn: MockConnection): void;
	hasController(): boolean;
	otherRun(): string | null;
	broadcast(topic: string, data: Record<string, unknown>): void;
	rows(): ChannelRow[];
	station(): { source: DataSource; ref: StationRef };
	clientName(conn: MockConnection): string;
	record?: 'unwritable' | 'problem';
	recordFolder?(runId: string): string;
}

export interface MimoMixedMock {
	methods: Record<string, MockMethod>;
	isLive(): boolean;
	liveId(): string | null;
	detach(conn: MockConnection): void;
	seatExpired(): void;
	/** The floor's laws, world `mimo`. */
	floorLaws(): LawEntry[];
}

const TICK_MS = 50;
const LIVE_MS = 200;
const SYSID_S = 1.5;
const FLOOR_S = 3;
/** Mock seconds per level below full, and at full level. */
const LEVEL_S = 3;
const FULL_S = 10;
/** Parts seconds per mock second: windows and sweeps move faster than the mock's clock. */
const PARTS_RATE = 10;
const GRID_POINTS = 256;
const HISTORY = 120;
const LIVE: MimoMixedRunState[] = ['armed', 'running', 'review', 'stopping'];
/** The tone tracker's delay is about 0.625 / bandwidth (1.25 s at 0.5 Hz). */
const TRACKER_DELAY = 0.625;

function refuse(code: 'refused' | 'bad_request', reason: string, message: string, detail?: Record<string, unknown>): never {
	throw new MockError(code, message, reason as RefusalReason, detail);
}

const round = (v: number, d = 3) => Math.round(v * 10 ** d) / 10 ** d;
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const amp = (db: number) => 10 ** (db / 20);
const ser = (name: string, role: Series['role'], x: ArrayLike<number>, y: ArrayLike<number>): Series => ({ name, role, x: Float64Array.from(x), y: Float64Array.from(y) });
const listOf = <T>(v: T | T[], n: number): T[] => (Array.isArray(v) ? v : Array.from({ length: n }, () => v));

// ---------------------------------------------------------------------------------------------
// The floor's law catalog, world `mimo`

function floorEntry(id: string, name: string, summary: string, params: LawEntry['params']): LawEntry {
	return {
		id,
		world: 'mimo',
		builtin: true,
		name,
		version: '0.1.0',
		author: 'Hertzforge',
		summary,
		sha256: null,
		check: { status: 'built_in', date: null, tool_version: null, cases: [], hardware: true, message: 'Built in: cleared for hardware.' },
		params,
		diagnostics: [{ key: 'step', label: 'Step', unit: '', help: 'The share of the correction taken on the last loop.', layout: 'scalar' }],
		shapes: ['square', 'more_controls_than_drives', 'fewer_controls_than_drives'],
		needs: null
	};
}

const FLOOR_LAWS: LawEntry[] = [
	floorEntry('pinv_closed', 'Pseudo-inverse, closed loop', "Corrects the drive matrix each loop through the model's regularized inverse, by the measured error at the controls.", [
		{ key: 'step', label: 'Step', help: 'The share of the correction taken each loop, 0 to 1.', kind: 'number', unit: '', min: 0.05, max: 1, step: 0.05, default: 0.5 }
	]),
	floorEntry('pinv_open', 'Pseudo-inverse, open loop', "Plays the model's inverse of the reference and makes no correction. For a check of the model.", [])
];

// ---------------------------------------------------------------------------------------------
// The run

interface PartRun {
	id: string;
	kind: MixedPartKind;
	name: string;
	enabled: boolean;
	ramp: number;
	schedule: MixedSchedule | null;
	window: { on_s: number; off_s: number | null };
	/** Per control: g peak (a tone) or the target multiplier on the floor (a band). */
	target: number[];
	phase: number[];
	alarm: [number, number];
	freq: number | null;
	direction: 'up' | 'down';
	sweeps: number;
	err: number[];
	scale: number;
	limiting: boolean;
	ampHist: number[][];
	phHist: number[][];
	levelHist: number[][];
}

interface Run {
	id: string;
	config: MimoMixedConfig;
	derived: MimoMixedDerived;
	state: MimoMixedRunState;
	phase: MimoMixedPhase;
	source: DataSource;
	station: StationRef;
	n: number;
	m: number;
	/** The floor's PSD per control on the display grid at full level, and the grid. */
	freqs: number[];
	psd: number[][];
	levels: number[];
	rung: number;
	levelDb: number | null;
	t: number;
	phaseT: number;
	partsT: number | null;
	atLevel: number;
	held: boolean;
	floorErr: number[];
	floorEqualized: boolean;
	parts: PartRun[];
	scale: number | null;
	softening: number;
	gates: MimoMixedState['gates'];
	prediction: MimoMixedPrediction | null;
	excluded: number[];
	override: MimoMixedState['override'];
	end: MimoMixedEnd | null;
	stopping: number | null;
	timer: ReturnType<typeof setInterval> | null;
	record: RunRecordRef | null;
	histT: number[];
	logged: Set<string>;
}

/** The PSD of a breakpoint table at `f`: log-log between anchors, slopes from the anchor before. */
function tablePsd(rows: { freq_hz: number; psd_g2_hz?: number; slope_db_oct?: number }[], f: number): number {
	const pts: [number, number][] = [];
	for (const r of rows) {
		if (r.psd_g2_hz !== undefined) pts.push([r.freq_hz, r.psd_g2_hz]);
		else if (pts.length && r.slope_db_oct !== undefined) {
			const [f0, p0] = pts[pts.length - 1];
			pts.push([r.freq_hz, p0 * 10 ** ((r.slope_db_oct * Math.log2(r.freq_hz / f0)) / 10)]);
		}
	}
	if (!pts.length || f < pts[0][0] || f > pts[pts.length - 1][0]) return 0;
	for (let i = 1; i < pts.length; i++) {
		const [fa, pa] = pts[i - 1];
		const [fb, pb] = pts[i];
		if (f <= fb) return pa * (pb / pa) ** (Math.log(f / fa) / Math.log(fb / fa));
	}
	return pts[pts.length - 1][1];
}

/** A packed reference's diagonal at each control, on at most GRID_POINTS lines of its band. */
function diagonalOf(c: MimoMixedConfig): { freqs: number[]; psd: number[][] } {
	const r = c.reference;
	const n = c.channels.controls.length;
	if (r.kind !== 'inline' || !r.freqs_hz.length) return { freqs: [], psd: Array.from({ length: n }, () => []) };
	const per = (r.n * (r.n + 1)) / 2;
	const stride = Math.max(1, Math.ceil(r.freqs_hz.length / GRID_POINTS));
	const freqs: number[] = [];
	const psd: number[][] = Array.from({ length: n }, () => []);
	for (let l = 0; l < r.freqs_hz.length; l += stride) {
		freqs.push(r.freqs_hz[l]);
		for (let i = 0; i < n; i++) psd[i].push(i < r.n ? r.re[l * per + packedIndex(r.n, i, i)] : 0);
	}
	return { freqs, psd };
}

function at(freqs: number[], ys: number[], f: number): number {
	if (!freqs.length) return 0;
	let k = 0;
	while (k < freqs.length - 1 && freqs[k + 1] <= f) k++;
	return ys[k] ?? 0;
}

function scheduleSpan(s: MixedSchedule | null): [number, number] | null {
	if (!s) return null;
	if (s.kind === 'fixed') return [s.freq_hz, s.freq_hz];
	if (s.kind === 'sweep') return [s.band_hz[0], s.band_hz[1]];
	return null;
}

export function createMimoMixedMock(host: MimoMixedMockHost): MimoMixedMock {
	let run: Run | null = null;
	let runCount = 0;
	let log: LogEntry[] = [];
	let lastKey = '';
	const subs = new Map<MockConnection, { rate: number; plots: MimoMixedSubscribePlot[] }>();
	let liveTimer: ReturnType<typeof setInterval> | null = null;
	const isLive = () => run !== null && LIVE.includes(run.state);

	function addLog(kind: string, text: string) {
		const e = { seq: log.length, at_unix: Date.now() / 1000, kind, text } as LogEntry;
		log.push(e);
		if (run) host.broadcast('mimo_mixed.log', { run_id: run.id, entries: [e] });
	}

	// ---- checking a configuration ---------------------------------------------------------------

	function item(reason: MimoMixedReportItem['reason'], message: string, where: Partial<MimoMixedReportItem> = {}): MimoMixedReportItem {
		return { reason, message, ...where };
	}

	function check(c: MimoMixedConfig): { errors: MimoMixedReportItem[]; warnings: MimoMixedReportItem[]; derived: MimoMixedDerived | null; views: View[] } {
		const errors: MimoMixedReportItem[] = [];
		const warnings: MimoMixedReportItem[] = [];
		const n = c.channels.controls.length;
		const m = c.channels.drives.length;
		const rows = host.rows();
		const label = (ch: number) => rows.find((r) => r.index === ch)?.description || `Channel ${ch}`;
		if (!n) errors.push(item('bad_config', 'Give at least one control.', { field: 'channels.controls' }));
		if (n > 16) errors.push(item('too_many_controls', 'At most 16 controls.', { field: 'channels.controls' }));
		if (!m) errors.push(item('bad_config', 'Give at least one drive.', { field: 'channels.drives' }));
		if (m > 8) errors.push(item('too_many_drives', 'At most 8 drives.', { field: 'channels.drives' }));
		if ((c.channels.limits?.length ?? 0) > 8) errors.push(item('too_many_limits', 'At most 8 limit channels.', { field: 'channels.limits' }));
		c.channels.drives.forEach((d, j) => {
			if (!(d.ceiling_vrms > 0)) errors.push(item('bad_config', 'The rms ceiling is above 0 V.', { drive: j, field: `channels.drives.${j}` }));
			if (!(d.peak_v >= d.ceiling_vrms)) errors.push(item('bad_config', 'The peak ceiling is at least the rms ceiling.', { drive: j, field: `channels.drives.${j}` }));
		});
		if (c.reference.kind === 'inline' && c.reference.n !== n) errors.push(item('reference_invalid', `The reference is for ${c.reference.n} controls; the setup has ${n}. Build it again.`, { field: 'reference' }));
		if (c.control?.frf && c.control.frf !== 'hold') errors.push(item('frf_update_not_built', 'An updated model during the run is not built: the model is held from the identification.', { field: 'control.frf' }));
		const law = c.law ?? {};
		if (law.bands && !['auto', 'scalar_servo', 'per_control'].includes(law.bands.id)) errors.push(item('unknown_band_law', `No band law "${law.bands.id}".`, { field: 'law.bands' }));
		if (law.ceiling_rule && !['common_scale', 'in_turn'].includes(law.ceiling_rule.id)) errors.push(item('unknown_ceiling_rule', `No ceiling rule "${law.ceiling_rule.id}".`, { field: 'law.ceiling_rule' }));
		if (law.fill_rule && !['no_error', 'model_fill'].includes(law.fill_rule.id)) errors.push(item('unknown_fill_rule', `No fill rule "${law.fill_rule.id}".`, { field: 'law.fill_rule' }));
		if (law.floor && !FLOOR_LAWS.some((l) => l.id === law.floor?.id)) errors.push(item('law_unknown', `The floor's law "${law.floor.id}" is not in the catalog.`, { field: 'law.floor.id' }));
		const parts = c.parts ?? [];
		const floors = parts.filter((p) => p.kind === 'floor');
		if (!floors.length) errors.push(item('no_floor', 'The parts have no floor.', { field: 'parts' }));
		const tones = parts.filter((p): p is MimoMixedTonePart => p.kind === 'tone');
		const bands = parts.filter((p): p is MimoMixedBandPart => p.kind === 'narrowband');
		if (tones.length > 12) errors.push(item('too_many_tones', 'At most 12 tones.', { field: 'parts' }));
		if (bands.length > 12) errors.push(item('too_many_bands', 'At most 12 narrowbands.', { field: 'parts' }));
		const { freqs } = diagonalOf(c);
		const band: [number, number] = freqs.length ? [freqs[0], freqs[freqs.length - 1]] : [20, 2000];
		const ids = new Set<string>();
		for (const p of parts) {
			if (ids.has(p.id)) errors.push(item('duplicate_part_id', `Two parts have the id "${p.id}".`, { part: p.id }));
			ids.add(p.id);
			if (!['floor', 'tone', 'narrowband'].includes(p.kind)) errors.push(item('unknown_part_kind', `No part kind "${(p as { kind: string }).kind}".`, { part: p.id }));
		}
		const count = (part: string, field: string, v: unknown) => {
			if (Array.isArray(v) && v.length !== n) errors.push(item('wrong_control_count', `${field}: ${v.length} values for ${n} controls.`, { part, detail: { field, expected: n, got: v.length } }));
		};
		const tracking = c.tracking_hz ?? 0.5;
		for (const p of [...tones, ...bands]) {
			const span = scheduleSpan(p.schedule);
			if (p.schedule.kind === 'follow' && !ids.has(p.schedule.of)) errors.push(item('follow_unknown_part', `Follows "${p.schedule.of}", which is not a part.`, { part: p.id }));
			if (span && (span[0] < band[0] || span[1] > band[1])) errors.push(item('part_out_of_band', `Plays from ${round(span[0], 1)} to ${round(span[1], 1)} Hz, outside the floor's band, ${round(band[0], 1)} to ${round(band[1], 1)} Hz.`, { part: p.id }));
			if (p.schedule.kind === 'sweep' && p.kind === 'tone') {
				const s = p.schedule;
				const hzps = s.rate.law === 'linear' ? s.rate.hz_per_s : (s.band_hz[1] * Math.LN2 * s.rate.oct_per_min) / 60;
				if (hzps * (TRACKER_DELAY / tracking) > 2 * tracking)
					warnings.push(item('sweep_beyond_tracker', `At its fastest it moves ${round(hzps, 2)} Hz/s; the tracker lags about ${round(TRACKER_DELAY / tracking, 2)} s at ${tracking} Hz, so through a resonance it can lose the tone and abort.`, { part: p.id }));
			}
		}
		for (const t of tones) {
			count(t.id, 'amplitude', t.amplitude);
			if (t.phases_stated) count(t.id, 'phase_deg', t.phase_deg);
		}
		for (const b of bands) {
			const s = b.statement;
			if (s.kind === 'matrix') errors.push(item('band_matrix_not_built', 'A band stated as a full matrix is not built: state it as a raise or an added level.', { part: b.id }));
			else if (s.kind === 'raise') {
				count(b.id, 'statement.db', s.db);
				listOf(s.db, n).forEach((db, i) => {
					if (db < 0) errors.push(item('band_not_additive', `A raise of ${db} dB lowers the floor at this control: a band adds to the floor, it does not take from it.`, { part: b.id, control: i, detail: { line: 0, min_eigenvalue_rel: round(1 - amp(db * 2), 3) } }));
				});
			} else {
				count(b.id, 'statement.level_g2_hz', s.level_g2_hz);
				if ((s.coherence ?? 0) < 0 || (s.coherence ?? 0) > 1) errors.push(item('band_not_additive', 'The coherence is from 0 to 1.', { part: b.id }));
			}
		}
		if (errors.length) return { errors, warnings, derived: null, views: [] };
		const fs = c.estimation?.fs_hz ?? 4096;
		const npts = c.estimation?.n_pts ?? 2048;
		const lawFacts = (id: string, live: string[]) => ({ id, name: id, version: '0.1.0', sha256: null, live, hardware: true });
		const levels = c.schedule.levels.map((l) => l.db);
		const test = c.schedule.mode === 'manual' ? null : c.schedule.levels.slice(0, -1).reduce((s, l) => s + (l.dwell_s ?? 0), 0) + c.schedule.full_level_s;
		void levels;
		const derived: MimoMixedDerived = {
			band_hz: [round(band[0], 2), round(band[1], 2)],
			grid: { fs_hz: fs, n_pts: npts, df_hz: round(fs / npts, 4), lines: freqs.length, loop_s: round(npts / fs, 3), tick_s: 0.01, actual: false },
			control_labels: c.channels.controls.map((x) => label(x.channel)),
			drive_labels: c.channels.drives.map((x) => label(x.channel)),
			parts: parts.map((p) => ({
				id: p.id,
				kind: p.kind,
				name: p.name ?? p.id,
				freq_hz: p.kind === 'floor' ? null : scheduleSpan(p.schedule),
				peak_g: p.kind === 'tone' ? listOf(p.amplitude, n).map((a) => (a.kind === 'constant' ? a.g_pk : Math.max(...a.rows.map((r) => r.value)))) : null,
				band_law: p.kind === 'narrowband' ? (law.bands?.id === 'scalar_servo' || (law.bands?.id !== 'per_control' && p.statement.kind === 'raise') ? 'scalar_servo' : 'per_control') : null
			})),
			floor_equalize_s: 12,
			identification_s: round(((c.identification?.frames ?? 40) * npts) / fs, 1),
			test_s: test,
			tracker_delay_s: round(TRACKER_DELAY / tracking, 3),
			law: {
				floor: lawFacts(law.floor?.id ?? 'pinv_closed', []),
				tones: lawFacts(law.tones?.id ?? 'matrix', ['carry']),
				bands: law.bands?.id ?? 'auto',
				ceiling_rule: law.ceiling_rule?.id ?? 'common_scale',
				fill_rule: law.fill_rule?.id ?? 'no_error'
			}
		};
		return { errors, warnings, derived, views: [] };
	}

	// ---- the reference --------------------------------------------------------------------------

	function buildReference(p: MimoBuildBreakpointsParams): MimoReferenceResult {
		const fs = p.grid.fs_hz;
		const npts = p.grid.n_pts;
		const df = fs / npts;
		const n = p.tables.length;
		const errors: MimoReferenceResult['errors'] = [];
		p.tables.forEach((t, i) => {
			if (t.rows.length < 2) errors.push({ reason: 'out_of_range', message: `${t.label}: give at least two rows.`, row: 0, field: `tables.${i}` } as never);
			if (t.rows.some((r) => r.psd_g2_hz !== undefined && !(r.psd_g2_hz > 0))) errors.push({ reason: 'out_of_range', message: `${t.label}: every PSD is above 0.`, field: `tables.${i}` } as never);
		});
		const lo = Math.min(...p.tables.map((t) => t.rows[0]?.freq_hz ?? Infinity));
		const hi = Math.max(...p.tables.map((t) => t.rows[t.rows.length - 1]?.freq_hz ?? 0));
		if (!(hi > lo) || hi > fs / 2.56) errors.push({ reason: 'out_of_range', message: `The band reaches ${hi} Hz; at ${fs} Hz the highest line is ${round(fs / 2.56, 1)} Hz.` } as never);
		const base = { labels: p.tables.map((t) => t.label), unit: p.unit ?? 'g', df_hz: df, warnings: [], transforms: { response: null, drive: null }, provenance: null };
		const psd0 = { violations: 0, min_eigenvalue_ratio: 0, worst_hz: null, lines_hz: [], repair: null, repair_accepted: false };
		if (errors.length) return { ok: false, reference: null, band_hz: null, lines: 0, errors, psd: psd0, views: [], ...base };
		const freqs: number[] = [];
		for (let f = Math.ceil(lo / df) * df; f <= hi + 1e-9; f += df) freqs.push(round(f, 4));
		const per = (n * (n + 1)) / 2;
		const re = new Array<number>(freqs.length * per).fill(0);
		const im = new Array<number>(freqs.length * per).fill(0);
		const coh = new Map((p.pairs ?? []).map((q) => [`${q.row}-${q.col}`, q]));
		freqs.forEach((f, l) => {
			const g = p.tables.map((t) => tablePsd(t.rows, f));
			for (let r = 0; r < n; r++)
				for (let c = r; c < n; c++) {
					const k = l * per + packedIndex(n, r, c);
					if (r === c) re[k] = g[r];
					else {
						const q = coh.get(`${r}-${c}`);
						const gamma = q ? Math.sqrt(typeof q.coherence === 'number' ? q.coherence : 0) : 0;
						const ph = ((q && typeof q.phase_deg === 'number' ? q.phase_deg : 0) * Math.PI) / 180;
						const mag = gamma * Math.sqrt(g[r] * g[c]);
						re[k] = mag * Math.cos(ph);
						im[k] = mag * Math.sin(ph);
					}
				}
		});
		const stride = Math.max(1, Math.ceil(freqs.length / GRID_POINTS));
		const xs = freqs.filter((_, i) => i % stride === 0);
		const views: View[] = [
			{
				view: 'spectrum',
				title: 'Reference PSDs',
				x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
				y: { label: 'PSD', unit: `${p.unit ?? 'g'}²/Hz`, scale: 'log' },
				series: p.tables.map((t, i) => (ser(t.label, i === 0 ? 'reference' : 'other', xs, xs.map((f) => tablePsd(t.rows, f)))) as Series)
			} as SpectrumView
		];
		return {
			ok: true,
			reference: { kind: 'inline', freqs_hz: freqs, n, re, im },
			band_hz: [freqs[0], freqs[freqs.length - 1]],
			lines: freqs.length,
			errors: [],
			psd: { ...psd0, lines_hz: [] },
			views,
			...base
		} as MimoReferenceResult;
	}

	// ---- the state ------------------------------------------------------------------------------

	const levelAmp = (r: Run) => (r.levelDb === null ? 0 : amp(r.levelDb));
	const driving = (r: Run) => r.state === 'running' && ['floor_only', 'engaged', 'full_level'].includes(r.phase);
	const partsIn = (r: Run) => r.state === 'running' && (r.phase === 'engaged' || r.phase === 'full_level');

	function floorRms(r: Run, i: number): number {
		const ys = r.psd[i] ?? [];
		let s = 0;
		for (let k = 1; k < ys.length; k++) s += ys[k] * (r.freqs[k] - r.freqs[k - 1]);
		return Math.sqrt(s);
	}

	function partRow(r: Run, p: PartRun): MimoMixedPartRow {
		const la = levelAmp(r);
		const tone = p.kind === 'tone';
		const controls = Array.from({ length: r.n }, (_, i) => {
			if (p.kind === 'floor') {
				const t = driving(r) ? floorRms(r, i) * la : null;
				const e = r.floorErr[i];
				return { target: t === null ? null : round(t, 4), measured: t === null ? null : round(t * amp(e), 4), error_db: t === null ? null : round(e, 2), alarm: driving(r) && Math.abs(e) > 3 };
			}
			const on = partsIn(r) && p.ramp > 0;
			const base = tone ? p.target[i] : at(r.freqs, r.psd[i] ?? [], p.freq ?? 0) * p.target[i];
			const t = on ? base * (tone ? la * p.ramp : la * la * p.ramp) : null;
			const e = p.err[i] + (tone && p.limiting ? 20 * Math.log10(Math.max(p.scale, 1e-3)) : 0);
			return { target: t === null ? null : round(t, 5), measured: t === null ? null : round(t * amp(tone ? e : e / 2), 5), error_db: t === null ? null : round(e, 2), alarm: t !== null && Math.abs(e) > p.alarm[1] };
		});
		return {
			id: p.id,
			kind: p.kind,
			name: p.name,
			enabled: p.enabled,
			on: p.kind === 'floor' ? driving(r) : partsIn(r) && p.ramp > 0,
			ramp: round(p.kind === 'floor' ? (driving(r) ? 1 : 0) : p.ramp, 3),
			freq_hz: p.freq === null ? null : round(p.freq, 2),
			direction: p.schedule?.kind === 'sweep' ? p.direction : null,
			sweeps: p.schedule?.kind === 'sweep' ? { done: p.sweeps, of: p.schedule.end?.sweeps ?? null } : null,
			width_hz: p.kind === 'narrowband' ? round(widthOf(r, p), 2) : null,
			held: r.held && p.kind !== 'floor',
			limiting: tone && p.limiting,
			scale: tone ? round(p.scale, 3) : null,
			unit: p.kind === 'narrowband' ? 'g²/Hz' : 'g',
			controls,
			band_law: p.kind === 'narrowband' ? (r.derived.parts.find((x) => x.id === p.id)?.band_law ?? null) : null,
			follows: p.schedule?.kind === 'follow' ? { of: p.schedule.of, ratio: p.schedule.ratio } : null
		};
	}

	function widthOf(r: Run, p: PartRun): number {
		const b = r.config.parts.find((x) => x.id === p.id) as MimoMixedBandPart | undefined;
		if (!b) return 0;
		return b.width.kind === 'hz' ? b.width.hz : b.width.share * (p.freq ?? 0);
	}

	/** The tones' drive per drive at full level, volts peak per g, and the softening on the first. */
	function toneGain(r: Run, j: number): number {
		return 1.2 * (1 + 0.15 * j) * (j === 0 ? 1 + r.softening : 1);
	}

	function drivesOf(r: Run): MimoMixedDriveRowLive[] {
		const la = levelAmp(r);
		const crest = r.config.control?.crest_factor ?? 3;
		return r.config.channels.drives.map((d, j) => {
			const rms = driving(r) ? 0.25 * (1 + 0.1 * j) * la * (1 + 0.5 * Math.max(0, ...r.floorErr) / 6) : 0;
			const toneSum = partsIn(r) ? r.parts.filter((p) => p.kind === 'tone').reduce((s, p) => s + toneGain(r, j) * Math.max(...p.target, 0) * p.ramp * p.scale * la, 0) : 0;
			return {
				channel: d.channel,
				random_rms_v: round(rms, 4),
				random_peak_v: round(rms * crest, 4),
				tone_sum_v: round(toneSum, 4),
				ceiling_v: d.peak_v,
				headroom_v: round(Math.max(0, d.peak_v - rms * crest), 4)
			};
		});
	}

	function stateOf(r: Run | null): MimoMixedState {
		if (!r) {
			return {
				run_id: null, state: 'unconfigured', phase: 'idle', title: null, source: null, station: null, grid: null, clock: null, shape: null, mode: null, levels_db: null, level_db: null,
				full_level_s: null, t_s: null, elapsed_s: null, parts_s: null, at_level_s: null, remaining_at_level_s: null, held: false, alarm: false, limiting: false, floor_error_db: [],
				floor_equalized: false, scale: null, parts: [], drives: [], gates: null, prediction: null, law: null, excluded: [], override: null, end: null
			};
		}
		const started = r.state !== 'configured';
		const rows = r.parts.map((p) => partRow(r, p));
		const lawRef = (id: string) => ({ id, name: id, version: '0.1.0', sha256: null, misses: 0, capped: 0, live: {} });
		const full = r.levels.length - 1;
		return {
			run_id: r.id,
			state: r.state,
			phase: r.phase,
			title: r.config.title ?? null,
			source: r.source,
			station: r.station,
			grid: { ...r.derived.grid, actual: started },
			clock: started ? 'shared' : null,
			shape: { controls: r.n, drives: r.m, labels: r.derived.control_labels, drive_labels: r.derived.drive_labels },
			mode: r.config.schedule.mode,
			levels_db: r.levels,
			level_db: r.levelDb,
			full_level_s: r.config.schedule.full_level_s,
			t_s: started ? round(r.t, 2) : null,
			elapsed_s: started ? round(r.t, 2) : null,
			parts_s: r.partsT === null ? null : round(r.partsT, 1),
			at_level_s: driving(r) ? round(r.atLevel, 1) : null,
			remaining_at_level_s: driving(r) ? round(Math.max(0, (r.rung === full ? FULL_S : LEVEL_S) - r.atLevel), 1) : null,
			held: r.held,
			alarm: rows.some((p) => p.controls.some((c) => c.alarm)),
			limiting: rows.some((p) => p.limiting),
			floor_error_db: driving(r) ? r.floorErr.map((e) => round(e, 2)) : [],
			floor_equalized: r.floorEqualized,
			scale: partsIn(r) && (r.config.law?.ceiling_rule?.id ?? 'common_scale') === 'common_scale' ? round(r.scale ?? 1, 3) : null,
			parts: rows,
			drives: drivesOf(r),
			gates: r.gates,
			prediction: r.prediction,
			law: {
				floor: lawRef(r.config.law?.floor?.id ?? 'pinv_closed'),
				tones: lawRef(r.config.law?.tones?.id ?? 'matrix'),
				bands: r.config.law?.bands?.id ?? 'auto',
				ceiling_rule: r.config.law?.ceiling_rule?.id ?? 'common_scale',
				fill_rule: r.config.law?.fill_rule?.id ?? 'no_error'
			},
			excluded: r.excluded,
			override: r.override,
			end: r.end,
			...(r.record ? { record: { ...r.record } } : {})
		};
	}

	function stateChanged(force = false) {
		const s = stateOf(run);
		const key = JSON.stringify([s.state, s.phase, s.level_db, s.held, s.limiting, s.alarm, s.floor_equalized, s.parts.map((p) => [p.enabled, p.on]), s.end, s.excluded]);
		if (!force && key === lastKey) return;
		lastKey = key;
		host.broadcast('mimo_mixed.state', s as unknown as Record<string, unknown>);
	}

	// ---- the story ------------------------------------------------------------------------------

	function identify(r: Run) {
		const coherence = Array.from({ length: r.n }, (_, i) => round(0.93 - 0.04 * i, 3));
		const floor = r.config.identification?.coherence_floor ?? 0.5;
		r.gates = { coherence_min: Math.min(...coherence), coherence_floor: floor, condition_max: 42, condition_limit: r.config.identification?.condition_limit ?? 1000, passed: Math.min(...coherence) >= floor, coherence };
		r.prediction = predict(r);
		addLog('gate', `Identified: coherence ${Math.min(...coherence).toFixed(2)} at worst against ${floor}, condition 42.`);
		if (!r.gates.passed) {
			r.state = 'review';
			r.phase = 'gate_failed';
			addLog('gate', 'The identification gate did not pass; the run holds.');
		} else if (!r.prediction.within || r.prediction.forecast.cannot_fit) {
			r.state = 'review';
			r.phase = 'prediction_exceeded';
			const f = r.prediction.forecast.cannot_fit;
			addLog('gate', f ? `The budget cannot fit: ${r.derived.drive_labels[f.drive]} needs ${f.need_v.toFixed(2)} V against ${f.ceiling_v} V at ${f.time_s} s.` : 'The prediction is over a ceiling.');
		} else if (r.config.identification?.review !== false) {
			r.state = 'review';
			r.phase = 'awaiting_review';
			addLog('state', 'Identification done: waiting for review.');
		} else engage(r);
	}

	function predict(r: Run): MimoMixedPrediction {
		const crest = r.config.control?.crest_factor ?? 3;
		const top = amp(Math.max(...r.levels));
		const tones = r.parts.filter((p) => p.kind === 'tone' && p.enabled);
		const drives = r.config.channels.drives.map((d, j) => {
			const rms = 0.25 * (1 + 0.1 * j);
			const sum = tones.reduce((s, p) => s + 1.2 * (1 + 0.15 * j) * Math.max(...p.target, 0), 0);
			const ms = tones.reduce((s, p) => s + (1.2 * (1 + 0.15 * j) * Math.max(...p.target, 0)) ** 2 / 2, 0);
			const peak = rms * crest + sum;
			return { drive: j, random_rms_v: round(rms, 4), random_peak_v: round(rms * crest, 4), tone_peak_sum_v: round(sum, 4), tone_rms_v: round(Math.sqrt(ms), 4), total_rms_v: round(Math.sqrt(rms * rms + ms), 4), peak_v: round(peak, 4), ceiling_vrms: d.ceiling_vrms, ceiling_peak_v: d.peak_v, within: rms <= d.ceiling_vrms && peak <= d.peak_v };
		});
		const controls = Array.from({ length: r.n }, (_, i) => {
			const rr = floorRms(r, i);
			const tr = Math.sqrt(tones.reduce((s, p) => s + p.target[i] ** 2 / 2, 0));
			return { control: i, random_rms: round(rr, 4), tone_rms: round(tr, 4), total_rms: round(Math.sqrt(rr * rr + tr * tr), 4) };
		});
		const step = 0.5;
		const per = drives.map((d) => ({ drive: d.drive, time_s: 0, need_v: round(top * d.peak_v, 4), ceiling_v: d.ceiling_peak_v }));
		const worst = per.reduce<(typeof per)[number] | null>((w, x) => (!w || x.need_v / x.ceiling_v > w.need_v / w.ceiling_v ? x : w), null);
		const over = per.find((x) => x.need_v > x.ceiling_v) ?? null;
		return { drives, controls, forecast: { step_s: step, drives: per, worst, cannot_fit: over }, within: drives.every((d) => d.within) };
	}

	function engage(r: Run) {
		r.state = 'running';
		r.phase = 'floor_only';
		r.phaseT = 0;
		r.rung = 0;
		r.levelDb = r.levels[0];
		r.atLevel = 0;
		r.floorErr = Array.from({ length: r.n }, (_, i) => 5 - 0.6 * i);
		addLog('state', `The floor alone at ${r.levelDb} dB, equalizing before the parts come in.`);
	}

	function freqOf(r: Run, p: PartRun, dt: number): number | null {
		const s = p.schedule;
		if (!s) return null;
		if (s.kind === 'fixed') return s.freq_hz;
		if (s.kind === 'follow') {
			const lead = r.parts.find((x) => x.id === s.of);
			return lead?.freq === null || lead?.freq === undefined ? null : lead.freq * s.ratio;
		}
		if (s.kind !== 'sweep') return p.freq;
		const [lo, hi] = s.band_hz;
		const f = p.freq ?? s.start_hz ?? lo;
		if (r.held || dt <= 0) return f;
		const octs = s.rate.law === 'log' ? (s.rate.oct_per_min * dt * PARTS_RATE) / 60 : 0;
		let next = s.rate.law === 'log' ? f * 2 ** (p.direction === 'up' ? octs : -octs) : f + (p.direction === 'up' ? 1 : -1) * s.rate.hz_per_s * dt * PARTS_RATE;
		if (next >= hi) {
			next = hi;
			p.direction = 'down';
			p.sweeps += 1;
		} else if (next <= lo) {
			next = lo;
			p.direction = 'up';
			p.sweeps += 1;
		}
		return next;
	}

	const noise = (k: number) => (Math.sin(k * 12.9898 + Date.now() / 700) * 43758.5453) % 1;

	function step(r: Run, dt: number) {
		r.t += dt;
		r.phaseT += dt;
		if (r.stopping !== null && r.t >= r.stopping) {
			const op = r.end === null;
			const done = r.phase === 'ramping_down' && r.atLevel < 0;
			return end(r, op ? (done ? 'completed' : 'operator_stop') : r.end!.outcome, op ? (done ? 'The schedule finished.' : 'Stopped by the operator.') : r.end!.message);
		}
		if (r.phase === 'sysid' && r.phaseT >= SYSID_S) {
			identify(r);
			return stateChanged(true);
		}
		if (!driving(r)) return;
		if (!r.held) r.atLevel += dt;
		r.floorErr = r.floorErr.map((e, i) => e * Math.exp(-dt / 0.8) + 0.25 * noise(i + 1));
		if (r.phase === 'floor_only') {
			if (!r.floorEqualized && r.floorErr.every((e) => Math.abs(e) < 1) && r.phaseT >= FLOOR_S) {
				r.floorEqualized = true;
				r.phase = 'engaged';
				r.partsT = 0;
				r.atLevel = 0;
				addLog('state', `The floor equalized in ${round(r.phaseT, 1)} s: the parts come in.`);
			}
		} else {
			if (!r.held && r.partsT !== null) r.partsT += dt * PARTS_RATE;
			const ramp = dt / Math.max(0.1, (r.config.control?.part_ramp_s ?? 3) / PARTS_RATE);
			for (const p of r.parts) {
				if (p.kind === 'floor') continue;
				const inWindow = (r.partsT ?? 0) >= p.window.on_s && (p.window.off_s === null || (r.partsT ?? 0) < p.window.off_s);
				const want = p.enabled && inWindow ? 1 : 0;
				const was = p.ramp;
				p.ramp = want > p.ramp ? Math.min(1, p.ramp + ramp) : Math.max(0, p.ramp - ramp);
				if (was === 0 && p.ramp > 0 && !r.logged.has(`in-${p.id}`)) {
					r.logged.add(`in-${p.id}`);
					addLog('part', `${p.name} in${p.freq !== null ? ` at ${round(p.freq, 1)} Hz` : ''}.`);
				}
				p.freq = freqOf(r, p, dt);
				p.err = p.err.map((e, i) => e * Math.exp(-dt / 0.5) + 0.15 * noise(i + 7 + p.id.length));
			}
			applyCeiling(r);
			const full = r.levels.length - 1;
			if (r.config.schedule.mode === 'auto' && !r.held) {
				if (r.rung < full && r.atLevel >= LEVEL_S) {
					r.rung += 1;
					r.levelDb = r.levels[r.rung];
					r.atLevel = 0;
					if (r.rung === full) {
						r.phase = 'full_level';
						addLog('level', `Full level, ${r.levelDb} dB.`);
					} else addLog('level', `Level ${r.levelDb} dB.`);
				} else if (r.rung === full && r.atLevel >= FULL_S) {
					r.state = 'stopping';
					r.phase = 'ramping_down';
					r.stopping = r.t + 0.6;
					r.atLevel = -1;
					addLog('state', 'The full-level time is done: ramping down.');
				}
			}
			if (r.phase === 'full_level') r.softening = Math.min(softCap(r), r.softening + (dt * softCap(r)) / 5);
		}
		record(r);
		stateChanged();
	}

	/** How far the first drive's tone gain grows at full level: until the tones ask about 1.16 times its room. */
	function softCap(r: Run): number {
		const d = r.config.channels.drives[0];
		if (!d) return 0;
		const room = d.peak_v - 0.25 * (r.config.control?.crest_factor ?? 3);
		const ask = r.parts.filter((p) => p.kind === 'tone').reduce((s, p) => s + 1.2 * Math.max(...p.target, 0), 0);
		return ask > 0 && room > 0 ? Math.max(0.2, room / (0.86 * ask) - 1) : 0;
	}

	/** The ceiling rule: each drive's room left to the tones, and the scale it puts on them. */
	function applyCeiling(r: Run) {
		const la = levelAmp(r);
		const tones = r.parts.filter((p) => p.kind === 'tone');
		const rule = r.config.law?.ceiling_rule?.id ?? 'common_scale';
		const rows = drivesOf({ ...r, parts: r.parts.map((p) => ({ ...p, scale: 1 })) } as Run);
		let s = 1;
		let worst = -1;
		rows.forEach((d, j) => {
			const room = la > 0 ? d.ceiling_v / la - d.random_peak_v / Math.max(la, 1e-9) : Infinity;
			const ask = tones.reduce((sum, p) => sum + toneGain(r, j) * Math.max(...p.target, 0) * p.ramp, 0);
			if (ask > 0 && room / ask < s) {
				s = Math.max(0, room / ask);
				worst = j;
			}
		});
		const binds = s < 0.9995;
		if (rule === 'common_scale') {
			for (const p of tones) {
				p.scale = binds ? s : 1;
				p.limiting = binds && p.ramp > 0;
			}
		} else {
			tones.forEach((p, k) => {
				const last = k === tones.length - 1;
				p.scale = binds && last ? Math.max(0, 1 - (1 - s) * tones.length) : 1;
				p.limiting = binds && last && p.ramp > 0;
			});
		}
		r.scale = binds ? s : 1;
		if (binds && !r.logged.has('limit')) {
			r.logged.add('limit');
			addLog('alarm', rule === 'common_scale' ? `${r.derived.drive_labels[worst] ?? 'A drive'} is out of room: every tone at ${Math.round(s * 100)} % of its target.` : `${r.derived.drive_labels[worst] ?? 'A drive'} is out of room: the last tone is scaled down.`);
		}
	}

	function record(r: Run) {
		r.histT.push(round(r.t, 2));
		if (r.histT.length > HISTORY) r.histT.shift();
		const rows = r.parts.map((p) => partRow(r, p));
		r.parts.forEach((p, k) => {
			const row = rows[k];
			for (let i = 0; i < r.n; i++) {
				const c = row.controls[i];
				const push = (h: number[][], v: number) => {
					(h[i] ??= []).push(v);
					if (h[i].length > HISTORY) h[i].shift();
				};
				push(p.ampHist, c.measured ?? 0);
				push(p.levelHist, c.target ?? 0);
				push(p.phHist, round(p.phase[i] + 2 * noise(i + k), 2));
			}
		});
	}

	function end(r: Run, outcome: MimoMixedOutcome, message: string, where: Partial<MimoMixedEnd> = {}) {
		if (r.timer) clearInterval(r.timer);
		r.timer = null;
		r.state = 'finished';
		r.phase = outcome === 'operator_abort' || outcome === 'aborted' ? 'aborted' : 'stopped';
		r.end = { outcome, message, name: null, part: null, control: null, drive: null, at_s: round(r.t, 2), level_db: r.levelDb, ramp_s: outcome === 'operator_abort' || outcome === 'aborted' ? null : 0.6, ...where };
		r.stopping = null;
		r.parts.forEach((p) => {
			p.ramp = 0;
			p.limiting = false;
		});
		addLog('state', `${outcome === 'completed' ? 'Completed' : 'Ended'}: ${message}`);
		stateChanged(true);
		stopLive();
	}

	// ---- display --------------------------------------------------------------------------------

	const fAxis = { label: 'Frequency', unit: 'Hz', scale: 'log' as const };
	const tAxis = { label: 'Time', unit: 's', scale: 'linear' as const };

	function spectrum(title: string, unit: string, series: Series[]): SpectrumView {
		return { view: 'spectrum', title, x: fAxis, y: { label: 'PSD', unit, scale: 'log' }, link: 'frequency', series };
	}

	function measuredPsd(r: Run, i: number, tones: boolean): number[] {
		const la2 = levelAmp(r) ** 2;
		const parts = r.parts.filter((p) => p.kind !== 'floor' && p.ramp > 0 && p.freq !== null);
		const df = r.freqs.length > 1 ? r.freqs[1] - r.freqs[0] : 1;
		return r.freqs.map((f, k) => {
			let y = (r.psd[i][k] ?? 0) * la2 * amp(r.floorErr[i] ?? 0) ** 2 * (1 + 0.15 * noise(k + i));
			for (const p of parts) {
				const near = Math.abs(f - (p.freq as number)) <= df / 2;
				if (p.kind === 'tone' && tones && near) y += ((p.target[i] * Math.sqrt(la2) * p.ramp * p.scale) ** 2 / 2) / df;
				if (p.kind === 'narrowband' && Math.abs(f - (p.freq as number)) <= widthOf(r, p) / 2) y += (r.psd[i][k] ?? 0) * la2 * (p.target[i] - 1) * p.ramp;
			}
			return y;
		});
	}

	function plotViews(r: Run, p: MimoMixedSubscribePlot): View[] {
		const live = driving(r) || r.state === 'stopping' || r.state === 'finished';
		const la2 = levelAmp(r) ** 2;
		const label = (i: number) => r.derived.control_labels[i] ?? `Control ${i + 1}`;
		const chosen = (Array.isArray(p.channels) ? r.config.channels.controls.map((c, i) => (p.channels as number[]).includes(c.channel) ? i : -1).filter((i) => i >= 0) : r.config.channels.controls.map((_, i) => i)).slice(0, 8);
		const unit = 'g²/Hz';
		switch (p.quantity) {
			case 'control':
				return live ? chosen.map((i) => spectrum(label(i), unit, [ser('Measured', 'measured', r.freqs, measuredPsd(r, i, true)), ser('Reference', 'reference', r.freqs, r.psd[i].map((y) => y * la2))])) : [];
			case 'residual':
				return live
					? chosen.map((i) => {
							const ref = r.psd[i].map((y) => y * la2);
							const band = (db: number) => ref.map((y) => y * amp(2 * db) ** 0.5);
							return spectrum(`${label(i)}, tones out`, unit, [
								ser('Residual', 'measured', r.freqs, measuredPsd(r, i, false)),
								ser('Reference', 'reference', r.freqs, ref),
								ser('Alarm high', 'tolerance', r.freqs, band(3)),
								ser('Alarm low', 'tolerance', r.freqs, band(-3)),
								ser('Abort high', 'abort', r.freqs, band(6)),
								ser('Abort low', 'abort', r.freqs, band(-6))
							]);
						})
					: [];
			case 'cross': {
				if (!live) return [];
				const out: View[] = [];
				for (let a = 0; a < chosen.length && out.length < 6; a++)
					for (let b = a + 1; b < chosen.length && out.length < 6; b++)
						out.push({
							view: 'spectrum',
							title: `${label(chosen[a])} with ${label(chosen[b])}`,
							x: fAxis,
							y: { label: 'Coherence', unit: null, scale: 'linear' },
							link: 'frequency',
							series: [
								ser('Measured', 'measured', r.freqs, r.freqs.map((_, k) => Math.max(0, Math.min(1, 0.05 + 0.04 * noise(k + a + b))))),
								ser('Reference', 'reference', r.freqs, r.freqs.map(() => 0))
							]
						} as SpectrumView);
				return out;
			}
			case 'drive':
				return live
					? r.config.channels.drives.slice(0, 8).map((d, j) => spectrum(r.derived.drive_labels[j] ?? `Drive ${j + 1}`, 'V²/Hz', [ser('Drive', 'drive', r.freqs, measuredPsd(r, Math.min(j, r.n - 1), true).map((y) => y * 0.06 * (1 + 0.1 * j)))]))
					: [];
			case 'tone_amplitude':
			case 'tone_phase':
			case 'band_level': {
				const kind = p.quantity === 'band_level' ? 'narrowband' : 'tone';
				const which = r.parts.filter((x) => x.kind === kind && (!Array.isArray(p.parts) || p.parts.includes(x.id)));
				return which.map((x) => {
					const series: Series[] = [];
					for (let i = 0; i < r.n; i++) {
						const hist = p.quantity === 'tone_phase' ? x.phHist[i] : x.ampHist[i];
						const xs = r.histT.slice(-(hist?.length ?? 0));
						series.push(ser(label(i), 'measured', xs, hist ?? []));
						if (p.quantity !== 'tone_phase') series.push(ser(`${label(i)} target`, 'reference', xs, (x.levelHist[i] ?? []).slice(-(hist?.length ?? 0))));
						else series.push(ser(`${label(i)} stated`, 'reference', xs, xs.map(() => x.phase[i])));
					}
					return { view: 'spectrum', title: x.name, x: tAxis, y: p.quantity === 'tone_phase' ? { label: 'Phase', unit: 'deg', scale: 'linear' } : { label: kind === 'tone' ? 'Amplitude' : 'Level', unit: kind === 'tone' ? 'g' : 'g²/Hz', scale: 'linear' }, link: 'time', series } as SpectrumView;
				});
			}
			case 'identification':
				return r.gates ? chosen.map((i) => ({ view: 'spectrum', title: `To ${label(i)}`, x: fAxis, y: { label: 'Response', unit: 'g/V', scale: 'log' }, link: 'frequency', series: r.config.channels.drives.map((_, j) => (ser(r.derived.drive_labels[j], 'measured', r.freqs, r.freqs.map((f) => (i === j ? 2 : 0.4) / Math.sqrt((1 - (f / 180) ** 2) ** 2 + (0.1 * f / 180) ** 2))))) }) as SpectrumView) : [];
			case 'coherence':
				return r.gates ? chosen.map((i) => ({ view: 'spectrum', title: label(i), x: fAxis, y: { label: 'Coherence', unit: null, scale: 'linear' }, link: 'frequency', series: [ser('Multiple coherence', 'measured', r.freqs, r.freqs.map((_, k) => Math.min(1, (r.gates?.coherence?.[i] ?? 0.9) + 0.03 * noise(k)))), ser('Gate', 'tolerance', r.freqs, r.freqs.map(() => r.gates?.coherence_floor ?? 0.5))] }) as SpectrumView) : [];
			case 'law':
				return [{ view: 'reading', title: 'Laws', rows: [{ label: 'Floor law', value: r.config.law?.floor?.id ?? 'pinv_closed', unit: null, status: 'info' }, { label: 'Tone law', value: r.config.law?.tones?.id ?? 'matrix', unit: null, status: 'info' }, { label: 'Step', value: 0.5, unit: null, status: 'info' }, { label: 'Ceiling rule', value: r.config.law?.ceiling_rule?.id ?? 'common_scale', unit: null, status: 'info' }, { label: 'Scale', value: round(r.scale ?? 1, 3), unit: null, status: (r.scale ?? 1) < 1 ? 'warn' : 'info' }] } as View];
			case 'readings': {
				const s = stateOf(r);
				return [{ view: 'reading', title: 'Run', rows: [{ label: 'Level', value: s.level_db ?? 0, unit: 'dB', status: 'info' }, { label: 'Floor worst error', value: s.floor_error_db.length ? round(Math.max(...s.floor_error_db.map(Math.abs)), 2) : 0, unit: 'dB', status: 'info' }, ...s.drives.map((d, j) => ({ label: `${r.derived.drive_labels[j]} peak`, value: round(d.random_peak_v + d.tone_sum_v, 3), unit: 'V', status: d.random_peak_v + d.tone_sum_v > d.ceiling_v * 0.98 ? 'warn' : 'info' }))] } as View];
			}
		}
		return [];
	}

	function liveOf(r: Run): MimoMixedLiveHead {
		const s = stateOf(r);
		const st = r.state === 'running' || r.state === 'review' || r.state === 'stopping' ? r.state : 'finished';
		return {
			run_id: r.id,
			source: r.source,
			class: r.station.class,
			state: st,
			phase: r.phase,
			mode: r.config.schedule.mode,
			t_s: s.t_s,
			elapsed_s: s.elapsed_s,
			parts_s: s.parts_s,
			level_db: driving(r) ? r.levelDb : null,
			commanded_db: r.levelDb,
			at_level_s: s.at_level_s,
			remaining_at_level_s: s.remaining_at_level_s,
			loop: Math.floor(r.t / r.derived.grid.loop_s),
			held: r.held,
			alarm: s.alarm,
			limiting: s.limiting,
			floor_error_db: s.floor_error_db,
			floor_equalized: r.floorEqualized,
			scale: s.scale,
			parts: s.parts,
			drives: s.drives,
			channels: []
		};
	}

	function dataFor(conn: MockConnection) {
		const sub = subs.get(conn);
		if (!sub || !run || run.state === 'configured' || run.state === 'armed') return;
		const r = run;
		conn.emit('mimo_mixed.data', { ...liveOf(r), plots: sub.plots.map((p) => ({ id: p.id, quantity: p.quantity, channels: Array.isArray(p.channels) ? p.channels : null, parts: Array.isArray(p.parts) ? p.parts : null, views: plotViews(r, p) })) } as unknown as Record<string, unknown>);
	}

	function sendData() {
		for (const c of subs.keys()) dataFor(c);
	}

	function startLive() {
		if (liveTimer) return;
		liveTimer = setInterval(() => {
			if (run && isLive() && run.state !== 'armed') {
				host.broadcast('mimo_mixed.live', liveOf(run) as unknown as Record<string, unknown>);
				sendData();
			}
		}, LIVE_MS);
	}

	function stopLive() {
		if (liveTimer) clearInterval(liveTimer);
		liveTimer = null;
		sendData();
	}

	// ---- methods --------------------------------------------------------------------------------

	const needRun = (): Run => run ?? refuse('refused', 'not_configured', 'No MIMO Mixed Mode run is configured.');
	const wrong = (r: Run, verb: string): never => refuse('refused', 'wrong_state', `${verb} does not apply while the run is ${r.state}.`, { state: r.state, phase: r.phase });
	const needDriving = (verb: string): Run => {
		const r = needRun();
		if (!driving(r)) wrong(r, verb);
		return r;
	};

	function newRun(config: MimoMixedConfig, derived: MimoMixedDerived): Run {
		runCount += 1;
		const st = host.station();
		const n = config.channels.controls.length;
		const { freqs, psd } = diagonalOf(config);
		const parts: PartRun[] = config.parts.map((p) => {
			const base = { id: p.id, kind: p.kind, name: p.name ?? p.id, enabled: p.enabled !== false, ramp: 0, window: { on_s: p.window?.on_s ?? 0, off_s: p.window?.off_s ?? null }, direction: 'up' as const, sweeps: 0, err: Array(n).fill(0), scale: 1, limiting: false, ampHist: [], phHist: [], levelHist: [] };
			if (p.kind === 'tone') {
				const sched = p.schedule;
				return { ...base, schedule: sched, target: listOf(p.amplitude, n).map((a) => (a.kind === 'constant' ? a.g_pk : (a.rows[0]?.value ?? 1))), phase: p.phases_stated ? listOf(p.phase_deg, n) : Array(n).fill(0), alarm: (Array.isArray(p.alarm_db?.[0]) ? (p.alarm_db as [number, number][])[0] : ((p.alarm_db as [number, number]) ?? [1.5, 1.5])) as [number, number], freq: sched.kind === 'fixed' ? sched.freq_hz : sched.kind === 'sweep' ? (sched.start_hz ?? sched.band_hz[0]) : null };
			}
			if (p.kind === 'narrowband') {
				const s = p.statement;
				const target = s.kind === 'raise' ? listOf(s.db, n).map((db) => amp(2 * db) ** 0.5 ** 0 * 10 ** (db / 10)) : listOf(s.kind === 'added' ? s.level_g2_hz : 0, n).map((g, i) => 1 + g / Math.max(1e-9, at(freqs, psd[i] ?? [], scheduleSpan(p.schedule)?.[0] ?? 0)));
				return { ...base, schedule: p.schedule, target, phase: Array(n).fill(0), alarm: [3, 3] as [number, number], freq: p.schedule.kind === 'fixed' ? p.schedule.freq_hz : p.schedule.kind === 'sweep' ? (p.schedule.start_hz ?? p.schedule.band_hz[0]) : null };
			}
			return { ...base, schedule: null, target: Array(n).fill(1), phase: Array(n).fill(0), alarm: [3, 3] as [number, number], freq: null };
		});
		return {
			id: `mmix-${String(runCount).padStart(4, '0')}`,
			config,
			derived,
			state: 'configured',
			phase: 'idle',
			source: st.source,
			station: st.ref,
			n,
			m: config.channels.drives.length,
			freqs,
			psd,
			levels: config.schedule.levels.map((l) => l.db),
			rung: 0,
			levelDb: null,
			t: 0,
			phaseT: 0,
			partsT: null,
			atLevel: 0,
			held: false,
			floorErr: Array(n).fill(0),
			floorEqualized: false,
			parts,
			scale: null,
			softening: 0,
			gates: null,
			prediction: null,
			excluded: [],
			override: null,
			end: null,
			stopping: null,
			timer: null,
			record: null,
			histT: [],
			logged: new Set()
		};
	}

	function armRun(r: Run) {
		const other = host.otherRun();
		if (other) refuse('refused', 'run_active', `Run ${other} holds the station.`, { run_id: other });
		if (r.record) return;
		const folder = host.recordFolder?.(r.id) ?? `~/Hertzforge_Data/${r.id}`;
		if (host.record === 'unwritable') refuse('refused', 'record_unwritable', `The run's record could not be written, so the run did not arm: ${folder}.`, { path: folder });
		r.record = { folder, streams: r.config.record?.streams === false ? 'off' : 'on', problem: null };
	}

	const QUANTITIES: MimoMixedQuantity[] = ['control', 'residual', 'cross', 'drive', 'tone_amplitude', 'tone_phase', 'band_level', 'identification', 'coherence', 'law', 'readings'];

	const methods: Record<string, MockMethod> = {
		'mimo_mixed.reference.build_breakpoints': (params) => {
			const p = params as unknown as MimoBuildBreakpointsParams;
			if (!isObject(p.grid) || !Array.isArray(p.tables)) refuse('bad_request', 'bad_params', 'Give the grid and the tables.', { param: 'tables' });
			return buildReference(p);
		},
		'mimo_mixed.reference.import': () => refuse('refused', 'bad_params', 'This demonstration service reads no reference files: build the reference from breakpoints, or connect the control service to import one.'),
		'mimo_mixed.configure': (params, conn) => {
			if (!isObject(params.config)) refuse('bad_request', 'bad_params', 'Give the configuration.', { param: 'config' });
			const raw = params.config as Record<string, unknown>;
			if (!isObject(raw.channels) || !isObject(raw.reference) || !Array.isArray(raw.parts) || !isObject(raw.schedule)) refuse('bad_request', 'bad_params', 'Give the channels, the reference, the parts and the schedule.', { param: 'config' });
			const config = raw as unknown as MimoMixedConfig;
			const dry = params.dry_run === true;
			if (!dry) host.own(conn);
			if (!dry && isLive()) wrong(run as Run, 'Configure');
			const res = check(config);
			if (dry) return res;
			if (res.errors.length) refuse('bad_request', 'config_invalid', `The configuration has ${res.errors.length} ${res.errors.length === 1 ? 'refusal' : 'refusals'}.`, { errors: res.errors, warnings: res.warnings });
			if (run?.timer) clearInterval(run.timer);
			run = newRun(config, res.derived as MimoMixedDerived);
			log = [];
			addLog('state', 'Configured.');
			stateChanged(true);
			return { state: stateOf(run), derived: res.derived, warnings: res.warnings };
		},
		'mimo_mixed.config': () => ({ run_id: run?.id ?? null, config: run?.config ?? null, derived: run?.derived ?? null, views: [] }),
		'mimo_mixed.state': () => stateOf(run),
		'mimo_mixed.arm': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'configured' && r.state !== 'blocked') wrong(r, 'Arm');
			armRun(r);
			r.state = 'armed';
			r.phase = 'armed';
			addLog('station', 'Armed: the station is open; nothing is driven.');
			startLive();
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.start': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'configured' && r.state !== 'armed') wrong(r, 'Start');
			if (r.state === 'configured') armRun(r);
			r.state = 'running';
			r.phase = 'sysid';
			r.phaseT = 0;
			r.timer = setInterval(() => run === r && step(r, (TICK_MS / 1000) * host.speed), TICK_MS);
			startLive();
			addLog('state', 'Identification: independent random on every drive at once.');
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.proceed': (params, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'review') wrong(r, 'Proceed');
			if (r.phase === 'gate_failed' || r.phase === 'prediction_exceeded') refuse('refused', 'wrong_state', 'The run holds on its gate: proceed with your words (override), or abort.', { phase: r.phase });
			const ack = typeof params.acknowledge === 'string' ? params.acknowledge.trim() : '';
			if (ack) addLog('override', `Proceeded: "${ack}"`);
			engage(r);
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.override': (params, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'review' || (r.phase !== 'gate_failed' && r.phase !== 'prediction_exceeded')) wrong(r, 'Override');
			const ack = typeof params.ack === 'string' ? params.ack.trim() : '';
			if (!ack) refuse('bad_request', 'bad_params', 'Give your words for the log.', { param: 'ack' });
			r.override = { by: host.clientName(conn), text: ack, reason: r.phase as 'gate_failed' | 'prediction_exceeded' };
			addLog('override', `Proceeded past ${r.phase === 'gate_failed' ? 'the gate' : 'the prediction'}: "${ack}"`);
			engage(r);
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.exclude': (params, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'review') wrong(r, 'Exclude');
			const list = Array.isArray(params.controls) ? (params.controls as number[]) : [];
			r.excluded = [...new Set([...r.excluded, ...list.filter((i) => i >= 0 && i < r.n)])];
			addLog('gate', `Taken out of control: ${r.excluded.map((i) => r.derived.control_labels[i]).join(', ')}.`);
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.level': (params, conn) => {
			host.own(conn);
			const r = needDriving('Level');
			if (r.config.schedule.mode !== 'manual') refuse('refused', 'manual_mode_required', 'The level moves by hand in the manual mode only.');
			let to: number;
			if (typeof params.to_db === 'number') {
				if (!r.levels.includes(params.to_db)) refuse('refused', 'not_a_rung', `${params.to_db} dB is not a level of the schedule.`);
				to = params.to_db;
			} else to = r.levels[Math.max(0, Math.min(r.levels.length - 1, r.rung + (typeof params.delta_db === 'number' && params.delta_db < 0 ? -1 : 1)))];
			r.rung = r.levels.indexOf(to);
			r.levelDb = to;
			r.atLevel = 0;
			if (r.phase !== 'floor_only') r.phase = r.rung === r.levels.length - 1 ? 'full_level' : 'engaged';
			addLog('level', `Level ${to} dB, by hand.`);
			stateChanged(true);
			return { level_db: to };
		},
		'mimo_mixed.mode': (params, conn) => {
			host.own(conn);
			const r = needRun();
			if (!['configured', 'blocked'].includes(r.state)) refuse('refused', 'wrong_state', "The ladder's mode is fixed once the run has armed.");
			const mode = params.mode === 'manual' ? 'manual' : 'auto';
			r.config = { ...r.config, schedule: { ...r.config.schedule, mode } };
			stateChanged(true);
			return { mode };
		},
		'mimo_mixed.hold': (_p, conn) => {
			host.own(conn);
			const r = needDriving('Hold');
			r.held = true;
			addLog('note', "Held: every part's clock stops; the level and the control go on.");
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.resume': (_p, conn) => {
			host.own(conn);
			const r = needDriving('Resume');
			if (!r.held) wrong(r, 'Resume');
			r.held = false;
			addLog('note', 'Resumed.');
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.part': (params, conn) => {
			host.own(conn);
			const r = needRun();
			if (!partsIn(r)) refuse('refused', 'part_refused', 'Once the parts have come in.', { why: 'window_closed' });
			const p = r.parts.find((x) => x.id === params.id);
			if (!p) refuse('refused', 'part_refused', `No part "${String(params.id)}".`, { why: 'unknown_part' });
			if (p.kind === 'floor') refuse('refused', 'part_refused', 'The floor plays for the whole run.', { why: 'floor' });
			p.enabled = params.on === true;
			addLog('part', `${p.name} switched ${p.enabled ? 'on' : 'off'}.`);
			stateChanged(true);
			return { id: p.id, on: p.enabled };
		},
		'mimo_mixed.adjust': (params, conn) => {
			host.own(conn);
			needDriving('Adjust');
			if (typeof params.key !== 'string' || typeof params.value !== 'number') refuse('bad_request', 'bad_params', 'Give the key and the value.', { param: 'key' });
			addLog('law', `${params.key} set to ${params.value}.`);
			return { key: params.key, value: params.value };
		},
		'mimo_mixed.stop': () => {
			const r = needRun();
			if (!isLive()) wrong(r, 'Stop');
			if (r.state === 'armed' || r.state === 'review' || r.phase === 'sysid') end(r, 'operator_stop', 'Stopped by the operator.');
			else {
				r.state = 'stopping';
				r.phase = 'ramping_down';
				r.stopping = r.t + 0.6;
				stateChanged(true);
			}
			return stateOf(r);
		},
		'mimo_mixed.abort': () => {
			const r = needRun();
			if (!isLive()) wrong(r, 'Abort');
			end(r, 'operator_abort', 'Aborted by the operator.');
			return stateOf(r);
		},
		'mimo_mixed.reset': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (isLive()) wrong(r, 'New Run');
			run = newRun(r.config, r.derived);
			log = [];
			stateChanged(true);
			return stateOf(run);
		},
		'mimo_mixed.clear': (_p, conn) => {
			host.own(conn);
			if (isLive()) wrong(run as Run, 'Clear');
			run = null;
			stateChanged(true);
			return stateOf(run);
		},
		'mimo_mixed.log': (params) => {
			const since = typeof params.since === 'number' ? params.since : 0;
			return { entries: log.filter((e) => e.seq >= since), next: log.length };
		},
		'mimo_mixed.subscribe': (params, conn) => {
			if (params.default === true) {
				subs.delete(conn);
				return { default: true, rate_hz: 5, plots: null, quantities: QUANTITIES };
			}
			if (!Array.isArray(params.plots)) refuse('bad_request', 'bad_params', 'Give the plots, or ask for the default.', { param: 'plots' });
			const plots = (params.plots as MimoMixedSubscribePlot[]).map((p) => ({ ...p }));
			if (plots.length > 64) refuse('bad_request', 'bad_params', 'At most 64 plots.', { param: 'plots' });
			for (const p of plots) if (!QUANTITIES.includes(p.quantity)) refuse('bad_request', 'bad_params', `No quantity "${p.quantity}".`, { param: 'plots' });
			const rate = typeof params.rate_hz === 'number' ? params.rate_hz : 5;
			subs.set(conn, { rate, plots });
			queueMicrotask(() => dataFor(conn));
			return { default: false, rate_hz: rate, plots, quantities: QUANTITIES };
		}
	};

	return {
		methods,
		isLive,
		liveId: () => (isLive() && run ? run.id : null),
		detach: (conn) => {
			subs.delete(conn);
		},
		seatExpired: () => {
			if (run && isLive()) end(run, 'safe_stop', 'Nobody held control.');
		},
		floorLaws: () => FLOOR_LAWS
	};
}
