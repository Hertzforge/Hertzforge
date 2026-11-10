/**
 * The SRS shock store over the real client and codec, against the mock control service: the law
 * catalog of the SRS world, the synthesis preview and the dry run of the draft, the tables read
 * from files, the setup applied, a run from identification through the review hold to the end of
 * the schedule, the manual ladder's verbs and their refusals, the pulses laid over the plots, the
 * run slot shared with classical shock and Random, and a window's subscription. The mock refuses
 * as the service does; the last part holds it to that.
 */
import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { AcquireState } from '../store.svelte';
import { ShockStore } from '../shock/shockStore.svelte';
import { SrsStore } from './srsStore.svelte';
import { SrsViewsState } from './srsViews.svelte';
import { lastPulseWords, notchWords, worstBandWords } from './srsWords';

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
	const srs = new SrsStore(acq, { local: null, checkDelayMs: 0 });
	await srs.sync();
	return { client, acq, srs };
}

function ready(srs: SrsStore) {
	srs.setup.limits.drive_vpk_ceiling = 0.5;
	srs.setup.limits.drive_vpk_abort = 0.8;
}

const refusal = async (p: Promise<unknown>) => (await p.catch((e: unknown) => e)) as RequestError;

describe('the SRS shock store', () => {
	it("reads the SRS world's laws only, and starts the law at its defaults", async () => {
		const { srs } = await screen();
		expect(srs.laws?.map((l) => l.id)).toEqual(['srs_ratio']);
		expect(srs.srsLaws.map((l) => l.id)).toEqual(['srs_ratio']);
		expect(srs.law?.name).toBe('SRS ratio');
		expect(srs.setup.law.params).toEqual({ max_db_per_pulse: 3 });
	});

	it("imports an SRS law file and shows its parameters from its manifest; a classical one is not chosen", async () => {
		const { srs } = await screen();
		expect(await srs.importLaw('/laws/srs_template.wasm')).toBe(true);
		expect(srs.srsLaws.map((l) => l.name)).toEqual(['SRS ratio', 'SRS template']);
		expect(srs.law?.name).toBe('SRS template');
		expect(srs.setup.law.params).toEqual({ correct: true, seed_error_db: 0 });
		expect(srs.wire.control?.law?.params).toEqual({ correct: true, seed_error_db: 0 });
		expect(await srs.importLaw('/laws/shock_template.wasm')).toBe(true);
		expect(srs.note?.text).toMatch(/not an SRS shock law/);
		expect(srs.srsLaws).toHaveLength(2);
		expect(await srs.importLaw('laws/relative.wasm')).toBe(false);
		expect(srs.refusal?.reason).toBe('bad_params');
	});

	it('asks for the drive ceiling before the setup can be applied', async () => {
		const { srs } = await screen();
		expect(srs.actions.apply.reason).toBe('Fix the 2 errors in the setup first.');
		expect(srs.counts.limits.errors).toBe(2);
		ready(srs);
		expect(srs.actions.apply.enabled).toBe(true);
	});

	it("draws the target, the synthesis and the pulse from the service's preview, and names a target that does not cover the grid", async () => {
		const { srs } = await screen();
		ready(srs);
		await srs.refreshPreview();
		expect(srs.synthesis?.views.map((v) => v.title)).toEqual(['Target', 'Synthesis', 'Pulse', 'Velocity', 'Displacement']);
		expect(srs.bandsHz).toHaveLength(14);
		expect(srs.synthesis?.derived.synthesis.converged).toBe(true);
		srs.setup.target.points[2].freq_hz = 1500;
		expect(srs.synthesis).toBeNull();
		await srs.refreshPreview();
		expect(srs.previewError).toMatch(/covers 100 to 1500 Hz/);
		await srs.checkSetup();
		expect(srs.fieldErrors['target.points']?.[0]).toMatch(/covers 100 to 1500 Hz/);
		expect(srs.counts.target.errors).toBe(1);
		srs.setup.target.points[2].freq_hz = 2500;
		srs.setup.synthesis.tol_db = 0.2;
		await srs.checkSetup();
		expect(srs.fieldWarnings['synthesis.tol_db']?.[0]).toMatch(/did not reach/);
		expect(srs.counts.synthesis.warnings).toBe(1);
	});

	it('reads a target and a component table from files, and names a file it cannot read', async () => {
		const { srs } = await screen();
		expect(await srs.importTarget('/data/target.csv')).toBe(true);
		expect(srs.setup.target.points.map((p) => [p.freq_hz, p.accel_g])).toEqual([
			[100, 30],
			[800, 120],
			[2000, 120]
		]);
		expect(await srs.importTarget('/data/missing.csv')).toBe(false);
		expect(srs.imported?.errors[0].reason).toBe('target_file');
		expect(await srs.importTarget('data/target.csv')).toBe(false);
		expect([srs.refusal?.reason, srs.refusal?.detail?.param]).toEqual(['bad_params', 'path']);
		srs.setup.synthesis.engine = 'seds';
		expect(await srs.importTable('/data/components.csv')).toBe(true);
		expect([srs.setup.synthesis.table, srs.setup.synthesis.sines.length, srs.wire.synthesis?.components?.engine]).toEqual([true, 5, 'seds']);
	});

	it('fills a per-band envelope and bound from the band grid the service gives', async () => {
		const { srs } = await screen();
		await srs.refreshPreview();
		srs.setPerBandEnvelope(true);
		expect(srs.setup.envelope.bands).toHaveLength(14);
		expect(srs.setup.envelope.bands[0]).toEqual({ alarm_db: [3, 3], abort_db: [9, 6] });
		srs.setup.limit_channels.enabled = true;
		srs.setup.limit_channels.channels.push({ channel: 1, bound: { kind: 'flat', accel_g: 50, bounds_g: [] }, frf: { supplied: false, points: [] } });
		srs.setBoundKind(0, 'per_band');
		expect(srs.wire.limit_channels?.channels[0].bound).toEqual({ kind: 'per_band', bounds_g: Array(14).fill(50) });
		srs.setup.bands.bands_per_octave = 6;
		await srs.refreshPreview();
		expect(srs.bandsHz).toHaveLength(27);
		expect(srs.fitPerBand()).toBe(true);
		expect([srs.setup.envelope.bands.length, srs.setup.limit_channels.channels[0].bound.bounds_g.length]).toEqual([27, 27]);
	});

	it('runs a schedule from identification through the review hold to its end', async () => {
		const { srs } = await screen();
		ready(srs);
		expect(await srs.apply()).toBe(true);
		expect([srs.setupStatus, srs.status?.state, srs.primary]).toEqual(['applied', 'configured', 'start']);
		expect(await srs.start()).toBe(true);
		expect(srs.step).toBe('run');
		await until(() => srs.status?.state === 'review');
		expect(srs.status?.identification).toMatchObject({ excitation: 'shaped', max_vpk: 0.05, passed: true });
		expect(srs.actions.proceed.enabled).toBe(true);
		expect(await srs.proceed()).toBe(true);
		await until(() => srs.status?.state === 'finished');
		expect(srs.status?.end?.outcome).toBe('completed');
		await until(() => srs.pulses.length === 8);
		// the first pulse came in low in the upper bands and the law corrected it
		expect(lastPulseWords(srs.pulses[0], true)?.text).toMatch(/^Pulse 1: \d+ bands outside the alarm band; correcting$/);
		expect(worstBandWords(srs.pulses[0])).toMatch(/^Worst band \d+ Hz, \d\.\d+ dB low$/);
		expect(srs.pulses.slice(1).every((p) => p.passed)).toBe(true);
		expect(srs.pulses.map((p) => p.commanded_db)).toEqual([-12, -12, -12, -6, -6, 0, 0, 0]);
		await srs.toggleOverlay(0);
		await srs.toggleOverlay(1);
		expect(srs.overlaid.map((o) => o.index)).toEqual([0, 1]);
		expect(srs.overlaid[0].views.map((v) => v.title)).toEqual(['SRS', 'Pulse', 'Drive', 'Velocity', 'Displacement']);
		await srs.toggleOverlay(0);
		expect(srs.overlay).toEqual([1]);
		expect(srs.primary).toBe('reset');
	});

	it('plans the limit notch after the review, and holds components back for the limit channels', async () => {
		const { acq, srs } = await screen();
		acq.addRow('monitor');
		const fixture = acq.rows.at(-1)!.index;
		ready(srs);
		srs.setup.identification.review = false;
		srs.setup.limit_channels.enabled = true;
		srs.setup.limit_channels.channels = [{ channel: fixture, bound: { kind: 'flat', accel_g: 50, bounds_g: [] }, frf: { supplied: true, points: [{ freq_hz: 100, mag: 1 }, { freq_hz: 500, mag: 4 }, { freq_hz: 2000, mag: 1 }] } }];
		expect(await srs.apply()).toBe(true);
		await srs.start();
		await until(() => srs.pulses.length >= 1);
		expect(notchWords(srs.status?.notch ?? null)).toMatch(/^2 bands notched/);
		expect(srs.pulses[0].limited_components).toBe(2);
		await srs.abort();
	});

	it('fires the manual ladder on the operator word, and names a fire too early and a step off the table', async () => {
		const { client, srs } = await screen({ speed: 4 });
		ready(srs);
		srs.setup.schedule.mode = 'manual';
		srs.setup.schedule.start_db = -6;
		srs.setup.identification.review = false;
		expect(await srs.apply()).toBe(true);
		await srs.start();
		await until(() => srs.status?.phase === 'waiting');
		expect(srs.status?.level_db).toBe(-6);
		expect([srs.verbs.fire.visible, srs.verbs.train.visible, srs.verbs.burst.visible]).toEqual([true, false, false]);
		await until(() => srs.verbs.fire.enabled, 4000);
		expect(await srs.fire()).toBe(true);
		await until(() => srs.pulses.length === 1);
		// straight after a pulse the time between pulses has not passed
		expect(await srs.fire()).toBe(false);
		expect([srs.refusal?.reason, srs.refusal?.title]).toEqual(['not_yet', 'Not Yet']);
		expect(await srs.level(1)).toBe(-5);
		// a step is one decibel; the service refuses any other
		expect((await refusal(client.request('srs.level', { delta_db: 2 }))).reason).toBe('bad_params');
		expect(await srs.toRung(2)).toBe(0);
		expect(await srs.level(1)).toBeNull();
		expect(srs.refusal?.reason).toBe('not_a_rung');
		expect(await srs.stop()).toBe(true);
		await until(() => srs.status?.state === 'finished');
		expect(srs.status?.end?.outcome).toBe('operator_stop');
	});

	it("offers Fire again once the live frame says the time between pulses has passed", async () => {
		const { srs } = await screen({ speed: 4 });
		ready(srs);
		srs.setup.schedule.mode = 'manual';
		srs.setup.identification.review = false;
		expect(await srs.apply()).toBe(true);
		await srs.start();
		await until(() => srs.verbs.fire.enabled, 4000);
		expect(await srs.fire()).toBe(true);
		await until(() => srs.pulses.length === 1 && srs.status?.phase === 'waiting');
		expect(srs.verbs.fire.enabled).toBe(false);
		// the state said the next pulse was a while off; the live frames count it down
		await until(() => srs.live?.phase === 'waiting' && (srs.now?.next_pulse_s ?? 1) <= 0.05, 4000);
		expect(srs.verbs.fire.reason).toBeNull();
		expect(srs.verbs.fire.enabled).toBe(true);
		await srs.abort();
	});

	it('refuses the manual verbs on an automatic schedule, and the run slot both ways with classical shock and Random', async () => {
		const { client, acq, srs } = await screen({ speed: 1 });
		ready(srs);
		await srs.apply();
		await srs.start();
		expect((await refusal(client.request('srs.fire'))).reason).toBe('wrong_mode');
		const shock = new ShockStore(acq, { local: null, checkDelayMs: 0 });
		await shock.sync();
		shock.setup.limits.drive_vpk_ceiling = 0.5;
		shock.setup.limits.drive_vpk_abort = 0.8;
		expect(await shock.apply()).toBe(true);
		expect(await shock.start()).toBe(false);
		expect(shock.refusal?.reason).toBe('run_active');
		const sim = await refusal(client.request('sim.start', { profile: 'qual', level_db: 0 }));
		expect(sim.reason).toBe('run_active');
		expect(await srs.abort()).toBe(true);
		await until(() => srs.status?.state === 'finished');
		expect(srs.status?.end?.outcome).toBe('operator_abort');
		// and the other way: a live classical run holds the slot against SRS
		await shock.start();
		await until(() => shock.isLive);
		expect(await srs.reset()).toBe(true);
		expect(await srs.start()).toBe(false);
		expect(srs.refusal?.reason).toBe('run_active');
		await shock.abort();
	});

	it('pauses firing when nobody holds control, and safe-stops at the timeout', async () => {
		const { acq, srs } = await screen({ speed: 20, timeoutS: 0.3 });
		ready(srs);
		srs.setup.identification.review = false;
		await srs.apply();
		await srs.start();
		await until(() => srs.status?.phase === 'waiting');
		await acq.release();
		await until(() => srs.status?.state === 'finished', 4000);
		expect(srs.status?.end?.outcome).toBe('safe_stop');
	});
});

describe("a window's subscription", () => {
	it('asks only for what the page draws, a point count on the time histories only, and the service accepts it', async () => {
		const { client, srs } = await screen();
		let showing = true;
		const views = new SrsViewsState(client, { setupTitle: () => 'srs test', showing: () => showing, storage: null, session: null, events: null, subscribeDelayMs: 0 });
		const stop = views.watch();
		for (const page of views.pages) {
			views.selectPage(page.id);
			await until(() => views.accepted !== null && JSON.stringify(views.accepted.plots?.map((p) => p.id)) === JSON.stringify(views.wanted.map((p) => p.id)));
			expect(views.error).toBeNull();
			for (const p of views.accepted!.plots ?? []) expect('points' in p).toBe(['pulse', 'velocity', 'displacement'].includes(p.quantity));
		}
		showing = false;
		views.selectPage(views.pages[0].id);
		stop();
		void srs;
	});
});

describe('the mock refuses as the service does', () => {
	it('a point count on a plot that is not a time history, and the shapes the service refuses', async () => {
		const { client } = await screen();
		const sub = (params: Record<string, unknown>) => refusal(client.request('srs.subscribe', params));
		let e = await sub({ plots: [{ id: 'a', quantity: 'srs', points: 512 }] });
		expect([e.reason, e.detail?.param]).toEqual(['bad_params', 'plots[0].points']);
		e = await sub({ plots: [{ id: 'a', quantity: 'pulse', points: 32 }] });
		expect(e.detail?.param).toBe('plots[0].points');
		e = await sub({ plots: [{ id: 'a', quantity: 'spectrum' }] });
		expect(e.detail?.param).toBe('plots[0].quantity');
		e = await sub({ plots: [{ id: 'a', quantity: 'srs' }, { id: 'a', quantity: 'pulse' }] });
		expect(e.detail?.param).toBe('plots[1].id');
		e = await sub({ plots: [{ id: 'a', quantity: 'srs', rate: 2 }] });
		expect(e.detail?.param).toBe('plots[0].rate');
		e = await sub({ rate_hz: 20, plots: [] });
		expect(e.detail?.param).toBe('rate_hz');
		e = await sub({ default: true, plots: [] });
		expect(e.detail?.param).toBe('plots');
		const ok = (await client.request('srs.subscribe', { plots: [{ id: 'a', quantity: 'pulse', points: 512 }, { id: 'b', quantity: 'components' }] })) as { plots: unknown[] };
		expect(ok.plots).toEqual([{ id: 'a', quantity: 'pulse', points: 512 }, { id: 'b', quantity: 'components' }]);
		const def = (await client.request('srs.subscribe', { plots: [{ id: 'a', quantity: 'velocity' }] })) as { plots: { points?: number }[] };
		expect(def.plots[0].points).toBe(1024);
	});

	it('a pulse that has not fired, a point count out of range, and a relative path', async () => {
		const { client, srs } = await screen();
		ready(srs);
		await srs.apply();
		expect((await refusal(client.request('srs.pulse', { index: 0 }))).code).toBe('not_found');
		expect((await refusal(client.request('srs.pulse', { index: 0, points: 10 }))).detail?.param).toBe('points');
		expect((await refusal(client.request('srs.pulse', {}))).detail?.param).toBe('index');
		expect((await refusal(client.request('srs.table.import', { path: '/t.csv', engine: 'other' }))).detail?.param).toBe('engine');
		expect((await refusal(client.request('srs.preview', { target: { kind: 'flat', accel_g: 10 } }))).detail?.param).toBe('bands');
		expect((await refusal(client.request('srs.configure', { config: { title: 'x' } }))).detail?.param).toBe('config.target');
		expect((await refusal(client.request('srs.proceed'))).reason).toBe('wrong_state');
	});
});
