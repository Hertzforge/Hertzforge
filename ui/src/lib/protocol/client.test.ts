import { describe, expect, it } from 'vitest';
import { ARM_REQUEST_TIMEOUT_MS, RequestError, ServiceClient, requestTimeoutFor, type SocketLike } from './client.svelte';
import { createAnalysisServer } from './mock/analysis';
import { createControlServer } from './mock/control';
import { MockError, MockSocket, mockSocketFactory, type MockServer } from './mock/socket';
import type { Catalog, DisplayEvent, SpectrumView } from './types';

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
		const [catalog, again] = await Promise.all([c.request<Catalog>('catalog.list'), c.request<Catalog>('catalog.list')]);
		expect(Array.isArray(catalog.operations)).toBe(true);
		expect(again).toEqual(catalog);
		c.close();
	});

	it('carries typed arrays through binary frames', async () => {
		const server: MockServer = {
			hello: { type: 'hello', protocol: 'sds/1', service: 'analysis', version: 't' },
			methods: { arrays: () => ({ x: new Float64Array([1, 2, 3]), n: new Int32Array([4, 5]) }) }
		};
		const c = new ServiceClient({ service: 'analysis', url: 'ws://t/', socketFactory: mockSocketFactory(server, 1) });
		c.connect();
		const res = await c.request<{ x: Float64Array; n: Int32Array }>('arrays');
		expect(res.x).toBeInstanceOf(Float64Array);
		expect(Array.from(res.x)).toEqual([1, 2, 3]);
		expect(res.n).toBeInstanceOf(Int32Array);
		c.close();
	});

	it('turns error responses into RequestError with the protocol code', async () => {
		const server: MockServer = {
			hello: { type: 'hello', protocol: 'sds/1', service: 'analysis', version: 't' },
			methods: {
				refuse: () => {
					throw new MockError('bad_request', 'The abort limit is inside the tolerance.');
				}
			}
		};
		const c = new ServiceClient({ service: 'analysis', url: 'ws://t/', socketFactory: mockSocketFactory(server, 1) });
		c.connect();
		await expect(c.request('no.such.method')).rejects.toMatchObject({ code: 'unknown_method' });
		const err = await c.request('refuse').catch((e) => e);
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

	// A modal solve of a large mesh runs for minutes; the service says it is still running with
	// `op.progress` every few seconds. RED-MUTATION: start the wait again on no event, or on any
	// event, and one of the three requests below ends the wrong way.
	it('waits on a request for as long as its alive topic keeps coming, and no longer', async () => {
		const server: MockServer = {
			hello: { type: 'hello', protocol: 'sds/1', service: 'analysis', version: 't' },
			methods: {
				'long.op': (params, conn) =>
					new Promise((resolve) => {
						const topic = params.topic as string;
						let beats = 0;
						const beat = setInterval(() => {
							conn.emit(topic, { op: 'long.op', done: beats, total: -1 });
							if (++beats === 8) {
								clearInterval(beat);
								resolve({ beats });
							}
						}, 25);
					})
			}
		};
		const c = new ServiceClient({ service: 'analysis', url: 'ws://t/', socketFactory: mockSocketFactory(server, 1) });
		c.connect();
		await c.whenOpen();
		// 8 beats 25 ms apart is 200 ms of work, four times the 50 ms allowed between words
		await expect(c.request('long.op', { topic: 'op.progress' }, { timeoutMs: 50, aliveOn: 'op.progress' })).resolves.toEqual({ beats: 8 });
		const otherTopic = await c.request('long.op', { topic: 'project.progress' }, { timeoutMs: 50, aliveOn: 'op.progress' }).catch((e) => e);
		expect(otherTopic).toMatchObject({ code: 'timeout' });
		expect(otherTopic.message).toBe('long.op heard nothing from the analysis service for 0 s.');
		await expect(c.request('long.op', { topic: 'op.progress' }, { timeoutMs: 50 })).rejects.toMatchObject({ code: 'timeout' });
		c.close();
	});

	// A request that arms the station waits for the station's arm, which on hardware settles
	// every IEPE input first. RED-MUTATION: give `.arm` and `.start` the ordinary timeout and a
	// 15 s settle refusal reads as "no response within 10 s".
	it('gives the requests that arm the station a minute, and every other request its own default', () => {
		for (const m of ['random.arm', 'random.start', 'explorer.start', 'mimo_random.start', 'shock.arm', 'mimo_mixed.start']) {
			expect(requestTimeoutFor(m, 10_000), m).toBe(ARM_REQUEST_TIMEOUT_MS);
		}
		for (const m of ['random.stop', 'random.level', 'channels.get', 'explorer.steer', 'random.start_over', 'started']) {
			expect(requestTimeoutFor(m, 10_000), m).toBe(10_000);
		}
		expect(ARM_REQUEST_TIMEOUT_MS).toBeGreaterThanOrEqual(30_000);
		expect(requestTimeoutFor('random.start', 90_000)).toBe(90_000);
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
		expect(Array.isArray((await c.request<Catalog>('catalog.list')).operations)).toBe(true);
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
