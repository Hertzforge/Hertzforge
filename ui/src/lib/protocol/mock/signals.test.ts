/**
 * The mock control service's Signals: refused as the service refuses, and during a mock run of a
 * test type other than Random, `signals.data` at the rate asked with the run's channels on the
 * station's clock, one last frame when the run ends, and nothing for a window that subscribes
 * after it.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '../client.svelte';
import type { SignalsData, SignalsSubscribeResult } from '../signals';
import type { SpectrumView } from '../types';
import { createControlServer } from './control';
import { parseSignalsSubscribe } from './signals';
import { MockError, mockSocketFactory, type MockServer } from './socket';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

async function connect(server: MockServer) {
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
	client.connect();
	await client.whenOpen();
	const frames: { at: number; d: SignalsData }[] = [];
	client.on('signals.data', (d) => frames.push({ at: Date.now(), d: d as unknown as SignalsData }));
	return { client, frames };
}

function param(params: Record<string, unknown>): string {
	try {
		parseSignalsSubscribe(params);
	} catch (e) {
		expect(e).toBeInstanceOf(MockError);
		return (e as MockError).detail?.param as string;
	}
	throw new Error('not refused');
}

describe('mock Signals', () => {
	it('refuses what the service refuses, naming the field', () => {
		expect(param({})).toBe('plots');
		expect(param({ plots: [], rate_hz: 20 })).toBe('rate_hz');
		expect(param({ plots: [], channels: [1] })).toBe('channels');
		expect(param({ plots: [{ id: '' }] })).toBe('plots[0].id');
		expect(param({ plots: [{ id: 'a' }, { id: 'a' }] })).toBe('plots[1].id');
		expect(param({ plots: [{ id: 'a', quantity: 'time' }] })).toBe('plots[0].quantity');
		expect(param({ plots: [{ id: 'a', channels: 'limited' }] })).toBe('plots[0].channels');
		expect(param({ plots: [{ id: 'a', window_s: 0 }] })).toBe('plots[0].window_s');
		expect(param({ plots: [{ id: 'a', points: 63 }] })).toBe('plots[0].points');
		expect(param({ plots: [{ id: 'a', arrange: 'stacked' }] })).toBe('plots[0].arrange');
		expect(parseSignalsSubscribe({ plots: [{ id: 'a', channels: [3, 2, 3] }] })).toEqual({
			rate: 10,
			plots: [{ id: 'a', channels: [3, 2], window_s: 2, points: 1024, arrange: 'overlay' }]
		});
	});

	it('streams the signals of a Recorder run, ends with one last frame, and sends a late window nothing', async () => {
		const server = createControlServer({ speed: 20 });
		const c = await connect(server);
		const o = await connect(server);
		const sub = (await o.client.request('signals.subscribe', {
			rate_hz: 5,
			plots: [
				{ id: 'raw', channels: [1, 2, 9], window_s: 0.005, points: 1024, arrange: 'split' },
				{ id: 'env', window_s: 1, points: 128 }
			]
		})) as SignalsSubscribeResult;
		expect(sub.available).toBe(false);
		expect(sub.channels).toEqual([]);
		expect(sub.plots[1]).toEqual({ id: 'env', channels: 'all', window_s: 1, points: 128, arrange: 'overlay' });

		// A Recorder run on the default table: row 1 control, row 2 drive.
		await c.client.request('session.claim', { name: 'Shell' });
		await c.client.request('recorder.configure', { config: {} });
		await c.client.request('recorder.start', {});
		await until(() => o.frames.length >= 3);
		const { d } = o.frames.at(-1)!;
		expect(d.available).toBe(true);
		expect(d.fs_hz).toBe(12800);
		expect(d.channels).toEqual([
			{ index: 1, name: 'Control', unit: 'g', kind: 'input' },
			{ index: 2, name: 'Drive', unit: 'V', kind: 'drive' }
		]);
		const raw = d.plots[0];
		expect(raw.channels).toEqual([1, 2]);
		const [input, drive] = raw.views as SpectrumView[];
		expect([input.title, input.y.unit, input.series[0].role]).toEqual(['Channel 1: Control', 'g', 'measured']);
		expect([drive.title, drive.y.unit, drive.series[0].role]).toEqual(['Channel 2: Drive', 'V', 'drive']);
		// Every sample of 5 ms at 12 800 Hz, one station frame apart, ending at the newest.
		const x = input.series[0].x;
		expect(x.length).toBe(64);
		for (let i = 1; i < x.length; i++) expect((x[i] - x[i - 1]) * 12800).toBeCloseTo(1, 9);
		expect(x[x.length - 1] * 12800).toBeCloseTo(d.frames - 1, 6);
		expect(input.series[0].y_min).toBeUndefined();
		// The long window: the envelope at its points, one view for each unit.
		const env = d.plots[1].views as SpectrumView[];
		expect(env.map((v) => v.y.unit)).toEqual(['g', 'V']);
		expect(env[0].series[0].x.length).toBeLessThanOrEqual(128);
		expect(env[0].series[0].y_min).toBeDefined();
		// Each frame has more of the run, at no more than the rate asked.
		for (let i = 1; i < o.frames.length; i++) {
			expect(o.frames[i].d.frames).toBeGreaterThan(o.frames[i - 1].d.frames);
			expect(o.frames[i].at - o.frames[i - 1].at).toBeGreaterThanOrEqual(1000 / 5 - 40);
		}

		// The run ends: one last frame, then nothing.
		await c.client.request('recorder.abort', {});
		await until(() => o.frames.some((f) => !f.d.available));
		const n = o.frames.length;
		expect(o.frames.at(-1)!.d.available).toBe(false);
		await new Promise((r) => setTimeout(r, 400));
		expect(o.frames.length).toBe(n);

		// A window that subscribes now is offered nothing and sent nothing.
		const late = await connect(server);
		const r = (await late.client.request('signals.subscribe', { plots: [{ id: 'a' }] })) as SignalsSubscribeResult;
		expect(r.available).toBe(false);
		await new Promise((r) => setTimeout(r, 300));
		expect(late.frames).toEqual([]);
		expect(await o.client.request('signals.unsubscribe', {})).toEqual({});
	}, 20000);
});
