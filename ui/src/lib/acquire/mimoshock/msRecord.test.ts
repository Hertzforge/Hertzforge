/**
 * A MIMO Shock run's record from the shell's side, as the other test types': the run's folder
 * named in the state from arm on, the time histories off when the setup says so, the notice while
 * the record has a problem, the refusal when it cannot be written, the old capture refused and gone
 * from the setup, a draft saved with a capture read back without one, and the finished run's way to
 * its data opening in the Shock workspace. Through the mock control service and the real client.
 */
import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ChannelRow } from '$lib/protocol/types';
import { AcquireState } from '../store.svelte';
import { RunOpen, workspaceForRun, type RunOpenHost } from '../runOpen.svelte';
import { folderName, recordProblem } from '../words';
import { fromMsConfig, localErrors, mergeMsSetup, msSectionOf } from './msSetup';
import { MimoShockStore } from './msStore.svelte';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

function row(index: number, role: ChannelRow['role']): ChannelRow {
	return { index, enabled: true, description: `${role} ${index}`, role, physical: null, interface: 'single_ended', iepe_bias_v: null, units: role === 'drive' ? 'V' : 'g', sensitivity_mv_per_eu: role === 'drive' ? 1000 : 100, range_max: 10, polarity: 'positive', control_group: null, abort: { enabled: false } } as unknown as ChannelRow;
}

async function screen(options: ControlServerOptions = {}) {
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 40, dataRoot: '/data/Hertzforge_Data', ...options }), 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	acq.rows = [...[1, 2, 3, 4].map((i) => row(i, 'control')), ...[5, 6, 7, 8].map((i) => row(i, 'drive'))];
	expect(await acq.applyTable()).toBe(true);
	const ms = new MimoShockStore(acq, { local: null, checkDelayMs: 0 });
	await ms.sync();
	for (const x of ms.setup.drives) {
		x.ceiling_vpk = 4;
		x.abort_vpk = 6;
	}
	ms.setup.identification.review = false;
	return { client, acq, ms };
}

describe("a MIMO Shock run's record", () => {
	it('is named in the state from arm on, with its words and the time histories as the setup gives them', async () => {
		const { ms } = await screen();
		ms.setup.record = { operator: 'J. Operator', article: 'Bracket', notes: '', document: 'Test Method 516', clause: '4.2', streams: true };
		expect(await ms.apply()).toBe(true);
		expect(ms.served?.config?.operator).toBe('J. Operator');
		expect(ms.served?.config?.specification).toEqual({ document: 'Test Method 516', clause: '4.2' });
		expect(ms.served?.config?.record).toEqual({ streams: true });
		expect(ms.record).toBeNull();
		expect(await ms.arm()).toBe(true);
		await until(() => ms.record !== null);
		expect(ms.record?.streams).toBe('on');
		expect(folderName(ms.record!.folder)).toMatch(/^\d{8}-\d{6}_mimo_shock_ms-\d{4}$/);
		expect(ms.record!.folder.startsWith('/data/Hertzforge_Data/')).toBe(true);
		// the shared run head carries it, so the shared notices and the Run card read it
		expect(ms.run?.record).toEqual(ms.record);
		// a configuration read back keeps the record's words
		expect(fromMsConfig(ms.served!.config!).record.operator).toBe('J. Operator');
	});

	it('says the time histories are off when the setup leaves them out', async () => {
		const { ms } = await screen();
		ms.setup.record.streams = false;
		expect(await ms.apply()).toBe(true);
		expect(await ms.arm()).toBe(true);
		await until(() => ms.record !== null);
		expect(ms.record?.streams).toBe('off');
	});

	it('says while the run goes on that recording has a problem, and in the past once it has ended', async () => {
		const { ms } = await screen({ record: 'problem' });
		expect(await ms.apply()).toBe(true);
		expect(await ms.start()).toBe(true);
		await until(() => !!ms.run?.record?.problem, 15000);
		const going = recordProblem(ms.run?.record, false);
		expect(going?.title).toBe('Recording Has a Problem');
		expect(going?.text).toMatch(/^The run goes on\. Recording of the inputs stream stopped/);
		await until(() => ms.status?.state === 'finished', 20000);
		expect(recordProblem(ms.run?.record, true)?.title).toBe('Recording Had a Problem');
	});

	it('is refused at arm, in words, when its folder cannot be written, and nothing arms', async () => {
		const { ms } = await screen({ record: 'unwritable' });
		expect(await ms.apply()).toBe(true);
		expect(await ms.arm()).toBe(false);
		expect(ms.refusal?.reason).toBe('record_unwritable');
		expect(ms.refusal?.title).toBe("The Run's Record Cannot Be Written");
		expect(ms.refusal?.message).toMatch(/could not be written, so the run did not arm/);
		expect(ms.status?.state).toBe('configured');
		expect(ms.record).toBeNull();
	});

	it('has no capture: a configuration that asks for one is refused at the field, and the setup edits the Run Record', async () => {
		const { client, ms } = await screen();
		const config = { ...ms.wire, capture: { auto_start: true, run_name: 'old' } };
		const err = await client.request('mimo_shock.configure', { config }).catch((e) => e);
		expect(err).toBeInstanceOf(RequestError);
		expect((err as RequestError).reason).toBe('bad_params');
		expect((err as RequestError).detail).toEqual({ param: 'config.capture' });
		expect('capture' in ms.wire).toBe(false);
		expect(ms.wire.record).toEqual({ streams: true });
		expect(ms.recordChannels).toBe(ms.rows.filter((r) => r.enabled).length);
		expect(ms.recordRateHz).toBe(ms.setup.acquisition.fs_hz);
		// the record's findings belong with the channels, where the panel is
		expect(msSectionOf('specification.document')).toBe('channels');
		ms.setup.record.clause = '4.2';
		expect(localErrors(ms.setup)['specification.document']).toEqual(['Name the document this clause is from.']);
	});

	it('reads a draft saved with a capture back without one, its record from the defaults', () => {
		const saved = { capture: { auto_start: true, run_name: 'old' } } as unknown as Parameters<typeof mergeMsSetup>[0];
		const d = mergeMsSetup(saved);
		expect('capture' in d).toBe(false);
		expect(d.record.streams).toBe(true);
	});

	it('opens a finished run in the Shock workspace, through the same opener as the other shock runs', async () => {
		const shown: [string, string[]][] = [];
		const host: RunOpenHost = { bring: async () => ['inputs pulse', 'pulse_srs', 'srs_reference', 'Run Record'], show: (w, names) => shown.push([w, names]) };
		const opener = new RunOpen(host, 'mimo_shock');
		expect(await opener.open('/data/run')).toBe(true);
		expect(shown).toEqual([['shock', ['inputs pulse', 'pulse_srs', 'srs_reference', 'Run Record']]]);
		expect(workspaceForRun('mimo_shock')).toBe('shock');
	});
});
