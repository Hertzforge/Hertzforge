/**
 * Each setup editor's breakpoint rows as a breakpoint table file and back. The goldens are the
 * table shapes the analysis service reads and writes (a Random profile opened, a Sine bound saved,
 * a drive gain opened); each editor's rows come back from its shape unchanged and give it back
 * unchanged, and a file in other units is read in the editor's or refused in words.
 */
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame } from '$lib/protocol/codec';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createAnalysisServer } from '$lib/protocol/mock/analysis';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { BandDb, LimitProfile } from '$lib/protocol/types';
import { loadTable, saveTable, type LoadedTable, type TableData } from '$lib/tables/tableFiles';
import { pointsRows, pointsTable, profileRows, profileTable, sameUnit, sineRows, sineTable, TableRefusal, unitKey, type PointsFile } from './breakpointFile';
import { defaultSetup, newBreakpoint, toConfig, type BreakpointDraft } from './setup';
import { defaultSineSetup, newRow, profileOf, type SineRowDraft } from './sine/sineSetup';
import type { SrsFrfPointDraft, SrsPointDraft } from './srs/srsSetup';

const golden = (name: string) => resolve(dirname(fileURLToPath(import.meta.url)), '../../../../schema/golden', name);
const frame = (name: string) => decodeFrame(readFileSync(golden(`${name}.json`), 'utf-8')) as unknown as { params: Record<string, unknown>; result: Record<string, unknown> };

/** A file's table as the service hands it back. */
function loaded(tableKind: string, data: TableData, warnings: string[] = []): LoadedTable {
	return { kind: 'breakpoints', table_kind: tableKind, columns: [...data.columns], rows: structuredClone(data.rows), x_unit: data.x_unit, y_unit: data.y_unit, warnings };
}

const BASE: { alarm_db: BandDb; abort_db: BandDb } = { alarm_db: [3, 3], abort_db: [6, 6] };
const PROFILE = { bands: true, unit: 'g²/Hz', base: BASE };
const shape = (t: TableData) => ({ columns: t.columns, rows: t.rows, x_unit: t.x_unit, y_unit: t.y_unit });

describe('units', () => {
	it('are compared with spaces, case, the way a square is written and a trailing peak set aside', () => {
		expect(['g²/Hz', 'g^2/Hz', 'G2/hz', 'g **2 / Hz'].map(unitKey)).toEqual(['g²/hz', 'g²/hz', 'g²/hz', 'g²/hz']);
		expect(sameUnit('g pk', 'g')).toBe(true);
		expect(sameUnit('in/s pk', 'in/s')).toBe(true);
		expect(sameUnit('in pk-pk', 'in')).toBe(false);
		expect(sameUnit('lbf²/Hz', 'g²/Hz')).toBe(false);
		expect(sameUnit('kHz', 'Hz')).toBe(false);
	});
});

describe('a Random profile', () => {
	const opened = frame('response_tables_load').result as unknown as LoadedTable;

	it('reads the golden profile and gives it back unchanged', () => {
		const { rows, notes } = profileRows(opened, { ...PROFILE, bands: false });
		expect(rows.map((r) => [r.freq_hz, r.kind])).toEqual([
			[20, 'slope'],
			[80, 'psd'],
			[350, 'psd'],
			[2000, 'slope']
		]);
		expect(notes).toEqual([]);
		expect(shape(profileTable(rows, { bands: false, unit: 'g²/Hz' }))).toEqual({ columns: opened.columns, rows: opened.rows, x_unit: 'Hz', y_unit: 'g²/Hz' });
	});

	it('keeps every row and its own bands through a file, as the run is sent', () => {
		const d = defaultSetup();
		d.breakpoints[1].alarm_db = [1.5, 2];
		d.breakpoints[1].abort_db = [4, 5];
		const table = profileTable(d.breakpoints, PROFILE);
		expect(table.columns).toEqual(['freq_hz', 'psd_g2_hz', 'slope_db_oct', 'alarm_below_db', 'alarm_above_db', 'abort_below_db', 'abort_above_db']);
		const back = profileRows(loaded('random_profile', table), PROFILE).rows;
		expect(back).toEqual(d.breakpoints);
		expect(toConfig({ ...d, breakpoints: back })).toEqual(toConfig(d));
		expect(profileTable(back, PROFILE)).toEqual(table);
	});

	it('takes the profile band for a side a row leaves out, and leaves out bands a table has none of', () => {
		const table: TableData = { columns: ['freq_hz', 'psd_g2_hz', 'alarm_above_db'], rows: [[20, 0.01, 2], [2000, 0.01, null]], x_unit: 'Hz', y_unit: 'g²/Hz' };
		const own = profileRows(loaded('random_profile', table), PROFILE);
		expect(own.rows.map((r) => [r.alarm_db, r.abort_db])).toEqual([
			[[3, 2], [6, 6]],
			[null, null]
		]);
		const none = profileRows(loaded('random_profile', table), { ...PROFILE, bands: false });
		expect(none.rows.map((r) => r.alarm_db)).toEqual([null, null]);
		expect(none.notes).toEqual([{ level: 'warning', message: "Left out the bands on 1 row: this table's rows have no bands of their own." }]);
	});

	it('keeps the PSD of a row that gives a slope too, and says so', () => {
		const table: TableData = { columns: ['freq_hz', 'psd_g2_hz', 'slope_db_oct'], rows: [[20, 0.01, null], [80, 0.04, 3], [350, 0.04, -3]], x_unit: 'Hz', y_unit: 'g²/Hz' };
		const { rows, notes } = profileRows(loaded('random_profile', table), PROFILE);
		expect(rows.map((r) => r.kind)).toEqual(['psd', 'psd', 'psd']);
		expect(notes[0].message).toBe('rows 2 and 3 give a PSD and a slope; the PSD was kept.');
	});

	it('reads a square written another way, and refuses other units or no rows in words', () => {
		const table = profileTable([newBreakpoint(20, 'psd', 0.01), newBreakpoint(2000, 'psd', 0.01)], PROFILE);
		expect(profileRows(loaded('random_profile', { ...table, y_unit: 'g^2/Hz' }), PROFILE).rows).toHaveLength(2);
		const refused = (t: TableData, unit = 'g²/Hz') => () => profileRows(loaded('random_profile', t), { ...PROFILE, unit });
		expect(refused({ ...table, x_unit: 'kHz' })).toThrow(new TableRefusal("Its frequencies are in kHz; this table's are in Hz."));
		expect(refused({ ...table, y_unit: 'lbf²/Hz' })).toThrow("Its values are in lbf²/Hz; this table's are in g²/Hz.");
		expect(refused(table, 'in/s²/Hz')).toThrow("Its values are in g²/Hz; this table's are in in/s²/Hz.");
		expect(refused({ ...table, rows: [] })).toThrow('It has no rows.');
	});

	it('writes a field that holds no number as an empty cell, and reads it back as none', () => {
		const rows: BreakpointDraft[] = [newBreakpoint(NaN, 'psd', NaN), newBreakpoint(80, 'slope', 3)];
		const table = profileTable(rows, PROFILE);
		expect(table.rows[0]).toEqual([null, null, null, null, null, null, null]);
		expect(profileRows(loaded('random_profile', table), PROFILE).rows).toEqual(rows);
	});
});

describe('a Sine profile and a Sine bound', () => {
	const saved = frame('request_tables_save_bound').params as unknown as TableData;
	const BOUND = { tableKind: 'sine_limit', bands: false } as const;
	const SINE = { tableKind: 'sine_profile', bands: true } as const;

	it('reads the golden bound and gives it back unchanged', () => {
		const { rows, notes } = sineRows(loaded('sine_limit', saved), BOUND);
		expect(rows.map((r) => [r.freq_hz, r.quantity, r.value, r.own_bands])).toEqual([
			[5, 'disp_pp_in', 0.5, false],
			[20, 'velocity_ips', 2, false],
			[100, 'accel_g', 3, false],
			[2000, 'accel_g', 3, false]
		]);
		expect(notes).toEqual([]);
		expect(shape(sineTable(rows, BOUND))).toEqual({ columns: saved.columns, rows: saved.rows, x_unit: 'Hz', y_unit: '' });
	});

	it('keeps a profile and its rows own bands through a file, as the run is sent', () => {
		const p = defaultSineSetup().profile;
		p.rows = [newRow(5, 'disp_pp_in', 0.5), { ...newRow(20, 'velocity_ips', 2), own_bands: true, alarm_db: [1, 2], abort_db: [3, 4] }, newRow(2000, 'accel_g', 1)];
		const table = sineTable(p.rows, SINE);
		expect(table.columns).toEqual(['freq_hz', 'quantity', 'value', 'alarm_below_db', 'alarm_above_db', 'abort_below_db', 'abort_above_db']);
		const back = sineRows(loaded('sine_profile', table), { ...SINE, base: p }).rows;
		expect(profileOf({ ...p, rows: back })).toEqual(profileOf(p));
		expect(sineTable(back, SINE)).toEqual(table);
	});

	it('reads every row in the unit a file states where it names no quantities, and says so', () => {
		const table: TableData = { columns: ['freq_hz', 'value'], rows: [[10, 1], [100, 2]], x_unit: 'Hz', y_unit: 'in/s pk' };
		const { rows, notes } = sineRows(loaded('sine_profile', table), SINE);
		expect(rows.map((r) => r.quantity)).toEqual(['velocity_ips', 'velocity_ips']);
		expect(notes).toEqual([{ level: 'note', message: 'Read every value in in/s pk, the unit the file gives.' }]);
		expect(sineRows(loaded('sine_profile', { ...table, y_unit: 'in pk-pk' }), SINE).rows[0].quantity).toBe('disp_pp_in');
	});

	it('refuses a stated unit its rows do not agree with, or one a Sine table has not', () => {
		const table: TableData = { columns: ['freq_hz', 'quantity', 'value'], rows: [[10, 'accel_g', 1], [20, 'velocity_ips', 2], [30, 'disp_pp_in', 0.1]], x_unit: 'Hz', y_unit: 'g' };
		expect(() => sineRows(loaded('sine_profile', table), SINE)).toThrow('It says its values are in g pk, but rows 2 and 3 are in another unit.');
		expect(() => sineRows(loaded('sine_profile', { ...table, y_unit: 'm/s²' }), SINE)).toThrow("Its values are in m/s²; a Sine table's are in g pk, in/s pk, in pk-pk, each row in its own.");
		expect(() => sineRows(loaded('sine_profile', { ...table, y_unit: 'in' }), SINE)).toThrow('Its values are in in;');
	});

	it('leaves out bands a bound has none of, and says how many rows had them', () => {
		const table = sineTable([{ ...newRow(20), own_bands: true }, newRow(2000)], SINE);
		const { rows, notes } = sineRows(loaded('sine_profile', table), BOUND);
		expect(rows.map((r) => r.own_bands)).toEqual([false, false]);
		expect(notes[0].message).toBe("Left out the bands on 1 row: this table's rows have no bands of their own.");
	});
});

describe('two columns: a limit, an SRS target and a drive gain', () => {
	const opened = frame('response_tables_load_gain').result as unknown as LoadedTable;
	const GAIN: PointsFile<SrsFrfPointDraft> = { keys: ['freq_hz', 'mag'], column: 'gain', unit: 'g/V' };
	const TARGET: PointsFile<SrsPointDraft> = { keys: ['freq_hz', 'accel_g'], column: 'accel_g', unit: 'g' };
	const LIMIT: PointsFile<LimitProfile['rows'][number]> = { keys: ['freq_hz', 'ceiling'], column: 'ceiling', unit: 'lbf²/Hz' };

	it('reads the golden gain and gives it back unchanged', () => {
		const { rows } = pointsRows(opened, GAIN);
		expect(rows).toEqual([
			{ freq_hz: 50, mag: 0.8 },
			{ freq_hz: 400, mag: 4 },
			{ freq_hz: 2000, mag: 1.5 }
		]);
		expect(shape(pointsTable(rows, GAIN))).toEqual({ columns: opened.columns, rows: opened.rows, x_unit: 'Hz', y_unit: 'g/V' });
	});

	it('keeps a target and a limit through a file', () => {
		const target: SrsPointDraft[] = [
			{ freq_hz: 100, accel_g: 40 },
			{ freq_hz: 1000, accel_g: 100 },
			{ freq_hz: NaN, accel_g: 100 }
		];
		const table = pointsTable(target, TARGET);
		expect(table).toEqual({ columns: ['freq_hz', 'accel_g'], rows: [[100, 40], [1000, 100], [null, 100]], x_unit: 'Hz', y_unit: 'g' });
		expect(pointsRows(loaded('srs_target', table), TARGET).rows).toEqual(target);
		const limit = [
			{ freq_hz: 20, ceiling: 1 },
			{ freq_hz: 2000, ceiling: 0.5 }
		];
		expect(pointsRows(loaded('limit', pointsTable(limit, LIMIT)), LIMIT).rows).toEqual(limit);
	});

	it('reads a target in g peak, and refuses a gain in other units', () => {
		const table: TableData = { columns: ['freq_hz', 'accel_g'], rows: [[100, 40]], x_unit: 'Hz', y_unit: 'g pk' };
		expect(pointsRows(loaded('srs_target', table), TARGET).rows).toEqual([{ freq_hz: 100, accel_g: 40 }]);
		expect(() => pointsRows({ ...opened, y_unit: 'lbf/V' }, GAIN)).toThrow("Its values are in lbf/V; this table's are in g/V.");
	});
});

describe('through the mock analysis service', () => {
	async function client() {
		const c = new ServiceClient({ service: 'analysis', url: 'ws://mock/', socketFactory: mockSocketFactory(createAnalysisServer(), 1) });
		c.connect();
		await c.whenOpen();
		return c;
	}

	it('a profile saved opens again as it was, and a file of another kind is refused in words', async () => {
		const c = await client();
		const rows: SineRowDraft[] = [newRow(10, 'velocity_ips', 1), newRow(2000, 'accel_g', 2)];
		const data = sineTable(rows, { tableKind: 'sine_profile', bands: true });
		await saveTable(c, 'breakpoints', 'sine_profile', data, '/tests/sine.csv');
		const back = await loadTable(c, 'breakpoints', 'sine_profile', '/tests/sine.csv', { x_unit: 'Hz', y_unit: '' });
		expect(sineRows(back, { tableKind: 'sine_profile', bands: true }).rows).toEqual(rows);
		await expect(loadTable(c, 'breakpoints', 'sine_limit', '/tests/sine.csv')).rejects.toThrow('sine.csv holds a sine profile, not a sine limit');
		c.close();
	});
});
