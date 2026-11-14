/**
 * A Random run's record from the shell's side: the operator's fields in the setup draft and the
 * configuration, the size an hour of time histories takes, the recording line's words, the notice
 * while the record has a problem, and the refusal when it cannot be written. The run parts go
 * through the mock control service and the real client.
 */
import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, runFolderName, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { RandomConfig } from '$lib/protocol/types';
import { configKey, defaultSetup, fromConfig, localErrors, recordBytesPerHour, sizeWords, toConfig, type SetupDraft } from './setup';
import { AcquireState } from './store.svelte';
import { folderName, LOG_WORDS, recordProblem, refusalTitle, toRefusal } from './words';

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

describe('words for the record', () => {
	it("names the folder by its last part, on either machine's separator", () => {
		expect(folderName('/home/op/Hertzforge_Data/20261113-180000_random_r-0001')).toBe('20261113-180000_random_r-0001');
		expect(folderName('C:\\Users\\op\\Hertzforge_Data\\20261113-180000_random_r-0001\\')).toBe('20261113-180000_random_r-0001');
		expect(runFolderName(new Date(Date.UTC(2026, 10, 13, 18, 4, 5)), 'r-0007')).toBe('20261113-180405_random_r-0007');
		expect(LOG_WORDS.record).toBe('Recording');
	});

	it('says recording has a problem and the run goes on, only while there is one', () => {
		expect(recordProblem(null)).toBeNull();
		expect(recordProblem({ problem: null })).toBeNull();
		expect(recordProblem({ problem: '  ' })).toBeNull();
		expect(recordProblem({ problem: 'the inputs stream stopped: no space left on the disk' })).toEqual({
			title: 'Recording Has a Problem',
			text: 'The run goes on. The inputs stream stopped: no space left on the disk.'
		});
		expect(recordProblem({ problem: 'The log could not be written.' })?.text).toBe('The run goes on. The log could not be written.');
		expect(recordProblem({ problem: 'the inputs stream stopped' }, true)).toEqual({ title: 'Recording Had a Problem', text: 'The inputs stream stopped.' });
	});

	it('shows a record that cannot be written with its path and reason', () => {
		expect(refusalTitle('record_unwritable')).toBe("The Run's Record Cannot Be Written");
		const r = toRefusal(
			'Arm',
			new RequestError('refused', "The run's record could not be written, so the run did not arm: /data/x (Read-only file system). Choose a data folder that can be written.", 'random.arm', 'record_unwritable', {
				path: '/data/x',
				reason: 'Read-only file system'
			})
		);
		expect(r.title).toBe("The Run's Record Cannot Be Written");
		expect(r.message).toMatch(/^The run's record could not be written/);
		expect(r.facts).toEqual(['Path: /data/x', 'Reason: Read-only file system']);
	});
});

describe('the run record on the mock service', () => {
	it('sends the fields with the setup and names the folder from arm on, kept after the run ends', async () => {
		const s = await controller({ dataRoot: '/srv/hf' });
		s.acq.setup.record = { operator: 'J. Operator', article: 'Unit 3', notes: '', document: 'Test Method 514', clause: '3.2', streams: true };
		expect(await s.acq.apply()).toBe(true);
		await until(() => s.acq.setupStatus === 'applied');
		expect(s.acq.held?.config.operator).toBe('J. Operator');
		expect(s.acq.held?.config.specification).toEqual({ document: 'Test Method 514', clause: '3.2' });
		expect(s.acq.run?.record).toBeUndefined();

		expect(await s.acq.arm()).toBe(true);
		const rec = s.acq.run?.record;
		expect(rec?.streams).toBe('on');
		expect(rec?.problem).toBeNull();
		expect(rec?.folder).toMatch(/^\/srv\/hf\/\d{8}-\d{6}_random_r-\d{4}$/);
		expect(await s.acq.start()).toBe(true);
		expect(await s.acq.stop()).toBe(true);
		await until(() => s.acq.run?.state === 'finished');
		expect(s.acq.run?.end?.outcome).toBe('operator_stop');
		expect(s.acq.run?.record?.folder).toBe(rec?.folder);

		// the next run configured has no record until it arms
		expect(await s.acq.reset()).toBe(true);
		expect(s.acq.run?.record).toBeUndefined();
		s.client.close();
	});

	it('says the time histories are off when switched off', async () => {
		const s = await controller();
		s.acq.setup.record.streams = false;
		expect(await s.acq.apply()).toBe(true);
		expect(await s.acq.arm()).toBe(true);
		expect(s.acq.run?.record?.streams).toBe('off');
		s.client.close();
	});

	it('refuses the arm when the record cannot be written, with the path and the reason', async () => {
		const s = await controller({ record: 'unwritable' });
		expect(await s.acq.apply()).toBe(true);
		expect(await s.acq.arm()).toBe(false);
		expect(s.acq.refusal?.reason).toBe('record_unwritable');
		expect(s.acq.refusal?.title).toBe("The Run's Record Cannot Be Written");
		expect(s.acq.refusal?.facts[0]).toMatch(/^Path: ~\/Hertzforge_Data\/\d{8}-\d{6}_random_r-\d{4}$/);
		expect(s.acq.refusal?.facts[1]).toBe('Reason: Permission denied');
		expect(s.acq.run?.state).toBe('configured');
		s.client.close();
	});

	it('reports a problem writing the record while the run goes on', async () => {
		const s = await controller({ record: 'problem' });
		expect(await s.acq.apply()).toBe(true);
		expect(await s.acq.start()).toBe(true);
		await until(() => !!s.acq.run?.record?.problem);
		expect(recordProblem(s.acq.run?.record)?.text).toBe('The run goes on. The inputs stream stopped: no space left on the disk.');
		expect(['running', 'stopping']).toContain(s.acq.run?.state);
		await until(() => s.acq.log.some((e) => e.kind === 'record'));
		await until(() => s.acq.run?.state === 'finished', 15000);
		expect(s.acq.run?.end?.outcome).toBe('completed');
		s.client.close();
	});
});
