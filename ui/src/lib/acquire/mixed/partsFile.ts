/**
 * Mixed Mode's parts tables as table files (`mixed_tones`, `mixed_bands`): one row a part, every
 * column of the draft's row. A schedule's fields go in the columns of its kind and the others stay
 * empty; a tone's amplitude profile is one text cell, its rows as `freq quantity value` separated by
 * `;`, with a row's own bands as four more numbers. Reading takes the columns the file has and fills
 * the rest from a new part's, and says in a line what it could not read.
 */

import type { BandDb } from '$lib/protocol/types';
import type { SineQuantity } from '$lib/protocol/sine';
import type { FileResult, LoadedTable, TableCell, TableData } from '$lib/tables/tableFiles';
import { newBand, newTone, type MixedBandDraft, type MixedToneDraft, type ScheduleDraft } from './mixedSetup';
import { newRow, type SineRowDraft } from '../sine/sineSetup';

export const PART_TABLE_KIND = { tones: 'mixed_tones', bands: 'mixed_bands' } as const;

const SHARED = ['id', 'name', 'enabled', 'schedule', 'freq_hz', 'dwell_s', 'dwell_cycles', 'lo_hz', 'hi_hz', 'rate_law', 'rate', 'start_hz', 'direction', 'end_sweeps', 'end_s', 'follows', 'ratio', 'alarm_below_db', 'alarm_above_db', 'abort_below_db', 'abort_above_db', 'on_s', 'off_s'];
export const TONE_COLUMNS = [...SHARED, 'amplitude_g', 'profile'];
export const BAND_COLUMNS = [...SHARED, 'level_g2_hz', 'width_hz', 'width_share'];

const n = (v: number) => (Number.isFinite(v) ? v : null);

function scheduleCells(s: ScheduleDraft): Record<string, TableCell> {
	const out: Record<string, TableCell> = { schedule: s.kind };
	if (s.kind === 'fixed') {
		out.freq_hz = n(s.freq_hz);
		if (s.dwell.by === 'seconds') out.dwell_s = n(s.dwell.seconds);
		if (s.dwell.by === 'cycles') out.dwell_cycles = n(s.dwell.cycles);
	} else if (s.kind === 'sweep') {
		Object.assign(out, { lo_hz: n(s.band_hz[0]), hi_hz: n(s.band_hz[1]), rate_law: s.law, rate: n(s.law === 'log' ? s.oct_per_min : s.hz_per_s), start_hz: s.start_hz, direction: s.direction });
		if (s.end_by === 'sweeps' || s.end_by === 'both') out.end_sweeps = n(s.sweeps);
		if (s.end_by === 'duration' || s.end_by === 'both') out.end_s = n(s.duration_s);
	} else Object.assign(out, { follows: s.of, ratio: n(s.ratio) });
	return out;
}

/** A tone's profile rows in one cell. */
export function profileText(rows: SineRowDraft[]): string {
	return rows.map((r) => [r.freq_hz, r.quantity, r.value, ...(r.own_bands ? [...r.alarm_db, ...r.abort_db] : [])].join(' ')).join('; ');
}

export function profileRowsOf(text: string): SineRowDraft[] {
	return text
		.split(';')
		.map((x) => x.trim().split(/\s+/))
		.filter((p) => p.length >= 3)
		.map((p) => {
			const row = newRow(Number(p[0]), p[1] as SineQuantity, Number(p[2]));
			if (p.length >= 7) {
				row.own_bands = true;
				row.alarm_db = [Number(p[3]), Number(p[4])];
				row.abort_db = [Number(p[5]), Number(p[6])];
			}
			return row;
		});
}

/** The table a parts table writes. */
export function partsTable(table: 'tones' | 'bands', rows: (MixedToneDraft | MixedBandDraft)[]): TableData {
	const columns = table === 'tones' ? TONE_COLUMNS : BAND_COLUMNS;
	return {
		columns,
		rows: rows.map((r) => {
			const c: Record<string, TableCell> = { id: r.id, name: r.name, enabled: r.enabled ? 'on' : 'off', ...scheduleCells(r.schedule), on_s: n(r.window.on_s), off_s: r.window.off_s };
			const bands = 'amplitude' in r || r.own_bands;
			if (bands) Object.assign(c, { alarm_below_db: n(r.alarm_db[0]), alarm_above_db: n(r.alarm_db[1]), abort_below_db: n(r.abort_db[0]), abort_above_db: n(r.abort_db[1]) });
			if ('amplitude' in r) {
				if (r.amplitude.by === 'constant') c.amplitude_g = n(r.amplitude.g);
				else c.profile = profileText(r.amplitude.profile.rows);
			} else {
				c.level_g2_hz = n(r.level_g2_hz);
				if (r.width.by === 'hz') c.width_hz = n(r.width.hz);
				else c.width_share = n(r.width.share);
			}
			return columns.map((k) => c[k] ?? null);
		})
	};
}

const num = (v: TableCell | undefined): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null);
const str = (v: TableCell | undefined): string | null => (typeof v === 'string' && v.trim() ? v.trim() : null);

/** The rows a parts table file holds, as the draft keeps them, and what could not be read. */
export function partsRows(table: 'tones' | 'bands', loaded: LoadedTable): { rows: (MixedToneDraft | MixedBandDraft)[]; notes: FileResult[] } {
	const notes: FileResult[] = loaded.warnings.map((message) => ({ level: 'warning' as const, message }));
	const rows = loaded.rows.map((cells, i) => {
		const c: Record<string, TableCell> = Object.fromEntries(loaded.columns.map((k, j) => [k, cells[j]]));
		const id = str(c.id) ?? `${table === 'tones' ? 't' : 'b'}${i + 1}`;
		const base = table === 'tones' ? newTone(id, i + 1) : newBand(id, i + 1);
		base.name = str(c.name) ?? base.name;
		base.enabled = c.enabled !== 'off';
		base.window = { on_s: num(c.on_s) ?? 0, off_s: num(c.off_s) };
		const s = base.schedule;
		const kind = c.schedule === 'sweep' || c.schedule === 'follow' ? c.schedule : 'fixed';
		s.kind = kind;
		if (kind === 'fixed') {
			s.freq_hz = num(c.freq_hz) ?? NaN;
			s.dwell = { ...s.dwell, by: num(c.dwell_s) !== null ? 'seconds' : num(c.dwell_cycles) !== null ? 'cycles' : 'none', seconds: num(c.dwell_s) ?? s.dwell.seconds, cycles: num(c.dwell_cycles) ?? s.dwell.cycles };
		} else if (kind === 'sweep') {
			s.band_hz = [num(c.lo_hz) ?? NaN, num(c.hi_hz) ?? NaN];
			s.law = c.rate_law === 'linear' ? 'linear' : 'log';
			if (s.law === 'log') s.oct_per_min = num(c.rate) ?? NaN;
			else s.hz_per_s = num(c.rate) ?? NaN;
			s.start_hz = num(c.start_hz);
			s.direction = c.direction === 'down' ? 'down' : 'up';
			const sw = num(c.end_sweeps);
			const du = num(c.end_s);
			s.end_by = sw !== null && du !== null ? 'both' : sw !== null ? 'sweeps' : du !== null ? 'duration' : 'none';
			if (sw !== null) s.sweeps = sw;
			if (du !== null) s.duration_s = du;
		} else {
			s.of = str(c.follows) ?? '';
			s.ratio = num(c.ratio) ?? NaN;
		}
		const al: BandDb = [num(c.alarm_below_db) ?? NaN, num(c.alarm_above_db) ?? NaN];
		const ab: BandDb = [num(c.abort_below_db) ?? NaN, num(c.abort_above_db) ?? NaN];
		const hasBands = [...al, ...ab].some(Number.isFinite);
		if (hasBands) {
			base.alarm_db = [Number.isFinite(al[0]) ? al[0] : base.alarm_db[0], Number.isFinite(al[1]) ? al[1] : base.alarm_db[1]];
			base.abort_db = [Number.isFinite(ab[0]) ? ab[0] : base.abort_db[0], Number.isFinite(ab[1]) ? ab[1] : base.abort_db[1]];
		}
		if ('amplitude' in base) {
			const profile = str(c.profile);
			if (profile) {
				const rows = profileRowsOf(profile);
				if (rows.length < 2) notes.push({ level: 'warning', message: `Row ${i + 1}: the amplitude profile has fewer than two rows.` });
				base.amplitude = { ...base.amplitude, by: 'profile', profile: { ...base.amplitude.profile, rows } };
			} else base.amplitude.g = num(c.amplitude_g) ?? NaN;
		} else {
			base.own_bands = hasBands;
			base.level_g2_hz = num(c.level_g2_hz) ?? NaN;
			base.width = num(c.width_share) !== null ? { ...base.width, by: 'share', share: num(c.width_share)! } : { ...base.width, by: 'hz', hz: num(c.width_hz) ?? NaN };
		}
		return base;
	});
	if (!rows.length) notes.push({ level: 'warning', message: 'The file has no parts.' });
	return { rows, notes };
}
