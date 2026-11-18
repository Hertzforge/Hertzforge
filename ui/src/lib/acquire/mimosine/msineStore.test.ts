/**
 * The MIMO Sine store over the real client and codec, against the mock control service: the law
 * catalog of the sine_mimo world, the laws a setup cannot run and why in plain words, the draft
 * fitted to the table, a sweep, a dwell and a stepped sine from identification through the review
 * to completion, a limit channel notching, the review's sweep-rate warning, a protection's trip in
 * plain words, the progress verbs, and the run slot shared with the other test types.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ChannelRow } from '$lib/protocol/types';
import { AcquireState } from '../store.svelte';
import { MimoShockStore } from '../mimoshock/msStore.svelte';
import { MimoSineStore } from './msineStore.svelte';
import { msineTripFacts, resonanceWords } from './msineWords';

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

async function screen(options: ControlServerOptions = {}, controls = 4, drives = 4, monitors = 1) {
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 40, ...options }), 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	acq.rows = [
		...Array.from({ length: controls }, (_, i) => row(i + 1, 'control')),
		...Array.from({ length: drives }, (_, i) => row(controls + i + 1, 'drive')),
		...Array.from({ length: monitors }, (_, i) => row(controls + drives + i + 1, 'monitor'))
	];
	expect(await acq.applyTable()).toBe(true);
	const ms = new MimoSineStore(acq, { local: null, checkDelayMs: 0 });
	await ms.sync();
	for (const x of ms.setup.drives) {
		x.ceiling_vpk = 4;
		x.abort_vpk = 6;
	}
	return { client, acq, ms };
}

async function run(ms: MimoSineStore) {
	await ms.checkSetup();
	expect(ms.check?.errors).toEqual([]);
	expect(ms.actions.apply.reason).toBeNull();
	expect(await ms.apply()).toBe(true);
	expect(await ms.start()).toBe(true);
	await until(() => ms.status?.state === 'review');
	expect(await ms.proceed()).toBe(true);
}

describe('the MIMO Sine store', () => {
	it("lists the sine_mimo world's laws as the catalog gives them, and opens on the first that accepts the setup", async () => {
		const { ms, client } = await screen();
		const all = (await client.request<{ laws: { id: string; world: string }[] }>('laws.list', { world: 'sine_mimo' })).laws.filter((l) => l.world === 'sine_mimo');
		expect(ms.laws?.map((l) => l.id)).toEqual(all.map((l) => l.id));
		expect(ms.setup.law.id).toBe(ms.offeredLaws[0].id);
		expect(Object.keys(ms.setup.law.params).sort()).toEqual(ms.offeredLaws[0].params.map((p) => p.key).sort());
	});

	it('cannot choose a law that does not accept the test type, and says why in plain words', async () => {
		const { ms } = await screen();
		// a sweep: the laws that accept a dwell only are not offered
		const secant = ms.refusedLaws.find((r) => r.law.id === 'secant');
		expect(secant?.why).toBe('Not for a sweep: secant accepts a dwell only.');
		expect(ms.refusedLaws.find((r) => r.law.id === 'ratio')?.why).toBe('Not for a sweep: ratio accepts a dwell or a stepped sine.');
		expect(ms.chooseLaw('secant')).toBe(false);
		expect(ms.setup.law.id).not.toBe('secant');
		// a dwell offers it, and a change back to a sweep says why the choice no longer stands
		ms.setup.test.kind = 'dwell';
		expect(ms.offeredLaws.map((l) => l.id)).toContain('secant');
		expect(ms.chooseLaw('secant')).toBe(true);
		ms.setup.test.kind = 'sweep';
		expect(ms.fieldErrors['law.id']).toEqual(['Not for a sweep: secant accepts a dwell only.']);
		expect(ms.counts.law.errors).toBe(1);
	});

	it('refuses a law that needs a phase when the references are amplitudes only', async () => {
		const { ms } = await screen();
		ms.setup.test.kind = 'dwell';
		ms.setup.phase_specified = false;
		const refused = ms.refusedLaws.filter((r) => r.why.startsWith('Needs a phase')).map((r) => r.law.id);
		const needing = (ms.laws ?? []).filter((l) => (l as { amplitude_only?: boolean }).amplitude_only === false).map((l) => l.id);
		expect(needing.length).toBeGreaterThan(0);
		expect(refused).toEqual(needing);
	});

	it('fits the draft to the table and asks for every drive ceiling before it applies', async () => {
		const { ms } = await screen();
		expect(ms.setup.controls.map((c) => c.channel)).toEqual([1, 2, 3, 4]);
		expect(ms.setup.drives.map((d) => d.channel)).toEqual([5, 6, 7, 8]);
		ms.setup.drives[2].ceiling_vpk = NaN;
		expect(ms.actions.apply.reason).toBe('Fix the error in the setup first.');
		expect(ms.counts.channels.errors).toBe(1);
		ms.setup.drives[2].ceiling_vpk = 4;
		await ms.checkSetup();
		expect(ms.check?.errors).toEqual([]);
		expect(ms.actions.apply.enabled).toBe(true);
	});

	it("shows the service's refusals under the field they name", async () => {
		const { ms } = await screen();
		ms.setup.controls[1].profile.rows[1].freq_hz = 500;
		await ms.checkSetup();
		expect(ms.findings.errors.map((e) => e.reason)).toEqual(['profile_not_covering']);
		expect(ms.fieldErrors['controls.1.profile.rows']?.[0]).toMatch(/covers the test's band/);
		expect(ms.counts.references.errors).toBe(1);
	});

	it('runs a sweep through the review to completion, with a limit channel notching and every pass reported', async () => {
		const { ms, client } = await screen();
		expect(ms.addLimit()).toBe(true);
		ms.setup.limits.channels[0].profile.rows.forEach((r) => (r.value = 1));
		await ms.checkSetup();
		expect(ms.check?.errors).toEqual([]);
		expect(await ms.apply()).toBe(true);
		expect(await ms.start()).toBe(true);
		await until(() => ms.status?.state === 'review');
		// the review flags a resonance the sweep passes too fast for, in words
		expect(ms.flaggedResonances.length).toBe(1);
		expect(resonanceWords(ms.flaggedResonances[0])).toMatch(/a slower sweep is the remedy/);
		ms.acknowledgement = 'Reviewed the identification.';
		expect(await ms.proceed()).toBe(true);
		expect(ms.status?.review?.acknowledged).toBe('Reviewed the identification.');
		let notched = false;
		client.on('mimo_sine.state', (d) => {
			if ((d as { notch: boolean }).notch) notched = true;
		});
		await until(() => ms.status?.state === 'finished');
		expect(ms.status?.end?.outcome).toBe('completed');
		expect(notched).toBe(true);
		expect(ms.lastPass?.views.some((v) => v.title === 'Limit 9')).toBe(true);
		await until(() => ms.report !== null);
		expect(ms.report?.limits[0].notched_s).toBeGreaterThan(0);
		expect(Math.max(...ms.report!.controls.map((c) => c.notch_depth_max_db))).toBeGreaterThan(0);
		expect(ms.report?.controls.length).toBe(4);
		expect(ms.log.some((e) => /notch/i.test(e.text))).toBe(true);
		// A window that opens on the finished run is drawn the last pass, not empty plots.
		const frame = new Promise<{ plots: { views: { series: { x: ArrayLike<number> }[] }[] }[] }>((resolve) => client.on('mimo_sine.data', (d) => resolve(d as never)));
		await client.request('mimo_sine.subscribe', { rate_hz: 5, plots: [{ id: 'c', quantity: 'control', channels: 'all' }] });
		const views = (await frame).plots[0].views;
		expect(views.length).toBe(4);
		expect(views.every((v) => v.series.every((x) => x.x.length > 10))).toBe(true);
	});

	it('runs a dwell and a stepped sine to completion', async () => {
		const { ms } = await screen({}, 4, 4, 0);
		ms.setup.test.kind = 'dwell';
		ms.setup.schedule.per_level = { by: 'seconds', n: 2 };
		ms.setup.schedule.full_level = { by: 'seconds', n: 2 };
		ms.setup.test.dwell.freq_hz = 100;
		for (const c of ms.setup.controls) c.profile.rows.forEach((r) => (r.freq_hz = r.freq_hz === 20 ? 20 : 2000));
		await run(ms);
		await until(() => ms.status?.state === 'finished');
		expect(ms.status?.end?.outcome).toBe('completed');
		expect(ms.status?.test).toBe('dwell');

		ms.setup.test.kind = 'stepped';
		ms.setup.test.stepped = { ...ms.setup.test.stepped, on: 'list', frequencies_hz: [50, 100, 200], by: 'seconds', seconds: 1 };
		expect(await ms.reset()).toBe(true);
		await run(ms);
		expect(ms.status?.steps?.of).toBe(3);
		await until(() => ms.status?.state === 'finished');
		expect(ms.status?.end?.outcome).toBe('completed');
	});

	it('holds and resumes a sweep, and hides the sweep verbs in a dwell', async () => {
		const { ms } = await screen({ speed: 4 }, 4, 4, 0);
		await run(ms);
		await until(() => ms.now?.phase === 'engaged');
		expect(ms.verbs.hold.enabled).toBe(true);
		expect(ms.verbs.jog.visible).toBe(true);
		// A MIMO Sine sweep turns only at the band's edges: Reverse is never offered.
		expect(Object.keys(ms.verbs).sort()).toEqual(['goto', 'hold', 'jog', 'resume']);
		expect('reverse' in ms).toBe(false);
		expect(await ms.hold()).toBe(true);
		expect(ms.status?.held).toBe(true);
		expect(ms.verbs.resume.visible).toBe(true);
		expect(await ms.resume()).toBe(true);
		expect(ms.status?.held).toBe(false);
		expect(await ms.abort()).toBe(true);
		expect(ms.status?.end?.outcome).toBe('operator_abort');
		ms.setup.test.kind = 'dwell';
		ms.setup.schedule.per_level = { by: 'seconds', n: 2 };
		ms.setup.schedule.full_level = { by: 'seconds', n: 2 };
		await ms.checkSetup();
		expect(await ms.apply()).toBe(true);
		expect(await ms.start()).toBe(true);
		await until(() => ms.status?.state === 'review');
		await ms.proceed();
		expect(ms.verbs.jog.visible).toBe(false);
		expect(ms.verbs.goto.visible).toBe(false);
		await ms.abort();
	});

	it("ends a run whose control passes its abort band, and says what tripped in words", async () => {
		const { ms } = await screen({}, 4, 4, 0);
		ms.setup.simulator.structure = 'sine-softening-4x4-moderate';
		await run(ms);
		await until(() => ms.status?.state === 'finished');
		const end = ms.status?.end;
		expect(end?.outcome).toBe('aborted');
		expect(end?.trip?.kind).toBe('abort_band');
		const facts = msineTripFacts(end!.trip!);
		expect(facts.find((f) => f.label === 'Control')?.value).toBe('2');
		expect(facts.some((f) => f.label === 'Upper abort')).toBe(true);
	});

	it("refuses a notch release hysteresis out of 0 to 6 dB at its field, and takes the new structures", async () => {
		const { ms } = await screen({}, 4, 4, 0);
		ms.setup.limits.notch.release_hysteresis_db = 7;
		await ms.checkSetup();
		expect(ms.findings.errors.map((e) => [e.reason, e.field])).toEqual([['out_of_range', 'limits.notch.release_hysteresis_db']]);
		expect(ms.fieldErrors['limits.notch.release_hysteresis_db']?.length).toBe(1);
		expect(ms.counts.limits.errors).toBe(1);
		ms.setup.limits.notch.release_hysteresis_db = 0;
		for (const structure of ['sine-weak-4x4', 'sine-loopback-4x4']) {
			ms.setup.simulator.structure = structure;
			await ms.checkSetup();
			expect(ms.check?.errors).toEqual([]);
		}
	});

	it('says the notch is in while the reference is held down, with notch_scale the drive cut alone', async () => {
		const { ms } = await screen({ speed: 4 });
		expect(ms.addLimit()).toBe(true);
		ms.setup.limits.channels[0].profile.rows.forEach((r) => (r.value = 1));
		await run(ms);
		await until(() => ms.now?.phase === 'engaged');
		expect(await ms.hold()).toBe(true);
		// where the limit channel's response passes its bound, held there until the depth settles
		const [lo, hi] = ms.check!.derived!.band_hz;
		expect(await ms.gotoHz(Math.round(lo * (hi / lo) ** 0.45 * 1.4))).not.toBeNull();
		await until(() => {
			const f = ms.frameOfRun;
			return !!f && f.notch && f.notch_scale === 1 && f.controls.some((c) => c.notch_depth_db > 0);
		});
		expect(ms.now?.notch).toBe(true);
		await ms.abort();
	});

	it('refuses a simulator structure of another size by name', async () => {
		const { ms } = await screen({}, 4, 4, 0);
		ms.setup.simulator.structure = 'sine-tall-6x3';
		await ms.checkSetup();
		expect(ms.findings.errors.map((e) => e.reason)).toEqual(['structure_wrong_size']);
		expect(ms.fieldErrors['simulator.structure']?.length).toBe(1);
		expect(ms.counts.channels.errors).toBe(1);
	});

	it('holds the one run slot: another MIMO test is refused while a MIMO Sine run is live', async () => {
		const { ms, acq } = await screen({}, 4, 4, 0);
		const shock = new MimoShockStore(acq, { local: null, checkDelayMs: 0 });
		await shock.sync();
		for (const x of shock.setup.drives) {
			x.ceiling_vpk = 4;
			x.abort_vpk = 6;
		}
		expect(await shock.apply()).toBe(true);
		await run(ms);
		expect(ms.isLive).toBe(true);
		expect(await shock.start()).toBe(false);
		expect(shock.refusal?.reason).toBe('run_active');
		await ms.abort();
	});
});
