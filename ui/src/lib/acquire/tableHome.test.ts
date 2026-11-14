/**
 * The channel table's home and the restored state, against the mock control service through the
 * real client: a table taken from a file or the project arrives restored, every arm is refused in
 * the service's words until the operator applies it, and the project and the service exchange the
 * table with every column, each change shown before it is taken.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { RequestError } from '$lib/protocol/client.svelte';
import type { ChannelRow, ChannelTable, RestoredTable } from '$lib/protocol/types';
import type { TableFileClient } from '$lib/tables/tableFiles';
import { defaultSetup } from './setup';
import { AcquireState } from './store.svelte';
import { applyRows, restoredWords, sourceWords, whatChanges } from './tableHome';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

function row(index: number, role: ChannelRow['role'], extra: Partial<ChannelRow> = {}): ChannelRow {
	const drive = role === 'drive';
	return {
		index,
		enabled: true,
		description: drive ? 'Drive' : 'Control',
		role,
		physical: null,
		interface: 'single_ended',
		iepe_bias_v: null,
		units: drive ? 'V' : 'g',
		sensitivity_mv_per_eu: drive ? 1000 : 100,
		range_max: drive ? 10 : 100,
		polarity: 'positive',
		control_group: null,
		abort: { accel_g: null, velocity_ips: null, displacement_mil: null },
		...extra
	};
}

/** A project holding channel tables by name, as the analysis service answers for one. */
function project(tables: Record<string, ChannelRow[]>): TableFileClient & { put: Record<string, unknown>[] } {
	const put: Record<string, unknown>[] = [];
	return {
		put,
		request: (async (method: string, params: Record<string, unknown> = {}) => {
			if (method === 'project.channel_table') {
				const names = Object.keys(tables);
				const name = (params.name as string | undefined) ?? (names.length === 1 ? names[0] : undefined);
				if (!name || !tables[name]) throw new RequestError('not_found', 'the project holds no channel table', method);
				return { name, channels: structuredClone(tables[name]), tables: names, environments: {}, warnings: [] };
			}
			if (method === 'project.put_channel_table') {
				put.push(params);
				tables[params.name as string] = structuredClone(params.channels as ChannelRow[]);
				return { name: params.name, project: { title: 'Bench', objects: [] } };
			}
			throw new Error(`unexpected ${method}`);
		}) as TableFileClient['request']
	} as TableFileClient & { put: Record<string, unknown>[] };
}

async function controller(tables: TableFileClient | null = null) {
	const server = createControlServer({ speed: 10 });
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
	const acq = new AcquireState(client, { name: 'Shell', session: null, local: null, checkDelayMs: 0, tables });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	expect(await acq.claim()).toBe(true);
	const d = defaultSetup();
	d.acquisition = { fs_hz: 8192, n_pts: 1024, n_avg: 2 };
	acq.setup = d;
	return { server, client, acq };
}

describe('the restored state in words', () => {
	const r: RestoredTable = { source: 'project', name: 'Bracket table', at_unix: 1794240000.25, at: '2026-11-09 16:00:00 UTC' };

	it('names the source and the time, and says no run arms on it', () => {
		const words = restoredWords(r);
		expect(words).toContain('the project table "Bracket table"');
		expect(words).toContain('2026-11-09 16:00:00 UTC');
		expect(words).toContain('no run arms on it');
		expect(sourceWords({ source: 'file', name: 'bench.csv' })).toBe('the file "bench.csv"');
		expect(sourceWords({ source: 'kept', name: 'channel_table.json' })).toContain('kept');
	});

	it('the review shows every row with what a run arms with', () => {
		const rows = applyRows([row(1, 'control', { abort: { accel_g: 40, velocity_ips: null, displacement_mil: null }, polarity: 'negative' }), row(2, 'drive', { enabled: false })]);
		expect(rows).toEqual([
			{ channel: 1, enabled: true, description: 'Control', role: 'control', unit: 'g', sensitivity: '100 mV/g', range: '100 g', polarity: 'negative', abort: '40 g' },
			{ channel: 2, enabled: false, description: 'Drive', role: 'drive', unit: 'V', sensitivity: '1000 mV/V', range: '10 V', polarity: 'positive', abort: 'none' }
		]);
	});

	it('what would change is one line per channel', () => {
		expect(whatChanges(null, [row(1, 'control')])).toEqual(['Channel 1 added (Control).']);
		expect(whatChanges([row(1, 'control')], [row(1, 'control', { sensitivity_mv_per_eu: 98.7 })])).toEqual(['Channel 1: sensitivity changed.']);
	});
});

describe('a restored table cannot arm until it is applied', () => {
	it('a table opened from a file is sent restored, Arm is refused in the service words, and Apply lets it arm', async () => {
		const { acq } = await controller();
		acq.openTable({ columns: [], rows: [], channels: [row(1, 'control'), row(2, 'drive')], environments: {} }, '/bench/plate.csv');
		expect(acq.draftSource).toEqual({ kind: 'file', name: 'plate.csv' });
		expect(await acq.applyTable()).toBe(true);
		await until(() => acq.restored !== null);
		expect(acq.restored?.source).toBe('file');
		expect(acq.restored?.name).toBe('plate.csv');
		expect(acq.draftSource).toBeNull();

		expect(await acq.apply()).toBe(true);
		expect(await acq.arm()).toBe(false);
		expect(acq.refusal?.reason).toBe('table_not_applied');
		expect(acq.refusal?.message).toContain('the file "plate.csv"');
		expect(acq.refusal?.message).toMatch(/ at \d{4}-\d\d-\d\d \d\d:\d\d:\d\d UTC /);

		expect(await acq.applyRestored()).toBe(true);
		expect(acq.restored).toBeNull();
		expect(await acq.arm()).toBe(true);
	});

	it('a table typed here is applied as it is set', async () => {
		const { acq } = await controller();
		acq.addRow('monitor');
		expect(await acq.applyTable()).toBe(true);
		const t = await acq.control.request<ChannelTable>('channels.get');
		expect(t.restored).toBeUndefined();
		expect(await acq.apply()).toBe(true);
		expect(await acq.arm()).toBe(true);
	});

	it('every arm and every start that arms in the mock is held by the one check', async () => {
		const { acq } = await controller();
		acq.openTable({ columns: [], rows: [], channels: [row(1, 'control'), row(2, 'drive')], environments: {} }, 'plate.csv');
		expect(await acq.applyTable()).toBe(true);
		for (const verb of ['random.arm', 'random.start', 'shock.arm', 'shock.start', 'srs.arm', 'srs.start', 'sine.arm', 'sine.start', 'mixed.arm', 'mixed.start', 'twr.arm', 'twr.start', 'explorer.start', 'recorder.start', 'mimo_shock.arm', 'mimo_shock.start']) {
			const err = await acq.control.request(verb, {}).then(
				() => null,
				(e: RequestError) => e
			);
			expect(err?.reason, verb).toBe('table_not_applied');
		}
	});
});

describe('the project is the table home', () => {
	it('Use the Project Channel Table shows what changes, takes it restored, and every column survives', async () => {
		const rows = [row(1, 'control', { node: 12, direction: 'X+', serial_number: 'SN-1', extra: { Cable: 'C7' } }), row(2, 'drive')];
		const home = project({ 'Bench table': rows });
		const { acq } = await controller(home);
		expect(await acq.reviewProjectTable()).toBe(true);
		const review = acq.homeReview;
		expect(review?.direction).toBe('use');
		expect(review?.name).toBe('Bench table');
		expect(review?.changes.join(' ')).toMatch(/Channel 1: .*node, direction, serial number, other columns changed\./);
		expect(review?.report?.errors).toEqual([]);
		// nothing is taken before the operator says so
		expect(acq.restored).toBeNull();

		expect(await acq.takeProjectTable()).toBe(true);
		await until(() => acq.restored !== null);
		expect(acq.restored).toMatchObject({ source: 'project', name: 'Bench table' });
		const t = await acq.control.request<ChannelTable>('channels.get');
		expect(t.channels).toEqual(rows);

		// and back to the project, with every column
		expect(await acq.applyRestored()).toBe(true);
		expect(await acq.reviewSendToProject()).toBe(true);
		expect(acq.homeReview?.replaces).toBe(true);
		expect(acq.homeReview?.changes).toEqual([]);
		expect(await acq.sendToProject()).toBe(true);
		expect(home.put[0]).toMatchObject({ name: 'Bench table', channels: rows, replace: true });
	});

	it('a table the service refuses is not taken, and the refusal is in the service words', async () => {
		const home = project({ Bad: [row(1, 'control', { sensitivity_mv_per_eu: 0 }), row(2, 'drive')] });
		const { acq } = await controller(home);
		expect(await acq.reviewProjectTable()).toBe(true);
		expect(acq.homeReview?.report?.errors.length).toBeGreaterThan(0);
		expect(await acq.takeProjectTable()).toBe(false);
		expect(acq.homeProblem).toContain('The control service refuses this table');
		expect(acq.restored).toBeNull();
	});

	it('a project with no table says so, and Send adds one', async () => {
		const home = project({});
		const { acq } = await controller(home);
		expect(await acq.reviewProjectTable()).toBe(false);
		expect(acq.homeProblem).toContain('the project holds no channel table');
		expect(await acq.reviewSendToProject()).toBe(true);
		expect(acq.homeReview?.replaces).toBe(false);
		expect(acq.homeReview?.changes.length).toBe(2);
		expect(await acq.sendToProject()).toBe(true);
		expect(home.put[0]).toMatchObject({ name: 'Channel Table' });
		expect(home.put[0].replace).toBeUndefined();
	});
});
