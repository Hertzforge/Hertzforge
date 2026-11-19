/**
 * The breakpoint tables the setup editors hold, as breakpoint table files, and back: a Random
 * profile (and each MIMO quantity's), a limit's ceiling, a Sine profile or a Sine limit channel's
 * bound, an SRS target, and the gain from the drive to a limit channel. A file says which table it
 * holds (`table_kind`) and its units; a file of another kind is refused by the analysis service,
 * and one in other units is refused here, or read in the editor's unit where that is exact (a Sine
 * row's quantity). Pure functions: what is opened replaces the editor's rows, and the editor's own
 * checks and the service's dry run judge it as they judge typed rows.
 */

import type { BandDb } from '$lib/protocol/types';
import type { SineQuantity } from '$lib/protocol/sine';
import type { FileResult, LoadedTable, TableCell, TableData } from '$lib/tables/tableFiles';
import type { BreakpointDraft } from './setup';
import { QUANTITIES, type SineRowDraft } from './sine/sineSetup';

/** A table file's contents that cannot take the editor's place, said in words. */
export class TableRefusal extends Error {}

/** The rows a file holds, as the editor holds them, and what the reading changed or left out. */
export interface OpenedRows<R> {
	rows: R[];
	notes: FileResult[];
}

/** The x column of every breakpoint table, and its only unit. */
export const X_UNIT = 'Hz';

/** The band columns of a profile row: alarm and abort, dB below and above the reference. */
export const BAND_COLUMNS = ['alarm_below_db', 'alarm_above_db', 'abort_below_db', 'abort_above_db'] as const;

/** A number cell's value, or null for none. NaN is none: JSON has no NaN. */
const cell = (v: number): number | null => (Number.isFinite(v) ? v : null);

/** A cell as a number, NaN where it holds none. */
const num = (v: TableCell | undefined): number => (typeof v === 'number' ? v : NaN);

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
const rowList = (rows: number[]) => (rows.length === 1 ? `row ${rows[0]}` : `rows ${rows.slice(0, -1).join(', ')} and ${rows.at(-1)}`);

/**
 * A unit as two units are compared: spaces gone, case folded, a square written one way (`g^2/Hz`,
 * `g2/Hz` and `g²/Hz` are one), and a trailing `pk` or `peak` dropped (`g pk` is `g`).
 */
export function unitKey(unit: string): string {
	const u = unit
		.trim()
		.toLowerCase()
		.replace(/\s+/g, '')
		.replace(/\^2|\*\*2/g, '²')
		.replace(/([a-z])2(?=\/|$)/g, '$1²');
	return u.replace(/(?<=[a-z/²)])(?:peak|pk)$/, '');
}

/** Whether two units are the same unit, as `unitKey` compares them. */
export const sameUnit = (a: string, b: string) => unitKey(a) === unitKey(b);

/** The column's values by name: a column the file did not have is undefined. */
function columnsOf(loaded: LoadedTable): (name: string) => ((row: TableCell[]) => TableCell) | undefined {
	return (name) => {
		const at = loaded.columns.indexOf(name);
		return at < 0 ? undefined : (row) => row[at] ?? null;
	};
}

/** Refuse a file with no rows, or with its frequencies in another unit than Hz. */
function checkRowsAndX(loaded: LoadedTable): void {
	if (!loaded.rows.length) throw new TableRefusal('It has no rows.');
	const x = loaded.x_unit ?? '';
	if (x && !sameUnit(x, X_UNIT)) throw new TableRefusal(`Its frequencies are in ${x}; this table's are in ${X_UNIT}.`);
}

/** Refuse a file whose levels are in another unit than the editor's, where the editor offers no other. */
function checkY(loaded: LoadedTable, unit: string): void {
	const y = loaded.y_unit ?? '';
	if (y && !sameUnit(y, unit)) throw new TableRefusal(`Its values are in ${y}; this table's are in ${unit}.`);
}

// ---------------------------------------------------------------------------------------------
// A Random profile: a PSD anchor or a slope on each row, and a row's own bands

export interface ProfileFile {
	/** Each row's own bands are written and read; off where the bands are the channel's. */
	bands: boolean;
	/** The PSD's unit (`g²/Hz`). */
	unit: string;
}

/** A Random profile's rows as a `random_profile` table: one of the PSD and the slope on each row. */
export function profileTable(rows: BreakpointDraft[], file: ProfileFile): TableData {
	const columns = ['freq_hz', 'psd_g2_hz', 'slope_db_oct', ...(file.bands ? BAND_COLUMNS : [])];
	return {
		columns,
		rows: rows.map((b) => {
			const out: TableCell[] = [cell(b.freq_hz), b.kind === 'psd' ? cell(b.psd_g2_hz) : null, b.kind === 'slope' ? cell(b.slope_db_oct) : null];
			if (file.bands) out.push(...(b.alarm_db ?? [NaN, NaN]).map(cell), ...(b.abort_db ?? [NaN, NaN]).map(cell));
			return out;
		}),
		x_unit: X_UNIT,
		y_unit: file.unit
	};
}

/**
 * The rows of a `random_profile` file. A row with a PSD is an anchor and one with only a slope is
 * a slope; a row with both keeps the PSD and says so. A row with any band of its own takes the
 * profile's band for a side it leaves out; where the editor's rows have no bands of their own,
 * the file's are left out and named.
 */
export function profileRows(loaded: LoadedTable, file: ProfileFile & { base: { alarm_db: BandDb; abort_db: BandDb } }): OpenedRows<BreakpointDraft> {
	checkRowsAndX(loaded);
	checkY(loaded, file.unit);
	const col = columnsOf(loaded);
	const [f, psd, slope] = [col('freq_hz'), col('psd_g2_hz'), col('slope_db_oct')];
	const bands = BAND_COLUMNS.map(col);
	const notes: FileResult[] = [];
	const both: number[] = [];
	let dropped = 0;
	const rows = loaded.rows.map((r, i): BreakpointDraft => {
		const p = psd ? num(psd(r)) : NaN;
		const s = slope ? num(slope(r)) : NaN;
		if (Number.isFinite(p) && Number.isFinite(s)) both.push(i + 1);
		const kind = Number.isFinite(p) || !Number.isFinite(s) ? 'psd' : 'slope';
		const own = bands.map((b) => (b ? num(b(r)) : NaN));
		const has = own.some(Number.isFinite);
		if (has && !file.bands) dropped++;
		const side = (k: number, base: number) => (Number.isFinite(own[k]) ? own[k] : base);
		return {
			freq_hz: f ? num(f(r)) : NaN,
			kind,
			psd_g2_hz: kind === 'psd' ? p : NaN,
			slope_db_oct: kind === 'slope' ? s : NaN,
			alarm_db: has && file.bands ? [side(0, file.base.alarm_db[0]), side(1, file.base.alarm_db[1])] : null,
			abort_db: has && file.bands ? [side(2, file.base.abort_db[0]), side(3, file.base.abort_db[1])] : null
		};
	});
	if (both.length) notes.push({ level: 'warning', message: `${rowList(both)} ${both.length === 1 ? 'gives' : 'give'} a PSD and a slope; the PSD was kept.` });
	if (dropped) notes.push({ level: 'warning', message: `Left out the bands on ${plural(dropped, 'row', 'rows')}: this table's rows have no bands of their own.` });
	return { rows, notes };
}

// ---------------------------------------------------------------------------------------------
// A Sine profile or a Sine limit channel's bound: each row's value in its quantity's own unit

export type SineTableKind = 'sine_profile' | 'sine_limit';

export interface SineFile {
	tableKind: SineTableKind;
	/** Each row's own bands (the control profile); a bound has none. */
	bands: boolean;
}

/** The value column of each kind of Sine table. */
const SINE_VALUE: Record<SineTableKind, string> = { sine_profile: 'value', sine_limit: 'bound' };

/** A Sine table's rows: the frequency, the quantity, the value, and a profile row's own bands. */
export function sineTable(rows: SineRowDraft[], file: SineFile): TableData {
	const bands = file.bands && file.tableKind === 'sine_profile';
	return {
		columns: ['freq_hz', 'quantity', SINE_VALUE[file.tableKind], ...(bands ? BAND_COLUMNS : [])],
		rows: rows.map((r) => {
			const out: TableCell[] = [cell(r.freq_hz), r.quantity, cell(r.value)];
			if (bands) out.push(...(r.own_bands ? [...r.alarm_db, ...r.abort_db].map(cell) : [null, null, null, null]));
			return out;
		}),
		x_unit: X_UNIT,
		y_unit: ''
	};
}

/** The words a Sine file's unit may give for each quantity, as `unitKey` writes them. */
const SINE_UNITS: Record<string, SineQuantity> = Object.fromEntries([
	...QUANTITIES.map((q) => [unitKey(q.unit), q.id] as const),
	['ips', 'velocity_ips'],
	['inpp', 'disp_pp_in'],
	['inp-p', 'disp_pp_in']
]);

const quantityUnit = (q: SineQuantity) => QUANTITIES.find((x) => x.id === q)?.unit ?? q;

/**
 * The rows of a Sine table file. A file that states one unit for its values is read in it: with no
 * quantity column every row takes that unit's quantity, and with one every row must agree. Any
 * other unit is refused. A row with a band of its own takes the profile's for a side it leaves
 * out; a bound's rows have none, so a file's bands are left out and named.
 */
export function sineRows(loaded: LoadedTable, file: SineFile & { base?: { alarm_db: BandDb; abort_db: BandDb } }): OpenedRows<SineRowDraft> {
	checkRowsAndX(loaded);
	const col = columnsOf(loaded);
	const [f, q, v] = [col('freq_hz'), col('quantity'), col(SINE_VALUE[file.tableKind])];
	const notes: FileResult[] = [];
	const y = loaded.y_unit ?? '';
	let stated: SineQuantity | null = null;
	if (y) {
		stated = SINE_UNITS[unitKey(y)] ?? null;
		if (!stated) throw new TableRefusal(`Its values are in ${y}; a Sine table's are in ${QUANTITIES.map((x) => x.unit).join(', ')}, each row in its own.`);
	}
	const quantityOf = (r: TableCell[]): SineQuantity => {
		const said = q?.(r);
		return typeof said === 'string' && QUANTITIES.some((x) => x.id === said) ? (said as SineQuantity) : (stated ?? 'accel_g');
	};
	if (stated && q) {
		const other = loaded.rows.flatMap((r, i) => (quantityOf(r) === stated ? [] : [i + 1]));
		if (other.length) throw new TableRefusal(`It says its values are in ${quantityUnit(stated)}, but ${rowList(other)} ${other.length === 1 ? 'is' : 'are'} in another unit.`);
	}
	if (stated && !q && stated !== 'accel_g') notes.push({ level: 'note', message: `Read every value in ${quantityUnit(stated)}, the unit the file gives.` });
	const bands = BAND_COLUMNS.map(col);
	const base = file.base ?? { alarm_db: [1.5, 1.5], abort_db: [3, 3] };
	let dropped = 0;
	const rows = loaded.rows.map((r): SineRowDraft => {
		const own = bands.map((b) => (b ? num(b(r)) : NaN));
		const has = own.some(Number.isFinite);
		if (has && !file.bands) dropped++;
		const side = (k: number, fallback: number) => (Number.isFinite(own[k]) ? own[k] : fallback);
		return {
			freq_hz: f ? num(f(r)) : NaN,
			quantity: quantityOf(r),
			value: v ? num(v(r)) : NaN,
			own_bands: has && file.bands,
			alarm_db: has && file.bands ? [side(0, base.alarm_db[0]), side(1, base.alarm_db[1])] : [base.alarm_db[0], base.alarm_db[1]],
			abort_db: has && file.bands ? [side(2, base.abort_db[0]), side(3, base.abort_db[1])] : [base.abort_db[0], base.abort_db[1]]
		};
	});
	if (dropped) notes.push({ level: 'warning', message: `Left out the bands on ${plural(dropped, 'row', 'rows')}: this table's rows have no bands of their own.` });
	return { rows, notes };
}

// ---------------------------------------------------------------------------------------------
// Two columns, a frequency and a level: a limit's ceiling, an SRS target, a drive gain

export interface PointsFile<R> {
	/** The editor's keys of the frequency and of the level. */
	keys: [keyof R & string, keyof R & string];
	/** The file's column for the level (`ceiling`, `accel_g`, `gain`). */
	column: string;
	/** The level's unit. */
	unit: string;
}

/** Rows of a frequency and a level as a two-column table. */
export function pointsTable<R extends object>(rows: R[], file: PointsFile<R>): TableData {
	const [x, y] = file.keys;
	return {
		columns: ['freq_hz', file.column],
		rows: rows.map((r) => [cell(r[x] as unknown as number), cell(r[y] as unknown as number)]),
		x_unit: X_UNIT,
		y_unit: file.unit
	};
}

/** The rows of a two-column table file, in the editor's keys. */
export function pointsRows<R extends object>(loaded: LoadedTable, file: PointsFile<R>): OpenedRows<R> {
	checkRowsAndX(loaded);
	checkY(loaded, file.unit);
	const col = columnsOf(loaded);
	const [f, v] = [col('freq_hz'), col(file.column)];
	const [x, y] = file.keys;
	return { rows: loaded.rows.map((r) => ({ [x]: f ? num(f(r)) : NaN, [y]: v ? num(v(r)) : NaN }) as R), notes: [] };
}
