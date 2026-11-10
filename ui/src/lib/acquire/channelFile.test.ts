/**
 * A control row as a row of the one channel table, and back. The golden request is the channel
 * table the shell saves for five channels; the analysis service's tests read it back from every
 * file form unchanged, and these check the rows it was made from come back from it unchanged.
 */
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame } from '$lib/protocol/codec';
import type { ChannelRow } from '$lib/protocol/types';
import type { TableData } from '$lib/tables/tableFiles';
import { CONTROL_COLUMNS, cellsOf, channelFile, channelRowsFrom, renumbered, roleOf, rowOf, type ChannelExtras } from './channelFile';
import { newRow } from './table';

// a path rather than a URL beside this module: the stores' build would take the URL for an asset
const golden = (name: string) => resolve(dirname(fileURLToPath(import.meta.url)), '../../../../schema/golden', name);
const SAVED = (decodeFrame(readFileSync(golden('request_tables_save.json'), 'utf-8')) as unknown as { params: { columns: string[]; rows: TableData['rows'] } }).params;

const row = (over: Partial<ChannelRow>): ChannelRow => ({ ...newRow(1, 'monitor'), ...over });

/** The five channels the golden table was saved from: every setting a control row has. */
const ROWS: ChannelRow[] = [
	row({ index: 1, description: 'Table top', role: 'control', physical: { device: 'cDAQ1Mod1', channel: 'ai0' }, interface: 'iepe', iepe_bias_v: 24, units: 'g', sensitivity_mv_per_eu: 10.2, range_max: 50, control_group: 0, abort: { accel_g: 30, velocity_ips: null, displacement_mil: null } }),
	row({ index: 2, description: 'Fixture', role: 'reference', physical: { device: 'cDAQ1Mod1', channel: 'ai1' }, units: 'lbf', sensitivity_mv_per_eu: 2.25, range_max: 500, polarity: 'negative' }),
	row({ index: 3, enabled: false, description: '', role: 'monitor', units: 'in/s', sensitivity_mv_per_eu: 100, range_max: 100, abort: { accel_g: null, velocity_ips: 5.5, displacement_mil: 200 } }),
	row({ index: 4, description: 'Drive', role: 'drive', physical: { device: 'cDAQ1Mod2', channel: 'ao0' }, units: 'V', sensitivity_mv_per_eu: 1000, range_max: 10 }),
	row({ index: 5, description: 'Mil gauge', role: 'control', physical: { device: 'cDAQ1Mod1', channel: 'ai2' }, units: 'mil', sensitivity_mv_per_eu: 50, range_max: 20, polarity: 'negative', control_group: 1, abort: { accel_g: null, velocity_ips: null, displacement_mil: 300 } })
];

/** The analysis columns the table had for those channels, which the control table carries untouched. */
const KEPT: ChannelExtras = {
	1: { role: 'response', node: 101, direction: 'Z+', serial_number: 'A1023' },
	2: { role: 'reference', node: 7, direction: 'X-', serial_number: '' },
	3: { role: '', node: null, direction: '', serial_number: '' },
	4: { role: '', node: null, direction: '', serial_number: '' },
	5: { role: '', node: null, direction: '', serial_number: '' }
};

describe('a control row as a channel table row', () => {
	it('writes the golden table from the control rows and the columns they carry', () => {
		expect(channelFile(ROWS, KEPT)).toEqual({ columns: SAVED.columns, rows: SAVED.rows });
	});

	it('reads the golden table back to the same rows, every column kept', () => {
		const { rows, extras, order } = channelRowsFrom({ columns: SAVED.columns, rows: SAVED.rows });
		expect(rows).toEqual(ROWS);
		expect(order).toEqual(SAVED.columns);
		// and writes the same table again
		expect(channelFile(rows, extras, order)).toEqual({ columns: SAVED.columns, rows: SAVED.rows });
	});

	it('a table opened for a test carries every column it had through to the save', () => {
		const columns = [...SAVED.columns, 'channel_type', 'make', 'model', 'expiration'];
		const rows = SAVED.rows.map((r, k) => [...r, ['acceleration', 'force', 'velocity', 'voltage', 'length'][k], 'Acme', 'T1', '2027-03-01']);
		const read = channelRowsFrom({ columns, rows });
		read.rows[0].description = 'Table top, edited';
		const written = channelFile(read.rows, read.extras, read.order);
		expect(written.columns).toEqual(columns);
		expect(written.rows.map((r) => r.slice(-3))).toEqual(rows.map(() => ['Acme', 'T1', '2027-03-01']));
		expect(written.rows[0][columns.indexOf('comment')]).toBe('Table top, edited');
	});

	it("a row's role in the loop is three of the table's columns", () => {
		expect(roleOf({ drive: true, control: true })).toBe('drive');
		expect(roleOf({ control: true, role: 'reference' })).toBe('control');
		expect(roleOf({ role: 'reference' })).toBe('reference');
		expect(roleOf({ role: 'response' })).toBe('monitor');
		expect(roleOf({})).toBe('monitor');
		for (const role of ['control', 'monitor', 'reference', 'drive'] as const) {
			for (const kept of ['', 'response', 'reference', 'monitor']) {
				expect(roleOf(cellsOf(row({ role }), { role: kept }))).toBe(role);
			}
		}
		// the table's own role is kept where the loop's allows: a controlled force stays a reference
		expect(cellsOf(row({ role: 'control' }), { role: 'reference' }).role).toBe('reference');
		expect(cellsOf(row({ role: 'monitor' }), { role: 'reference' }).role).toBe('');
		expect(cellsOf(row({ role: 'monitor' }), { role: 'response' }).role).toBe('response');
	});

	it('a unit changed here leaves the channel type for the table to work out again', () => {
		const kept = { unit: 'g', channel_type: 'acceleration' };
		expect(cellsOf(row({ units: 'g' }), kept).channel_type).toBe('acceleration');
		expect(cellsOf(row({ units: 'V' }), kept).channel_type).toBe('');
	});

	it('a table that says nothing of a column gives a new row its values; a table without on and off is on', () => {
		expect(rowOf({ channel: 3 })).toEqual({ ...newRow(3, 'monitor'), description: '' });
		const drive = rowOf({ channel: 4, drive: true });
		expect([drive.units, drive.sensitivity_mv_per_eu, drive.range_max]).toEqual(['V', 1000, 10]);
		expect(rowOf({ channel: 1, physical: 'ai0' }).physical).toBeNull();
		expect(rowOf({ channel: 1, physical: 'cDAQ1Mod1/ai0' }).physical).toEqual({ device: 'cDAQ1Mod1', channel: 'ai0' });
	});

	it('a new file gets the control columns, and a number that is not one goes as none', () => {
		const table = channelFile([row({ sensitivity_mv_per_eu: NaN })]);
		expect(table.columns).toEqual([...CONTROL_COLUMNS]);
		expect(table.rows[0][table.columns.indexOf('sensitivity')]).toBeNull();
	});

	it('the carried columns follow a channel renumbered', () => {
		expect(renumbered({ 1: { node: 101 }, 2: { node: 7 } }, 1, 9)).toEqual({ 9: { node: 101 }, 2: { node: 7 } });
		const same = { 1: { node: 101 } };
		expect(renumbered(same, 3, 4)).toBe(same);
	});
});
