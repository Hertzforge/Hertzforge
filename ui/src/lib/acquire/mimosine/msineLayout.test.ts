/**
 * MIMO Sine's run screen: its plot types and first pages, what a window asks the service for, and
 * every quantity offered answered with views by the mock control service once the tone engages.
 * Also the chooser: MIMO Sine is offered like the other modules.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ChannelRow, View } from '$lib/protocol/types';
import { pulseSubscription } from '../pulse/pulseScreen';
import type { PlotPanel } from '../screen/screen.svelte';
import { AcquireState } from '../store.svelte';
import { TEST_TYPES, TestTypeView } from '../testTypes.svelte';
import { readConfig } from '$lib/app/config';
import { MSINE_PLOT_TYPES, MSINE_QUANTITIES } from './msineLayout';
import { MimoSineStore } from './msineStore.svelte';
import { MSINE_HOOKS } from './msineViews.svelte';

describe('The mock simulator for a larger table', () => {
	it('takes its inputs and outputs from mock_sim, and offers them as the station', async () => {
		expect(readConfig('?mock=1&mock_sim=17x8').mockControl).toMatchObject({ simInputs: 17, simOutputs: 8 });
		for (const bad of ['0x8', '17', 'x8', '99x8', '17x8x1']) expect(readConfig(`?mock=1&mock_sim=${bad}`).mockControl.simInputs).toBeUndefined();
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ simInputs: 17, simOutputs: 8 }), 1) });
		client.connect();
		await client.whenOpen();
		const list = await client.request<{ stations: { kind: string; inputs: number; outputs: number }[] }>('station.list');
		expect(list.stations.find((x) => x.kind === 'sim')).toMatchObject({ inputs: 17, outputs: 8 });
		client.close();
	});
});

const plot = (id: string, quantity: string, channels: number[] | 'all' = 'all', points = 1024) => ({ id, kind: 'plot', plot: { quantity, channels, points } }) as unknown as PlotPanel;

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

describe('the MIMO Sine run screen', () => {
	it('offers every quantity the service draws, with a channel choice where the service takes one', () => {
		expect(MSINE_PLOT_TYPES.map((t) => t.id)).toEqual([...MSINE_QUANTITIES]);
		expect(MSINE_PLOT_TYPES.filter((t) => t.channels).map((t) => t.id)).toEqual(['control', 'phase', 'limit', 'time', 'identification']);
		expect(MSINE_PLOT_TYPES.filter((t) => t.time).map((t) => t.id)).toEqual(['time']);
	});

	it('opens on its first pages, each a page of its quantities', () => {
		const layout = MSINE_HOOKS.defaultLayout(1.6);
		expect(layout.pages.map((p) => p.name)).toEqual(['Controls', 'Error', 'Limits', 'Drives', 'Identification', 'Law']);
		expect(layout.pages[0].panels.map((p) => (p.kind === 'plot' ? p.plot.quantity : p.kind))).toEqual(['control', 'phase']);
	});

	it('asks for a channel list or every channel, and a point count on the time history only', () => {
		const params = pulseSubscription(MSINE_PLOT_TYPES, [plot('a', 'control', [1, 3]), plot('b', 'drive'), plot('c', 'time', 'all', 2048)], 5) as { plots: Record<string, unknown>[] };
		expect(params.plots).toEqual([
			{ id: 'a', quantity: 'control', channels: [1, 3] },
			{ id: 'b', quantity: 'drive' },
			{ id: 'c', quantity: 'time', channels: 'all', points: 2048 }
		]);
	});

	it('is answered with views for every quantity once the tone engages', async () => {
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 8 }), 1) });
		const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
		client.connect();
		await client.whenOpen();
		await acq.sync();
		await acq.claim();
		const r = (index: number, role: ChannelRow['role']) => ({ index, enabled: true, description: '', role, physical: null, interface: 'single_ended', iepe_bias_v: null, units: role === 'drive' ? 'V' : 'g', sensitivity_mv_per_eu: 100, range_max: 10, polarity: 'positive', control_group: null, abort: { enabled: false } }) as unknown as ChannelRow;
		acq.rows = [r(1, 'control'), r(2, 'control'), r(3, 'control'), r(4, 'control'), r(5, 'drive'), r(6, 'drive'), r(7, 'drive'), r(8, 'drive'), r(9, 'monitor')];
		expect(await acq.applyTable()).toBe(true);
		const ms = new MimoSineStore(acq, { local: null, checkDelayMs: 0 });
		await ms.sync();
		ms.setup.drives.forEach((x) => ((x.ceiling_vpk = 4), (x.abort_vpk = 6)));
		ms.addLimit(9);
		ms.setup.limits.channels[0].profile.rows.forEach((x) => (x.value = 1));
		await ms.checkSetup();
		expect(await ms.apply()).toBe(true);
		const got = new Map<string, View[]>();
		client.on('mimo_sine.data', (d) => {
			for (const p of (d as { plots: { id: string; views: View[] }[] }).plots) if (p.views.length) got.set(p.id, p.views);
		});
		await client.request('mimo_sine.subscribe', pulseSubscription(MSINE_PLOT_TYPES, MSINE_QUANTITIES.map((q) => plot(q, q)), 5));
		expect(await ms.start()).toBe(true);
		await until(() => ms.status?.state === 'review');
		await ms.proceed();
		await until(() => MSINE_QUANTITIES.every((q) => got.has(q)));
		expect(got.get('control')!.map((v) => v.title)).toEqual(['Control 1', 'Control 2', 'Control 3', 'Control 4']);
		expect(got.get('identification')!.map((v) => v.title)).toEqual(['Identification Gain 1', 'Identification Phase 1', 'Coherence']);
		expect(got.get('limit')!.map((v) => v.title)).toEqual(['Limit 9']);
		await ms.abort();
	});
});

describe('MIMO Sine in the chooser', () => {
	it('is offered like the other modules, with no switch to reach it', () => {
		expect(TEST_TYPES.find((t) => t.id === 'mimo_sine')?.ready).toBe(true);
		const view = new TestTypeView(() => null);
		view.open();
		view.settle(null);
		expect(view.whyNot('mimo_sine')).toBeNull();
		expect(view.choose('mimo_sine')).toBe(true);
		expect(view.screen).toBe('mimo_sine');
		expect(new TestTypeView(() => null).whyNot('mimo_twr')).toBe('Coming');
		expect('preview' in readConfig('?preview=mimo_sine')).toBe(false);
	});
});
