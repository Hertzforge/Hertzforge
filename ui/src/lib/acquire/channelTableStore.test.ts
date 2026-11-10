/**
 * The channel table as the store keeps it, against the mock control and analysis services: sized
 * from the station without being edited, several rows edited at once, and the table saved to a
 * file and opened again with every column it carried.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createAnalysisServer } from '$lib/protocol/mock/analysis';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { loadTable, saveTable } from '$lib/tables/tableFiles';
import { AcquireState } from './store.svelte';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

async function screen() {
	const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 10 }), 1) });
	const analysis = new ServiceClient({ service: 'analysis', url: 'ws://mock-a/', socketFactory: mockSocketFactory(createAnalysisServer(), 1) });
	const acq = new AcquireState(control, { name: 'Shell on bench-pc', session: null, local: null, checkDelayMs: 0, tables: analysis });
	control.connect();
	analysis.connect();
	await Promise.all([control.whenOpen(), analysis.whenOpen()]);
	await acq.sync();
	return { acq, control, analysis };
}

describe('the channel table sized from the station', () => {
	it('shows a row for every simulator channel without editing the table', async () => {
		const { acq, control, analysis } = await screen();
		// the service's starting table: a control input and a drive
		expect(acq.rows).toHaveLength(2);
		expect(acq.sized).toHaveLength(9);
		expect(acq.sized.slice(2).every((s) => s.virtual && !s.row.enabled)).toBe(true);
		expect(acq.tableDirty).toBe(false);
		control.close();
		analysis.close();
	});

	it('on NI, every channel of the selected devices; a row on a device taken away is missing', async () => {
		const { acq, control, analysis } = await screen();
		await acq.claim();
		acq.chooseStation('ni');
		acq.toggleDevice('cDAQ1Mod1');
		acq.toggleDevice('cDAQ1Mod2');
		expect(await acq.useStation()).toBe(true);
		expect(acq.bindRows()).toBe(2);
		// the two bound rows, then the other six channels of the two modules, off
		expect(acq.sized.map((s) => s.status)).toEqual(Array(8).fill('present'));
		expect(acq.sized.filter((s) => s.virtual)).toHaveLength(6);
		expect(await acq.applyTable()).toBe(true);
		acq.toggleDevice('cDAQ1Mod2');
		expect(await acq.useStation()).toBe(true);
		await until(() => acq.sized.some((s) => s.status === 'missing'));
		expect(acq.sized.map((s) => s.status)).toEqual(['present', 'missing', 'present', 'present', 'present']);
		control.close();
		analysis.close();
	});

	it('an edit to several rows reaches them all, and the hardware rows join the table', async () => {
		const { acq, control, analysis } = await screen();
		const { changed } = acq.editRows([1, 2, 3], { field: 'description', value: 'Fixture' });
		expect(changed).toBe(3);
		expect(acq.rows).toHaveLength(9);
		expect(acq.rows.map((r) => r.description)).toEqual(['Control', 'Fixture', 'Fixture', 'Fixture', '', '', '', '', '']);
		expect(acq.rows.map((r) => r.index)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9]);
		expect(acq.tableDirty).toBe(true);
		await acq.checkTable();
		expect(acq.tableCheck?.errors).toEqual([]);
		control.close();
		analysis.close();
	});
});

describe('the channel table as a file', () => {
	it('saves every row shown, and opens it again in place of the draft with the columns it carried', async () => {
		const { acq, control, analysis } = await screen();
		acq.editRows([0], { field: 'sensitivity_mv_per_eu', value: 10.2 });
		const saved = acq.tableData();
		expect(saved.rows).toHaveLength(9);
		// a table from the analysis side: a node and a serial number the control table does not edit
		const columns = [...saved.columns, 'node', 'serial_number'];
		const rows = saved.rows.map((r, k) => [...r, 100 + k, `SN${k}`]);
		await saveTable(analysis, 'channels', null, { columns, rows }, '/tests/bench/channels');
		acq.discardTable();
		expect(acq.rows).toHaveLength(2);

		acq.openTable(await loadTable(analysis, 'channels', null, '/tests/bench/channels.hforge'), '/tests/bench/channels.hforge');
		expect(acq.rows).toHaveLength(9);
		expect(acq.rows[0].sensitivity_mv_per_eu).toBe(10.2);
		expect(acq.tableFile).toBe('/tests/bench/channels.hforge');
		// an edit, then a save: the carried columns go back unchanged
		acq.editRows([1], { field: 'description', value: 'Drive A' });
		const again = acq.tableData();
		expect(again.columns).toEqual(columns);
		expect(again.rows.map((r) => r.slice(-2))).toEqual(rows.map((r) => r.slice(-2)));
		expect(again.rows[1][again.columns.indexOf('comment')]).toBe('Drive A');
		// a channel renumbered keeps its columns
		acq.editRows([2], { field: 'index', value: 30 });
		expect(acq.tableExtras[30]?.serial_number).toBe('SN2');
		control.close();
		analysis.close();
	});

	it('a removed row takes its carried columns with it', async () => {
		const { acq, control, analysis } = await screen();
		acq.openTable({ columns: ['channel', 'node', 'drive'], rows: [[1, 7, false], [2, 8, true], [3, 9, false]] });
		acq.removeRow(2);
		expect(Object.keys(acq.tableExtras)).toEqual(['1', '2']);
		control.close();
		analysis.close();
	});
});
