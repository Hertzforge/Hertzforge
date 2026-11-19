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
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { ChannelRow } from '$lib/protocol/types';
import type { ChannelEnvironment } from '$lib/tables/tableFiles';

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
		// the change of station bound the two rows; nothing is left to bind
		expect(acq.bindRows()).toBe(0);
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

	it('a change of station fits the table to it: unbound rows take the free channels, untouched rows go', async () => {
		const { acq, control, analysis } = await screen();
		await acq.claim();
		acq.editRows([2, 3, 4, 5], { field: 'enabled', value: true });
		acq.editRows([2, 3, 4, 5], { field: 'role', value: 'control' });
		expect(acq.sized).toHaveLength(9);
		acq.chooseStation('ni');
		acq.toggleDevice('cDAQ1Mod1');
		acq.toggleDevice('cDAQ1Mod2');
		expect(await acq.useStation()).toBe(true);
		// the service's word on the station change arrives too, and the fit holds
		await new Promise((r) => setTimeout(r, 50));
		const key = (r: { physical: { device: string; channel: string } | null }) => (r.physical ? `${r.physical.device}/${r.physical.channel}` : null);
		expect(acq.rows.map((r) => [r.index, r.role, key(r)])).toEqual([
			[1, 'control', 'cDAQ1Mod1/ai0'],
			[2, 'drive', 'cDAQ1Mod2/ao0'],
			[3, 'control', 'cDAQ1Mod1/ai1'],
			[4, 'control', 'cDAQ1Mod1/ai2'],
			[5, 'control', 'cDAQ1Mod1/ai3'],
			[6, 'control', null]
		]);
		expect(acq.sized).toHaveLength(9);
		expect(acq.sized.map((s) => s.status)).toEqual(['present', 'present', 'present', 'present', 'present', 'unbound', 'present', 'present', 'present']);
		await acq.checkTable();
		expect(acq.tableCheck?.errors.map((e) => [e.reason, e.channel])).toEqual([['channel_unbound', 6]]);

		// and back to the simulator: bindings kept, the untouched drives past its one output gone
		acq.editRows([5], { field: 'enabled', value: false });
		acq.materializeTable();
		acq.chooseStation('sim');
		expect(await acq.useStation()).toBe(true);
		await new Promise((r) => setTimeout(r, 50));
		expect(acq.rows.map((r) => r.index)).toEqual([1, 2, 3, 4, 5, 6]);
		expect(key(acq.rows[0])).toBe('cDAQ1Mod1/ai0');
		expect(acq.sized).toHaveLength(9);
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
	it('saves every row shown with its analysis columns, and opens it again in place of the draft', async () => {
		const { acq, control, analysis } = await screen();
		acq.editRows([0], { field: 'sensitivity_mv_per_eu', value: 10.2 });
		acq.materializeTable();
		acq.rows[0] = { ...acq.rows[0], node: 101, direction: 'Z+', serial_number: 'SN0', extra: { cable: 'C-1' } };
		const saved = acq.tableData();
		expect(saved.channels).toHaveLength(9);
		expect(saved.columns).toEqual([]);
		const environments = { Random: { controls: [1], drives: [2], response_transformation: null, reference_transformation: null } };
		await saveTable(analysis, 'channels', null, { ...saved, environments }, '/tests/bench/channels');
		acq.discardTable();
		expect(acq.rows).toHaveLength(2);

		acq.openTable(await loadTable(analysis, 'channels', null, '/tests/bench/channels.hforge'), '/tests/bench/channels.hforge');
		expect(acq.rows).toHaveLength(9);
		expect(acq.rows[0].sensitivity_mv_per_eu).toBe(10.2);
		expect(acq.rows[0].node).toBe(101);
		expect(acq.rows[0].extra).toEqual({ cable: 'C-1' });
		expect(acq.tableFile).toBe('/tests/bench/channels.hforge');
		expect(acq.tableEnvironments).toEqual(environments);
		// an edit, then a save: the analysis columns and the loops go back unchanged
		acq.editRows([1], { field: 'description', value: 'Drive A' });
		const again = acq.tableData();
		expect(again.channels?.[0].serial_number).toBe('SN0');
		expect(again.channels?.[1].description).toBe('Drive A');
		expect(again.environments).toEqual(environments);
		// a channel renumbered keeps its columns: they are the row's own
		acq.editRows([0], { field: 'index', value: 30 });
		expect(acq.rows[0].index).toBe(30);
		expect(acq.rows[0].direction).toBe('Z+');
		control.close();
		analysis.close();
	});

	it("the round trip's shell leg: the golden request's rows open and write back unchanged", async () => {
		// The same rows the control service's and the analysis side's tests read.
		const golden = JSON.parse(readFileSync(resolve(process.cwd(), '../schema/golden/request_tables_save_channels.json'), 'utf-8')) as {
			params: { channels: ChannelRow[]; environments: Record<string, ChannelEnvironment> };
		};
		const { acq, control, analysis } = await screen();
		acq.openTable({ columns: [], rows: [], channels: golden.params.channels, environments: golden.params.environments });
		acq.materializeTable();
		const out = acq.tableData();
		const byIndex = new Map((out.channels ?? []).map((r) => [r.index, r]));
		for (const row of golden.params.channels) expect(byIndex.get(row.index)).toEqual(row);
		expect(out.environments).toEqual(golden.params.environments);
		control.close();
		analysis.close();
	});

	it('the analysis columns survive applying the table and a page reload', async () => {
		const server = createControlServer({ speed: 10 });
		const open = async () => {
			const control = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
			const acq = new AcquireState(control, { name: 'Shell on bench-pc', session: null, local: null, checkDelayMs: 0 });
			control.connect();
			await control.whenOpen();
			await acq.sync();
			return { acq, control };
		};
		const first = await open();
		await first.acq.claim();
		first.acq.rows[0] = { ...first.acq.rows[0], node: 7, direction: 'X-', make: 'Maker', limit: true };
		expect(await first.acq.applyTable()).toBe(true);
		first.control.close();
		// a new page: nothing of the first is in memory, the service holds the table
		const reloaded = await open();
		await until(() => reloaded.acq.server !== null);
		const row = reloaded.acq.server!.channels[0];
		expect([row.node, row.direction, row.make, row.limit]).toEqual([7, 'X-', 'Maker', true]);
		expect(reloaded.acq.rows[0].node).toBe(7);
		reloaded.control.close();
	});

	it("discarding a file's rows brings back the table applied and its loops", async () => {
		const { acq, control, analysis } = await screen();
		await acq.claim();
		const loops = { A: { controls: [1], drives: [2], response_transformation: null, reference_transformation: null } };
		acq.openTable({ columns: [], rows: [], channels: acq.rows.map((r) => ({ ...r, node: r.index + 6 })), environments: loops });
		expect(await acq.applyTable()).toBe(true);
		acq.openTable({ columns: [], rows: [], channels: [{ ...acq.rows[0], node: 99 }], environments: {} });
		expect(acq.rows[0].node).toBe(99);
		acq.discardTable();
		expect(acq.rows[0].node).toBe(7);
		expect(acq.tableEnvironments).toEqual(loops);
		control.close();
		analysis.close();
	});
});
