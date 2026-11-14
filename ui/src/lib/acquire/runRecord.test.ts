/**
 * A Random run's record from the shell's side: the operator's fields in the setup draft and the
 * configuration, and the size an hour of time histories takes.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { RandomConfig } from '$lib/protocol/types';
import { configKey, defaultSetup, fromConfig, localErrors, recordBytesPerHour, sizeWords, toConfig, type SetupDraft } from './setup';
import { AcquireState } from './store.svelte';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

function memoryStorage(): Storage {
	const m = new Map<string, string>();
	return {
		get length() {
			return m.size;
		},
		clear: () => m.clear(),
		getItem: (k) => m.get(k) ?? null,
		key: (i) => [...m.keys()][i] ?? null,
		removeItem: (k) => void m.delete(k),
		setItem: (k, v) => void m.set(k, v)
	};
}

function quick(d: SetupDraft = defaultSetup()): SetupDraft {
	d.acquisition = { fs_hz: 8192, n_pts: 1024, n_avg: 2 };
	d.schedule = { mode: 'auto', levels: [{ db: -6, dwell_s: 1 }, { db: 0, dwell_s: 0 }], full_level_s: 2 };
	d.control.review_after_sysid = false;
	return d;
}

async function controller(options: ControlServerOptions = {}, local: Storage | null = null) {
	const server = createControlServer({ speed: 10, ...options });
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
	const acq = new AcquireState(client, { name: 'Shell on bench-pc', session: null, local, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	expect(await acq.claim()).toBe(true);
	acq.setup = quick();
	return { server, client, acq };
}

describe('the run record fields', () => {
	it('start empty with time histories on, and send nothing but empty words and the switch', () => {
		const d = defaultSetup();
		expect(d.record).toEqual({ operator: '', article: '', notes: '', document: '', clause: '', streams: true });
		const c = toConfig(d);
		expect([c.operator, c.article, c.notes]).toEqual(['', '', '']);
		expect(c.specification).toBeNull();
		expect(c.record).toEqual({ streams: true });
		expect(localErrors(d)).toEqual({});
	});

	it('send the operator\'s words trimmed, the specification when either part is given, and the switch', () => {
		const d = defaultSetup();
		d.record = { operator: '  J. Operator ', article: 'Bracket assembly, unit 3', notes: 'Second axis.\nFixture torqued. ', document: ' Test Method 514 ', clause: '', streams: false };
		const c = toConfig(d);
		expect(c.operator).toBe('J. Operator');
		expect(c.article).toBe('Bracket assembly, unit 3');
		expect(c.notes).toBe('Second axis.\nFixture torqued.');
		expect(c.specification).toEqual({ document: 'Test Method 514', clause: '' });
		expect(c.record).toEqual({ streams: false });
	});

	it('come back from the configuration the service holds, and make the same key both ways', () => {
		const d = defaultSetup();
		d.record = { operator: 'A', article: 'B', notes: 'C', document: 'D', clause: 'E', streams: false };
		const sent = toConfig(d);
		const back = fromConfig(JSON.parse(JSON.stringify(sent)) as RandomConfig);
		expect(back.record).toEqual(d.record);
		expect(configKey(toConfig(back))).toBe(configKey(sent));
		// a configuration written before these fields existed reads as empty, recording on
		const { operator: _o, article: _a, notes: _n, specification: _s, record: _r, ...older } = sent;
		expect(fromConfig(older as RandomConfig).record).toEqual(defaultSetup().record);
	});

	it('name a clause without its document; every other field may be left empty', () => {
		const d = defaultSetup();
		d.record.clause = '3.2';
		expect(localErrors(d)).toEqual({ 'specification.document': ['Name the document this clause is from.'] });
		d.record.document = 'Test Method 514';
		expect(localErrors(d)).toEqual({});
		d.record = { operator: '', article: '', notes: '', document: 'Test Method 514', clause: '', streams: true };
		expect(localErrors(d)).toEqual({});
	});

	it('are kept with the setup draft across a reload', async () => {
		const local = memoryStorage();
		const s = await controller({}, local);
		const stop = s.acq.watch();
		s.acq.setup.record.operator = 'J. Operator';
		s.acq.setup.record.streams = false;
		await until(() => (local.getItem('hertzforge.acquire.setup.v1') ?? '').includes('J. Operator'));
		stop();
		s.client.close();
		const again = new AcquireState(new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer(), 1) }), { session: null, local });
		expect(again.setup.record.operator).toBe('J. Operator');
		expect(again.setup.record.streams).toBe(false);
		expect(again.setup.record.clause).toBe('');
	});
});

describe('the size of an hour of time histories', () => {
	it('is the enabled inputs plus drives, times the sample rate, times four bytes', () => {
		expect(recordBytesPerHour(3, 8192)).toBe(3 * 8192 * 4 * 3600);
		expect(sizeWords(recordBytesPerHour(3, 8192)!)).toBe('354 MB');
		expect(sizeWords(recordBytesPerHour(9, 51200)!)).toBe('6.64 GB');
		expect(sizeWords(recordBytesPerHour(1, 100)!)).toBe('1.44 MB');
		expect(sizeWords(512)).toBe('512 bytes');
		expect(recordBytesPerHour(3, NaN)).toBeNull();
		expect(recordBytesPerHour(3, 0)).toBeNull();
	});

	it('counts the channel table being edited, drives included and channels switched off left out', async () => {
		const s = await controller();
		const enabled = s.acq.rows.filter((r) => r.enabled);
		expect(enabled.some((r) => r.role === 'drive')).toBe(true);
		expect(s.acq.recordChannels).toBe(enabled.length);
		s.acq.rows[0].enabled = false;
		expect(s.acq.recordChannels).toBe(enabled.length - 1);
		s.client.close();
	});
});
