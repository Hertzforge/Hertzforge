/**
 * The parts tables as table files: a tones table and a narrowbands table written as the service
 * takes them and read back with every column (each kind of schedule, a tone's profile with a row's
 * own bands, a band's own bands and width by share), through the mock analysis service's
 * `tables.save` and `tables.load` as the File menu sends them.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createAnalysisServer } from '$lib/protocol/mock/analysis';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { loadTable, saveTable } from '$lib/tables/tableFiles';
import { newBand, newTone, deepCopy } from './mixedSetup';
import { partsRows, partsTable, PART_TABLE_KIND, profileRowsOf, profileText } from './partsFile';

function tones() {
	const t = [newTone('t1', 1, 120), newTone('t2', 2, 300), newTone('t3', 3)];
	t[0].schedule.dwell = { by: 'cycles', seconds: 60, cycles: 600 };
	t[0].window = { on_s: 5, off_s: 100 };
	t[1].schedule = { ...t[1].schedule, kind: 'sweep', band_hz: [300, 600], law: 'log', oct_per_min: 1, start_hz: 350, direction: 'down', end_by: 'both', sweeps: 4, duration_s: 120 };
	t[1].amplitude.by = 'profile';
	t[1].amplitude.profile.rows[1] = { ...t[1].amplitude.profile.rows[1], own_bands: true, alarm_db: [1, 2], abort_db: [3, 4] };
	t[2].schedule = { ...t[2].schedule, kind: 'follow', of: 't2', ratio: 2 };
	t[2].enabled = false;
	t[2].alarm_db = [1, 2];
	return t;
}

function bands() {
	const b = [newBand('b1', 1, 250), newBand('b2', 2, 1300)];
	b[1].schedule = { ...b[1].schedule, kind: 'sweep', band_hz: [1300, 1800], law: 'linear', hz_per_s: 2, start_hz: null, end_by: 'none' };
	b[1].width = { by: 'share', hz: 20, share: 0.05 };
	b[1].own_bands = true;
	b[1].abort_db = [5, 7];
	return b;
}

/** The fields a file carries: the kind's own schedule fields, not the others the draft keeps. */
function carried(r: ReturnType<typeof tones>[number] | ReturnType<typeof bands>[number]) {
	const s = r.schedule;
	const sched = s.kind === 'fixed' ? { kind: s.kind, f: s.freq_hz, dwell: s.dwell.by, d: s.dwell.by === 'cycles' ? s.dwell.cycles : s.dwell.by === 'seconds' ? s.dwell.seconds : null } : s.kind === 'sweep' ? { kind: s.kind, band: s.band_hz, law: s.law, rate: s.law === 'log' ? s.oct_per_min : s.hz_per_s, start: s.start_hz, dir: s.direction, end: s.end_by } : { kind: s.kind, of: s.of, ratio: s.ratio };
	const rest = 'amplitude' in r ? { amp: r.amplitude.by === 'constant' ? r.amplitude.g : r.amplitude.profile.rows, bands: [r.alarm_db, r.abort_db] } : { level: r.level_g2_hz, width: r.width.by === 'hz' ? ['hz', r.width.hz] : ['share', r.width.share], own: r.own_bands, bands: r.own_bands ? [r.alarm_db, r.abort_db] : null };
	return { id: r.id, name: r.name, enabled: r.enabled, window: r.window, sched, ...rest };
}

describe('parts tables as files', () => {
	it('a profile reads back from its cell with a row of its own bands', () => {
		const rows = tones()[1].amplitude.profile.rows;
		expect(profileRowsOf(profileText(rows))).toEqual(deepCopy(rows));
	});

	for (const [table, make] of [['tones', tones], ['bands', bands]] as const) {
		it(`the ${table} table saves and opens through the analysis service with every column`, async () => {
			const client = new ServiceClient({ service: 'analysis', url: 'ws://mock/', socketFactory: mockSocketFactory(createAnalysisServer(), 1) });
			client.connect();
			await client.whenOpen();
			const rows = make();
			const path = await saveTable(client, 'breakpoints', PART_TABLE_KIND[table], partsTable(table, rows), `/tmp/${table}.csv`);
			const loaded = await loadTable(client, 'breakpoints', PART_TABLE_KIND[table], path);
			const read = partsRows(table, loaded);
			expect(read.notes).toEqual([]);
			expect(read.rows.map(carried)).toEqual(rows.map(carried));
		});
	}
});
