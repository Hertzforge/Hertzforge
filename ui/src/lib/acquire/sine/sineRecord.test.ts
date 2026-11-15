/**
 * Sine's run record in the shell, over the real client and codec against the mock control
 * service: the record's fields reach the service and come back from the configuration it holds;
 * from arm the state names the folder, the Run card's line is drawn from it, and a problem is
 * a notice while the run goes on; a record that cannot be written refuses the arm in words; a
 * finished run's Open in Analysis lands in the Sine workspace. The shared pieces are Random's
 * (RunRecordEditor, RunRecordLine, RunOpenActions, runOpen); these tests hold Sine to them.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, runFolderName, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { RunOpen, workspaceForRun } from '../runOpen.svelte';
import { recordBytesPerHour } from '../setup';
import { AcquireState } from '../store.svelte';
import { recordProblem } from '../words';
import { fromSineConfig, SINE_SECTIONS, toSineConfig } from './sineSetup';
import { SineStore } from './sineStore.svelte';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

async function screen(options: ControlServerOptions = {}) {
	const server = createControlServer({ speed: 20, ...options });
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	const sine = new SineStore(acq, { local: null, checkDelayMs: 0 });
	await sine.sync();
	return { client, acq, sine };
}

function short(sine: SineStore) {
	sine.setup.limits.drive_vpk_ceiling = 0.5;
	sine.setup.limits.drive_vpk_abort = 0.8;
	const t = sine.setup.test;
	t.band_hz = [50, 800];
	t.sweep.oct_per_min = 32;
	t.sweep.sweeps = 2;
	sine.setup.profile.rows = [
		{ freq_hz: 20, quantity: 'accel_g', value: 1, own_bands: false, alarm_db: [1.5, 1.5], abort_db: [3, 3] },
		{ freq_hz: 2000, quantity: 'accel_g', value: 1, own_bands: false, alarm_db: [1.5, 1.5], abort_db: [3, 3] }
	];
	sine.setup.schedule.levels = [-6, 0];
	sine.setup.schedule.start_db = -6;
	sine.setup.schedule.full_level = { by: 'sweeps', n: 1 };
	sine.setup.identification.review = false;
	sine.setup.record = { operator: 'A. Operator', article: 'Bracket, serial 7', notes: ' first article ', document: 'Test procedure 12', clause: '4.2', streams: true };
}

describe("Sine's run record", () => {
	it('is a section of the setup, named as Random names its panel, with no capture section', () => {
		expect(SINE_SECTIONS.map((s) => s.id)).toContain('record');
		expect(SINE_SECTIONS.find((s) => s.id === 'record')?.label).toBe('Run Record');
		expect(SINE_SECTIONS.map((s) => s.id)).not.toContain('capture');
	});

	it("sends the operator's words and the streams switch, and reads them back", async () => {
		const { sine } = await screen();
		short(sine);
		const c = toSineConfig(sine.setup, null);
		expect([c.operator, c.article, c.notes, c.specification, c.record]).toEqual(['A. Operator', 'Bracket, serial 7', 'first article', { document: 'Test procedure 12', clause: '4.2' }, { streams: true }]);
		expect(fromSineConfig(c).record).toEqual({ ...sine.setup.record, notes: 'first article' });
		expect(await sine.apply()).toBe(true);
		expect(sine.runConfig?.operator).toBe('A. Operator');
		// the size of an hour of streams at the rate the run asks for (the usual one when none is given)
		expect(sine.recordRateHz).toBe(16384);
		expect(recordBytesPerHour(sine.recordChannels, sine.recordRateHz)).toBe(sine.recordChannels * 16384 * 4 * 3600);
	});

	it('names the folder from arm to the end, the time histories off when the setup says so', async () => {
		const { sine } = await screen({ dataRoot: '/data/hf' });
		short(sine);
		sine.setup.record.streams = false;
		expect(await sine.apply()).toBe(true);
		expect(sine.status?.record).toBeUndefined();
		expect(await sine.start()).toBe(true);
		await until(() => !!sine.status?.record);
		const record = sine.status!.record!;
		expect(record.folder).toMatch(/^\/data\/hf\/\d{8}-\d{6}_sine_sine-\d{4}$/);
		expect(record.streams).toBe('off');
		expect(sine.run?.record?.folder).toBe(record.folder);
		await until(() => sine.status?.state === 'finished', 20000);
		expect(sine.status?.record?.folder).toBe(record.folder);
		expect(runFolderName(new Date(Date.UTC(2026, 10, 13, 18, 4, 5)), 'sine-0001', 'sine')).toBe('20261113-180405_sine_sine-0001');
	});

	it('says a recording problem while the run goes on, and the run still ends as it would', async () => {
		const { sine } = await screen({ record: 'problem' });
		short(sine);
		expect(await sine.apply()).toBe(true);
		expect(await sine.start()).toBe(true);
		await until(() => !!sine.run?.record?.problem, 20000);
		const words = recordProblem(sine.run?.record, false);
		expect(words?.title).toBe('Recording Has a Problem');
		expect(words?.text).toMatch(/^The run goes on\. Recording of the inputs stream stopped/);
		await until(() => sine.status?.state === 'finished', 20000);
		expect(sine.status?.end?.outcome).toBe('completed');
	});

	it("refuses the arm in words when the record cannot be written, and nothing is armed", async () => {
		const { sine } = await screen({ record: 'unwritable' });
		short(sine);
		expect(await sine.apply()).toBe(true);
		expect(await sine.arm()).toBe(false);
		expect(sine.refusal?.reason).toBe('record_unwritable');
		expect(sine.refusal?.title).toBe("The Run's Record Cannot Be Written");
		expect(sine.refusal?.facts.some((f) => f.startsWith('Path: '))).toBe(true);
		expect(sine.status?.state).toBe('configured');
	});

	it('opens a finished run in the Sine workspace', async () => {
		expect(workspaceForRun('sine')).toBe('sine');
		const shown: string[] = [];
		const open = new RunOpen({ bring: async () => ['Channel Table', 'Specification', 'Sine Levels'], show: (w) => shown.push(w) }, 'sine');
		expect(await open.open('/data/hf/20261113-180405_sine_sine-0001')).toBe(true);
		expect(shown).toEqual(['sine']);
		expect(open.added).toContain('Sine Levels');
	});
});
