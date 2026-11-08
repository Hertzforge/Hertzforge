/**
 * The classical shock store over the real client and codec, against the mock control service: the
 * law catalog of the classical world, the preview and the dry run of the draft, the setup applied,
 * a run from identification through the review hold to the end of the schedule, the manual
 * ladder's verbs and their refusals, the pulses laid over the plots, and the run slot shared with
 * Random.
 */
import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory, type MockConnection, type MockServer } from '$lib/protocol/mock/socket';
import type { ShockLive, ShockState } from '$lib/protocol/shock';
import { AcquireState } from '../store.svelte';
import { ShockStore } from './shockStore.svelte';
import { lastPulseWords } from './shockWords';

const until = async (cond: () => boolean, ms = 6000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

async function screen(options: ControlServerOptions = {}) {
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 20, ...options }), 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	const shock = new ShockStore(acq, { local: null, checkDelayMs: 0 });
	await shock.sync();
	return { client, acq, shock };
}

function ready(shock: ShockStore) {
	shock.setup.limits.drive_vpk_ceiling = 0.5;
	shock.setup.limits.drive_vpk_abort = 0.8;
}

describe('the classical shock store', () => {
	it("reads the classical world's laws and starts the law at its defaults", async () => {
		const { shock } = await screen();
		expect(shock.classicalLaws.map((l) => l.id)).toEqual(['fd_correct']);
		expect(shock.law?.name).toBe('frequency-domain correction');
		expect(shock.setup.law.params).toEqual({ beta: 0.5, max_db_per_pulse: 6, initial_backoff_db: 0 });
	});

	it('asks for the drive ceiling before the setup can be applied', async () => {
		const { shock } = await screen();
		expect(shock.actions.apply.reason).toBe('Fix the 2 errors in the setup first.');
		expect(shock.counts.limits.errors).toBe(2);
		ready(shock);
		expect(shock.actions.apply.enabled).toBe(true);
	});

	it("draws the pulse from the service's preview, and warns of shoulders that leave the quiet window", async () => {
		const { shock } = await screen();
		ready(shock);
		await shock.refreshPreview();
		expect(shock.pulsePreview?.views.map((v) => v.title)).toEqual(['Pulse', 'Velocity', 'Displacement']);
		expect(shock.pulsePreview?.derived.quiet_window.inside).toBe(true);
		shock.setup.pulse.compensation = { shape: 'half_sine', pre_frac: 1.5, post_frac: 1.5, symmetric: true };
		expect(shock.pulsePreview).toBeNull();
		await shock.refreshPreview();
		expect(shock.pulsePreview?.derived.quiet_window.inside).toBe(false);
		await shock.checkSetup();
		expect(shock.fieldWarnings['pulse.compensation']?.[0]).toMatch(/quiet window/);
		expect(shock.counts.pulse.warnings).toBe(1);
	});

	it('runs a schedule from identification through the review hold to its end', async () => {
		const { shock } = await screen();
		ready(shock);
		expect(await shock.apply()).toBe(true);
		expect(shock.setupStatus).toBe('applied');
		expect(shock.status?.state).toBe('configured');
		expect(shock.primary).toBe('start');
		expect(await shock.start()).toBe(true);
		expect(shock.step).toBe('run');
		await until(() => shock.status?.state === 'review');
		expect(shock.status?.identification?.passed).toBe(true);
		expect(shock.actions.proceed.enabled).toBe(true);
		expect(await shock.proceed()).toBe(true);
		await until(() => shock.status?.state === 'finished');
		expect(shock.status?.end?.outcome).toBe('completed');
		await until(() => shock.pulses.length === 8);
		// the first pulse came in low and the law corrected it; every pulse after passed
		expect(lastPulseWords(shock.pulses[0], true)?.text).toBe('Pulse 1 was 25% low; correcting');
		expect(shock.pulses.slice(1).every((p) => p.passed)).toBe(true);
		expect(shock.pulses.map((p) => p.commanded_db)).toEqual([-12, -12, -9, -6, -3, 0, 0, 0]);
		await shock.toggleOverlay(0);
		await shock.toggleOverlay(1);
		expect(shock.overlaid.map((o) => o.index)).toEqual([0, 1]);
		expect(shock.overlaid[0].views.some((v) => v.title === 'SRS')).toBe(true);
		await shock.toggleOverlay(0);
		expect(shock.overlay).toEqual([1]);
		expect(shock.primary).toBe('reset');
	});

	it('fires the manual ladder on the operator word, and names a fire too early', async () => {
		const { shock } = await screen({ speed: 4 });
		ready(shock);
		shock.setup.schedule.mode = 'manual';
		shock.setup.schedule.start_db = -9;
		shock.setup.identification.review = false;
		expect(await shock.apply()).toBe(true);
		await shock.start();
		await until(() => shock.status?.phase === 'waiting');
		expect(shock.status?.level_db).toBe(-9);
		expect(shock.verbs.fire.visible).toBe(true);
		await until(() => shock.verbs.fire.enabled, 4000);
		expect(await shock.fire()).toBe(true);
		await until(() => shock.pulses.length === 1);
		// straight after a pulse the time between pulses has not passed
		expect(await shock.fire()).toBe(false);
		expect(shock.refusal?.reason).toBe('not_yet');
		expect(shock.refusal?.title).toBe('Not Yet');
		expect(await shock.level(1)).toBe(-8);
		expect(await shock.toRung(4)).toBe(0);
		expect(await shock.stop()).toBe(true);
		await until(() => shock.status?.state === 'finished');
		expect(shock.status?.end?.outcome).toBe('operator_stop');
	});

	it('refuses the manual verbs on an automatic schedule, and the run slot both ways with Random', async () => {
		const { client, shock } = await screen({ speed: 1 });
		ready(shock);
		await shock.apply();
		await shock.start();
		const err = await client.request('shock.fire').catch((e: unknown) => e);
		expect((err as RequestError).reason).toBe('wrong_mode');
		const randomConfig = {
			title: 'Random',
			profile: { breakpoints: [{ freq_hz: 20, psd_g2_hz: 0.01 }, { freq_hz: 2000, psd_g2_hz: 0.01 }], alarm_db: [3, 3], abort_db: [6, 6] },
			acquisition: { fs_hz: 8192, n_pts: 1024, n_avg: 2 },
			schedule: { mode: 'auto', levels: [{ db: 0 }], full_level_s: 2 }
		};
		const random = await client.request('random.configure', { config: randomConfig }).catch((e: unknown) => e);
		expect((random as RequestError).reason).toBe('run_active');
		const sim = await client.request('sim.start', { profile: 'qual', level_db: 0 }).catch((e: unknown) => e);
		expect((sim as RequestError).reason).toBe('run_active');
		expect(await shock.abort()).toBe(true);
		await until(() => shock.status?.state === 'finished');
		expect(shock.status?.end?.outcome).toBe('operator_abort');
	});

	it('pauses firing when nobody holds control, and safe-stops at the timeout', async () => {
		const { acq, shock } = await screen({ speed: 20, timeoutS: 0.3 });
		ready(shock);
		shock.setup.identification.review = false;
		await shock.apply();
		await shock.start();
		await until(() => shock.status?.phase === 'waiting');
		await acq.release();
		await until(() => shock.status?.state === 'finished', 4000);
		expect(shock.status?.end?.outcome).toBe('safe_stop');
	});
});

describe('the run numbers on screen', () => {
	it('take a state event over an older live frame, until the next frame comes', async () => {
		let conn: MockConnection | null = null;
		const server: MockServer = { hello: { type: 'hello', protocol: 'sds/1', service: 'control', version: 'test' }, methods: {}, attach: (c) => void (conn = c) };
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
		const shock = new ShockStore(acq, { local: null, checkDelayMs: 0 });
		client.connect();
		await client.whenOpen();
		await until(() => conn !== null);
		const pulses = (fired: number) => ({ fired, passed: fired, at_level: 0, of_level: 1 });
		const live = (fired: number, phase: ShockLive['phase']): ShockLive => ({ run_id: 's-0001', source: 'SIM', state: 'running', phase, mode: 'manual', level_db: -12, rung: 0, pulses: pulses(fired), next_pulse_s: null, fire: null });
		const state = { run_id: 's-0001', state: 'running', phase: 'waiting', title: 't', source: 'SIM', station: null, grid: null, clock: null, mode: 'manual', levels_db: [-12, 0], level_db: -12, rung: 0, pulses: pulses(2), next_pulse_s: 0.4, elapsed_s: 3, alarm: false, identification: null, prediction: null, last_pulse: null, law: null, feasibility: null, end: null } as unknown as ShockState;
		conn!.emit('shock.live', live(1, 'firing') as unknown as Record<string, unknown>);
		await until(() => shock.live !== null);
		conn!.emit('shock.state', state as unknown as Record<string, unknown>);
		await until(() => shock.status !== null);
		// the pulse in flight has been judged: the state says so, and the older frame must not undo it
		expect([shock.now?.phase, shock.now?.pulses.fired]).toEqual(['waiting', 2]);
		conn!.emit('shock.live', { ...live(2, 'waiting'), next_pulse_s: 0.2 } as unknown as Record<string, unknown>);
		await until(() => shock.live !== null);
		expect(shock.now?.next_pulse_s).toBe(0.2);
	});
});
