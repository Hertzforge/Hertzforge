/**
 * A classical shock run's record from the shell's side, as Random's: the run's folder named in
 * the state from arm on, the time histories off when the setup says so, the notice while the
 * record has a problem, the refusal when it cannot be written, the old capture refused, and the
 * finished run's way to its data opening in the Shock workspace. Through the mock control service
 * and the real client.
 */
import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { AcquireState } from '../store.svelte';
import { RunOpen, workspaceForRun, type RunOpenHost } from '../runOpen.svelte';
import { folderName, recordProblem } from '../words';
import { ShockStore } from './shockStore.svelte';

const until = async (cond: () => boolean, ms = 6000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

async function screen(options: ControlServerOptions = {}) {
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 20, dataRoot: '/data/Hertzforge_Data', ...options }), 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	const shock = new ShockStore(acq, { local: null, checkDelayMs: 0 });
	await shock.sync();
	shock.setup.limits.drive_vpk_ceiling = 0.5;
	shock.setup.limits.drive_vpk_abort = 0.8;
	shock.setup.identification.review = false;
	return { client, acq, shock };
}

describe("a classical shock run's record", () => {
	it('is named in the state from arm on, with its words and the time histories as the setup gives them', async () => {
		const { shock } = await screen();
		shock.setup.record = { operator: 'J. Operator', article: 'Bracket', notes: '', document: 'Test Method 516', clause: '4.2', streams: true };
		expect(await shock.apply()).toBe(true);
		expect(shock.served?.config?.operator).toBe('J. Operator');
		expect(shock.served?.config?.specification).toEqual({ document: 'Test Method 516', clause: '4.2' });
		expect(shock.record).toBeNull();
		expect(await shock.arm()).toBe(true);
		await until(() => shock.record !== null);
		expect(shock.record?.streams).toBe('on');
		expect(folderName(shock.record!.folder)).toMatch(/^\d{8}-\d{6}_classical_shock_s-\d{4}$/);
		expect(shock.record!.folder.startsWith('/data/Hertzforge_Data/')).toBe(true);
		// the shared run head carries it, so the shared notices and the Run card read it
		expect(shock.run?.record).toEqual(shock.record);
	});

	it('says the time histories are off when the setup leaves them out', async () => {
		const { shock } = await screen();
		shock.setup.record.streams = false;
		expect(await shock.apply()).toBe(true);
		expect(await shock.arm()).toBe(true);
		await until(() => shock.record !== null);
		expect(shock.record?.streams).toBe('off');
	});

	it('says while the run goes on that recording has a problem, and in the past once it has ended', async () => {
		const { shock } = await screen({ record: 'problem' });
		expect(await shock.apply()).toBe(true);
		expect(await shock.start()).toBe(true);
		await until(() => !!shock.run?.record?.problem, 8000);
		const going = recordProblem(shock.run?.record, false);
		expect(going?.title).toBe('Recording Has a Problem');
		expect(going?.text).toMatch(/^The run goes on\. Recording of the inputs stream stopped/);
		await until(() => shock.status?.state === 'finished', 8000);
		expect(recordProblem(shock.run?.record, true)?.title).toBe('Recording Had a Problem');
	});

	it('is refused at arm, in words, when its folder cannot be written, and nothing arms', async () => {
		const { shock } = await screen({ record: 'unwritable' });
		expect(await shock.apply()).toBe(true);
		expect(await shock.arm()).toBe(false);
		expect(shock.refusal?.reason).toBe('record_unwritable');
		expect(shock.refusal?.title).toBe("The Run's Record Cannot Be Written");
		expect(shock.refusal?.message).toMatch(/could not be written, so the run did not arm/);
		expect(shock.status?.state).toBe('configured');
		expect(shock.record).toBeNull();
	});

	it('has no capture: a configuration that asks for one is refused at the field', async () => {
		const { client, shock } = await screen();
		const config = { ...shock.wire, capture: { auto_start: true, run_name: 'old' } };
		const err = await client.request('shock.configure', { config }).catch((e) => e);
		expect(err).toBeInstanceOf(RequestError);
		expect((err as RequestError).reason).toBe('bad_params');
		expect((err as RequestError).detail).toEqual({ param: 'config.capture' });
		expect('capture' in shock.wire).toBe(false);
	});

	it('opens a finished run in the Shock workspace, through the same opener as Random', async () => {
		const shown: [string, string[]][] = [];
		const host: RunOpenHost = { bring: async () => ['inputs pulse', 'pulse_srs', 'Run Record'], show: (w, names) => shown.push([w, names]) };
		const opener = new RunOpen(host, 'classical_shock');
		expect(await opener.open('/data/run')).toBe(true);
		expect(shown).toEqual([['shock', ['inputs pulse', 'pulse_srs', 'Run Record']]]);
		expect(workspaceForRun('classical_shock')).toBe('shock');
		expect(workspaceForRun('random')).toBe('random');
		expect(workspaceForRun('recorder')).toBe('signals');
	});
});
