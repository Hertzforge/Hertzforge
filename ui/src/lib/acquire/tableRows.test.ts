/**
 * The channel table sized from the station: one row per hardware channel, off until switched
 * on, rows that are set kept through a rescan, rows for hardware that went shown missing; and
 * an edit made to several rows at once.
 */
import { describe, expect, it } from 'vitest';
import type { ChannelRow, NiDevice, StationList } from '$lib/protocol/types';
import { bulkEdit, freeChannels, hardwareOf, materialize, moveOnto, newRow, rolesFor, sizeTable, type Hardware } from './table';

const ch = (prefix: string, n: number) => Array.from({ length: n }, (_, k) => ({ id: `${prefix}${k}`, label: `${prefix}${k}` }));

function device(id: string, inputs: number, outputs: number, slot = 1): NiDevice {
	return { id, product: 'NI 9234', serial: '', simulated: true, parent: 'cDAQ1', slot, timing_family: 'cDAQ1', inputs: ch('ai', inputs), outputs: ch('ao', outputs) };
}

function stations(selected: 'sim' | string[], devices: NiDevice[] = []): StationList {
	const ni = Array.isArray(selected);
	return {
		stations: [
			{ kind: 'sim', title: 'Simulator', available: true, class: 'sim', inputs: 8, outputs: 1, clock: 'shared' },
			{ kind: 'ni', title: 'NI-DAQmx', available: true, unavailable: null, scanned_unix: 1, devices }
		],
		selected: ni
			? { kind: 'ni', devices: selected, class: 'ni_simulated', source: 'HARDWARE', inputs: 0, outputs: 0, timing_family: 'cDAQ1' }
			: { kind: 'sim', devices: [], class: 'sim', source: 'SIM', inputs: 8, outputs: 1, timing_family: null }
	};
}

const bound = (row: ChannelRow, device: string, channel: string): ChannelRow => ({ ...row, physical: { device, channel } });
const RIG = [device('cDAQ1Mod1', 4, 0, 1), device('cDAQ1Mod2', 0, 2, 2)];
const ni = (devices = RIG): Hardware => hardwareOf(stations(devices.map((d) => d.id), devices))!;

describe('sizing the table from the station', () => {
	it('on NI, a row for every input and output of the selected devices, each off', () => {
		const sized = sizeTable([], ni());
		expect(sized.map((s) => `${s.row.physical?.device}/${s.row.physical?.channel}`)).toEqual([
			'cDAQ1Mod1/ai0',
			'cDAQ1Mod1/ai1',
			'cDAQ1Mod1/ai2',
			'cDAQ1Mod1/ai3',
			'cDAQ1Mod2/ao0',
			'cDAQ1Mod2/ao1'
		]);
		expect(sized.every((s) => s.virtual && !s.row.enabled && s.status === 'present')).toBe(true);
		expect(sized.map((s) => s.row.role)).toEqual(['monitor', 'monitor', 'monitor', 'monitor', 'drive', 'drive']);
		expect(sized.map((s) => s.row.index)).toEqual([1, 2, 3, 4, 5, 6]);
		expect(sized[0].row.interface).toBe('single_ended');
	});

	it('only the selected devices size it', () => {
		const devices = [...RIG, device('cDAQ2Mod1', 8, 0)];
		const hw = hardwareOf(stations(['cDAQ1Mod1'], devices))!;
		expect(sizeTable([], hw)).toHaveLength(4);
	});

	it('rows already bound keep their settings and their place; the rest of the hardware is added after', () => {
		const control = { ...bound(newRow(7, 'control'), 'cDAQ1Mod1', 'ai2'), sensitivity_mv_per_eu: 10.2 };
		const sized = sizeTable([control], ni());
		expect(sized[0]).toMatchObject({ row: control, status: 'present', virtual: false, direction: 'input' });
		expect(sized.slice(1).map((s) => s.row.physical?.channel)).toEqual(['ai0', 'ai1', 'ai3', 'ao0', 'ao1']);
		// numbered on from the highest the table has
		expect(sized.slice(1).map((s) => s.row.index)).toEqual([8, 9, 10, 11, 12]);
	});

	it('a row on a channel the station no longer has stays, missing; an unbound row is not bound', () => {
		const gone = bound(newRow(1, 'control'), 'cDAQ9Mod1', 'ai0');
		const loose = newRow(2, 'drive');
		const sized = sizeTable([gone, loose], ni());
		expect(sized.slice(0, 2).map((s) => s.status)).toEqual(['missing', 'unbound']);
		expect(sized.filter((s) => s.virtual)).toHaveLength(6);
		// a rescan that finds it again puts it back on its channel
		const back = sizeTable([gone, loose], ni([...RIG, device('cDAQ9Mod1', 1, 0, 3)]));
		expect(back[0].status).toBe('present');
	});

	it('a second row on one channel is extra', () => {
		const a = bound(newRow(1, 'control'), 'cDAQ1Mod1', 'ai0');
		const b = bound(newRow(2, 'monitor'), 'cDAQ1Mod1', 'ai0');
		expect(sizeTable([a, b], ni()).slice(0, 2).map((s) => s.status)).toEqual(['present', 'extra']);
	});

	it('on the simulator, one row per input and output, any role on any row', () => {
		const sim = hardwareOf(stations('sim'))!;
		const start = [newRow(1, 'control'), newRow(2, 'drive')];
		const sized = sizeTable(start, sim);
		expect(sized).toHaveLength(9);
		expect(sized.filter((s) => s.virtual).map((s) => s.row.role)).toEqual(Array(7).fill('monitor'));
		expect(sized.every((s) => s.status === 'present' && s.direction === null)).toBe(true);
		// four drives, as a test of several shakers runs on the simulator: still eight inputs
		const drives = bulkEdit(materialize(sized), sim, [2, 3, 4], { field: 'role', value: 'drive' });
		expect(drives.skipped).toBe(0);
		const after = sizeTable(drives.rows, sim);
		expect(after.filter((s) => s.row.role === 'drive')).toHaveLength(4);
		expect(after.filter((s) => s.row.role !== 'drive')).toHaveLength(8);
	});

	it('with no station known, the rows as they are', () => {
		const rows = [newRow(1, 'control')];
		expect(sizeTable(rows, null)).toEqual([{ row: rows[0], status: 'present', virtual: false, direction: null }]);
	});

	it('the roles a row may take follow its hardware', () => {
		expect(rolesFor('input')).toEqual(['control', 'monitor', 'reference']);
		expect(rolesFor('output')).toEqual(['drive']);
		expect(rolesFor(null)).toHaveLength(4);
	});
});

describe('editing several rows at once', () => {
	const hw = ni();

	it('an edit reaches the rows chosen and nothing else, adding the hardware rows when it reaches one', () => {
		const control = { ...bound(newRow(1, 'control'), 'cDAQ1Mod1', 'ai0'), enabled: true };
		const { rows, changed, skipped } = bulkEdit([control], hw, [1, 2, 3], { field: 'enabled', value: true });
		expect([changed, skipped]).toEqual([3, 0]);
		// every hardware row is now in the table, numbered as it was shown
		expect(rows.map((r) => r.index)).toEqual([1, 2, 3, 4, 5, 6]);
		expect(rows.map((r) => r.enabled)).toEqual([true, true, true, true, false, false]);
		expect(rows[0]).toEqual(control);
	});

	it('a row that cannot take the edit is left and counted', () => {
		const table = materialize(sizeTable([], hw));
		const role = bulkEdit(table, hw, [0, 1, 4], { field: 'role', value: 'control' });
		expect(role.rows.map((r) => r.role)).toEqual(['control', 'control', 'monitor', 'monitor', 'drive', 'drive']);
		expect([role.changed, role.skipped]).toEqual([2, 1]);
		const iepe = bulkEdit(table, hw, [0, 4], { field: 'iepe', value: true });
		expect(iepe.rows.map((r) => r.interface)).toEqual(['iepe', 'single_ended', 'single_ended', 'single_ended', 'single_ended', 'single_ended']);
		expect(iepe.skipped).toBe(1);
	});

	it('number fields, IEPE off and abort limits apply as one row would', () => {
		let rows = materialize(sizeTable([], hw));
		rows = bulkEdit(rows, hw, [0, 1], { field: 'iepe', value: true }).rows;
		rows = bulkEdit(rows, hw, [0, 1], { field: 'iepe_bias_v', value: 24 }).rows;
		rows = bulkEdit(rows, hw, [0, 1], { field: 'sensitivity_mv_per_eu', value: 10.2 }).rows;
		rows = bulkEdit(rows, hw, [0, 1, 4], { field: 'abort_accel_g', value: 30 }).rows;
		expect(rows[0]).toMatchObject({ interface: 'iepe', iepe_bias_v: 24, sensitivity_mv_per_eu: 10.2, abort: { accel_g: 30 } });
		expect(rows[4].abort.accel_g).toBeNull();
		rows = bulkEdit(rows, hw, [0], { field: 'iepe', value: false }).rows;
		expect([rows[0].interface, rows[0].iepe_bias_v, rows[1].iepe_bias_v]).toEqual(['single_ended', null, 24]);
	});

	it('a channel number is set on one row only', () => {
		const rows = materialize(sizeTable([], hw));
		const { rows: out, changed } = bulkEdit(rows, hw, [0, 1], { field: 'index', value: 40 });
		expect(changed).toBe(1);
		expect(out.map((r) => r.index)).toEqual([40, 2, 3, 4, 5, 6]);
	});

	it('the rows given are never changed', () => {
		const rows = [bound(newRow(1, 'control'), 'cDAQ1Mod1', 'ai0')];
		const before = structuredClone(rows);
		bulkEdit(rows, hw, [0], { field: 'description', value: 'Table' });
		expect(rows).toEqual(before);
	});
});

describe('moving a row onto the hardware', () => {
	const hw = ni();

	it('a missing row moves onto a free channel, taking the place of the off row there', () => {
		const gone = { ...bound(newRow(1, 'control'), 'cDAQ9Mod1', 'ai0'), sensitivity_mv_per_eu: 9.81 };
		const sized = sizeTable([gone], hw);
		expect(freeChannels(sized, hw, 0).map((c) => c.key)).toEqual(['cDAQ1Mod1/ai0', 'cDAQ1Mod1/ai1', 'cDAQ1Mod1/ai2', 'cDAQ1Mod1/ai3']);
		const moved = moveOnto([gone], hw, 0, 'cDAQ1Mod1/ai2');
		expect(moved).toHaveLength(6);
		expect(moved[0]).toMatchObject({ index: 1, sensitivity_mv_per_eu: 9.81, physical: { device: 'cDAQ1Mod1', channel: 'ai2' } });
		expect(moved.filter((r) => r.physical?.channel === 'ai2')).toHaveLength(1);
		expect(sizeTable(moved, hw).every((s) => s.status === 'present' && !s.virtual)).toBe(true);
	});

	it('a channel whose row is on is not offered', () => {
		const on = { ...bound(newRow(1, 'monitor'), 'cDAQ1Mod1', 'ai1'), enabled: true };
		const loose = newRow(2, 'control');
		const sized = sizeTable([on, loose], hw);
		expect(freeChannels(sized, hw, 1).map((c) => c.channel)).toEqual(['ai0', 'ai2', 'ai3']);
		expect(freeChannels(sized, hardwareOf(stations('sim')), 1)).toEqual([]);
	});
});
