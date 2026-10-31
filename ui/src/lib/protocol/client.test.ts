import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient, type SocketLike } from './client.svelte';
import { createAnalysisServer } from './mock/analysis';
import { createControlServer } from './mock/control';
import { MockError, MockSocket, mockSocketFactory, type MockServer } from './mock/socket';
import type { DisplayEvent, OpResult, OpenedDataset, SpectrumView } from './types';

const until = async (cond: () => boolean, ms = 3000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

function analysisClient() {
	return new ServiceClient({ service: 'analysis', url: 'ws://mock/', socketFactory: mockSocketFactory(createAnalysisServer(), 1) });
}

describe('ServiceClient over the mock transport', () => {
	it('connects, checks hello and answers requests by id', async () => {
		const c = analysisClient();
		c.connect();
		await c.whenOpen();
		expect(c.status).toBe('open');
		expect(c.hello?.protocol).toBe('sds/1');
		const [catalog, opened] = await Promise.all([
			c.request<{ operations: unknown[]; datasets: { id: string }[] }>('catalog.list'),
			c.request<OpenedDataset>('dataset.open', { id: 'syn-run-qual-z' })
		]);
		expect(catalog.operations.length).toBeGreaterThan(0);
		expect(opened.channels.length).toBe(4);
		c.close();
	});

	it('carries typed arrays through binary frames', async () => {
		const c = analysisClient();
		c.connect();
		const opened = await c.request<OpenedDataset>('dataset.open', { id: 'syn-run-qual-z' });
		const res = await c.request<OpResult>('op.run', {
			op: 'random.psd_vs_spec',
			handle: opened.handle,
			params: { channel: 'ctrl1', tolerance_db: 3, abort_db: 6, label: 'Control' }
		});
		const spectrum = res.views[0] as SpectrumView;
		expect(spectrum.view).toBe('spectrum');
		expect(spectrum.series[0].x).toBeInstanceOf(Float64Array);
		expect(spectrum.series[0].x.length).toBe(397);
		expect(res.views[1].view).toBe('reading');
		c.close();
	});

	it('turns error responses into RequestError with the protocol code', async () => {
		const c = analysisClient();
		c.connect();
		await expect(c.request('dataset.open', { id: 'plate-x' })).rejects.toMatchObject({ code: 'not_found' });
		await expect(c.request('no.such.method')).rejects.toMatchObject({ code: 'unknown_method' });
		const opened = await c.request<OpenedDataset>('dataset.open', { id: 'syn-run-qual-z' });
		const err = await c
			.request('op.run', { op: 'random.psd_vs_spec', handle: opened.handle, params: { channel: 'ctrl1', tolerance_db: 4, abort_db: 3 } })
			.catch((e) => e);
		expect(err).toBeInstanceOf(RequestError);
		expect(err.code).toBe('bad_request');
		expect(err.message).toMatch(/abort limit/);
		c.close();
	});

	it('times out a request that gets no response', async () => {
		const server: MockServer = {
			hello: { type: 'hello', protocol: 'sds/1', service: 'analysis', version: 't' },
			methods: { 'slow.op': () => new Promise(() => {}) }
		};
		const c = new ServiceClient({ service: 'analysis', url: 'ws://t/', socketFactory: mockSocketFactory(server, 1) });
		c.connect();
		await c.whenOpen();
		await expect(c.request('slow.op', {}, { timeoutMs: 40 })).rejects.toMatchObject({ code: 'timeout' });
		c.close();
	});

	it('refuses a service that speaks another protocol', async () => {
		const server: MockServer = { hello: { type: 'hello', protocol: 'sds/2', service: 'analysis', version: 'x' }, methods: {} };
		const c = new ServiceClient({ service: 'analysis', url: 'ws://t/', socketFactory: mockSocketFactory(server, 1) });
		c.connect();
		await until(() => c.status === 'incompatible');
		expect(c.detail).toMatch(/sds\/2/);
		expect(c.hello).toBeNull();
		c.close();
	});

	it('reconnects with backoff after the connection drops', async () => {
		const sockets: SocketLike[] = [];
		const server = createAnalysisServer();
		const c = new ServiceClient({
			service: 'analysis',
			url: 'ws://t/',
			backoffInitialMs: 20,
			socketFactory: (url) => {
				const s = new MockSocket(url, server, 1);
				sockets.push(s);
				return s;
			}
		});
		c.connect();
		await c.whenOpen();
		sockets[0].onclose?.({ code: 1006 }); // the network drops
		expect(c.status).toBe('retrying');
		await until(() => c.status === 'open');
		expect(sockets.length).toBe(2);
		expect((await c.request<{ datasets: unknown[] }>('catalog.list')).datasets.length).toBeGreaterThan(0);
		c.close();
		expect(c.status).toBe('offline');
	});

	it('fails pending requests when the connection drops', async () => {
		const sockets: SocketLike[] = [];
		const server: MockServer = {
			hello: { type: 'hello', protocol: 'sds/1', service: 'analysis', version: 't' },
			methods: { wait: () => new Promise(() => {}), bad: () => Promise.reject(new MockError('refused', 'No.')) }
		};
		const c = new ServiceClient({
			service: 'analysis',
			url: 'ws://t/',
			socketFactory: (url) => {
				const s = new MockSocket(url, server, 1);
				sockets.push(s);
				return s;
			}
		});
		c.connect();
		await c.whenOpen();
		await expect(c.request('bad')).rejects.toMatchObject({ code: 'refused' });
		const pending = c.request('wait');
		sockets[0].onclose?.({ code: 1006 });
		await expect(pending).rejects.toMatchObject({ code: 'disconnected' });
		c.close();
	});
});

describe('mock control service events', () => {
	it('streams random.display marked SIM while running, then stops on request', async () => {
		const c = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer(), 1) });
		const events: DisplayEvent[] = [];
		c.on('random.display', (d) => events.push(d as unknown as DisplayEvent));
		c.connect();
		const { profiles } = await c.request<{ profiles: { id: string }[] }>('sim.profiles');
		expect(profiles.map((p) => p.id)).toContain('qual');
		const { run_id } = await c.request<{ run_id: string }>('sim.start', { profile: 'qual', level_db: 0, speed: 0 });
		await expect(c.request('sim.start', { profile: 'qual', level_db: 0, speed: 1 })).rejects.toMatchObject({ code: 'refused' });
		await until(() => events.length >= 2, 2000);
		expect(events.every((e) => e.source === 'SIM' && e.run_id === run_id && e.state === 'running')).toBe(true);
		const spectrum = events[0].views.find((v) => v.view === 'spectrum') as SpectrumView;
		expect(spectrum.series.map((s) => s.role)).toEqual(['measured', 'reference', 'tolerance', 'tolerance', 'abort', 'abort']);
		expect(await c.request('sim.stop')).toEqual({ state: 'stopped' });
		await until(() => events.at(-1)?.state === 'stopped');
		expect(await c.request('sim.state')).toEqual({ state: 'stopped', run_id });
		c.close();
	});
});
