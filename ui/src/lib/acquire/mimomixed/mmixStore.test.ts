/**
 * The MIMO Mixed Mode store over the real client and codec, against the mock control service: the
 * two law catalogs read from their own worlds, the refusals named with their part and control, the
 * reference built from the breakpoints, and a session from identification through the review, the
 * floor alone, the tones in, a drive running out of room and the tones limiting, to completion,
 * with hold, resume, a part switched off and a stop on the way.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ChannelRow } from '$lib/protocol/types';
import { AcquireState } from '../store.svelte';
import { MimoMixedStore } from './mmixStore.svelte';

const until = async (cond: () => boolean, ms = 10000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

function row(index: number, role: ChannelRow['role']): ChannelRow {
	return { index, enabled: true, description: `${role} ${index}`, role, physical: null, interface: 'single_ended', iepe_bias_v: null, units: role === 'drive' ? 'V' : 'g', sensitivity_mv_per_eu: role === 'drive' ? 1000 : 100, range_max: 10, polarity: 'positive', control_group: null, abort: { enabled: false } } as unknown as ChannelRow;
}

async function screen(options: ControlServerOptions = {}, controls = 4, drives = 2) {
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 40, ...options }), 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	acq.rows = [...Array.from({ length: controls }, (_, i) => row(i + 1, 'control')), ...Array.from({ length: drives }, (_, i) => row(controls + i + 1, 'drive'))];
	expect(await acq.applyTable()).toBe(true);
	const mm = new MimoMixedStore(acq, { local: null, checkDelayMs: 0 });
	await mm.sync();
	for (const d of mm.setup.drives) {
		d.ceiling_vrms = 2;
		d.peak_v = 4;
	}
	return { client, acq, mm };
}

async function applied(mm: MimoMixedStore) {
	await mm.checkSetup();
	expect(mm.check?.errors).toEqual([]);
	expect(mm.actions.apply.reason).toBeNull();
	expect(await mm.apply()).toBe(true);
}

describe('the MIMO Mixed Mode store', () => {
	it("reads the floor's laws from the mimo world and the tones' from sine_mimo, and opens on the first of each", async () => {
		const { mm, client } = await screen();
		const floor = (await client.request<{ laws: { id: string; world: string }[] }>('laws.list', { world: 'mimo' })).laws.filter((l) => l.world === 'mimo');
		const tones = (await client.request<{ laws: { id: string; world: string }[] }>('laws.list', { world: 'sine_mimo' })).laws.filter((l) => l.world === 'sine_mimo');
		expect(mm.lawsOf('floor').map((l) => l.id)).toEqual(floor.map((l) => l.id));
		expect(mm.lawsOf('tones').map((l) => l.id)).toEqual(tones.map((l) => l.id));
		expect(floor.length).toBeGreaterThan(0);
		expect(tones.length).toBeGreaterThan(0);
		expect(mm.chooseLaw('floor', 'no_such_law')).toBe(false);
		expect(mm.chooseLaw('tones', tones[tones.length - 1].id)).toBe(true);
		expect(mm.setup.laws.tones.id).toBe(tones[tones.length - 1].id);
	});

	it('fits the draft to the table: every control and drive, a tone amplitude and phase per control', async () => {
		const { mm } = await screen({}, 3, 2);
		expect(mm.setup.controls.map((c) => c.channel)).toEqual([1, 2, 3]);
		expect(mm.setup.drives.map((d) => d.channel)).toEqual([4, 5]);
		expect(mm.setup.tones[0].amp_g).toHaveLength(3);
		expect(mm.setup.tones[0].phase_deg).toHaveLength(3);
	});

	it("builds the reference from the breakpoints and checks the setup with the floor's band", async () => {
		const { mm } = await screen();
		await mm.checkSetup();
		expect(mm.ref?.ok).toBe(true);
		expect(mm.ref?.reference?.kind).toBe('inline');
		expect(mm.check?.errors).toEqual([]);
		expect(mm.check?.derived?.tracker_delay_s).toBeCloseTo(1.25, 2);
	});

	it('names a refusal with its part and its control: a band that would take from the floor', async () => {
		const { mm } = await screen();
		expect(mm.addBand()).toBe(true);
		const b = mm.setup.bands[0];
		b.statement.db[2] = -3;
		await mm.checkSetup();
		const item = mm.check?.errors.find((e) => e.reason === 'band_not_additive');
		expect(item?.part).toBe(b.id);
		expect(item?.control).toBe(2);
		expect(mm.findings.errors.some((e) => e.reason === 'band_not_additive')).toBe(true);
	});

	it('refuses a tone out of the floor band, by part', async () => {
		const { mm } = await screen();
		mm.setup.tones[0].schedule.freq_hz = 5000;
		await mm.checkSetup();
		const errs = mm.check?.errors ?? [];
		const local = Object.keys(mm.fieldErrors);
		expect(errs.some((e) => e.reason === 'part_out_of_band' && e.part === 't1') || local.some((k) => k.includes('t1'))).toBe(true);
	});

	it('runs a session: identify, review, the floor equalizes, the tones come in, a drive runs out of room, completed', async () => {
		const { mm } = await screen();
		await applied(mm);
		expect(await mm.start()).toBe(true);
		await until(() => mm.status?.state === 'review');
		expect(mm.status?.phase).toBe('awaiting_review');
		expect(mm.status?.gates?.passed).toBe(true);
		expect(mm.status?.prediction?.forecast.drives).toHaveLength(2);
		expect(mm.status?.prediction?.forecast.cannot_fit).toBeNull();
		expect(await mm.proceed()).toBe(true);
		expect(mm.status?.phase).toBe('floor_only');
		await until(() => mm.status?.floor_equalized === true);
		await until(() => mm.status?.parts.find((p) => p.id === 't1')?.on === true);
		await until(() => mm.status?.phase === 'full_level');
		await until(() => mm.status?.limiting === true);
		const tone = mm.status?.parts.find((p) => p.id === 't1');
		expect(tone?.limiting).toBe(true);
		expect(tone?.scale).toBeLessThan(1);
		expect(mm.status?.scale).toBeLessThan(1);
		await until(() => mm.status?.state === 'finished', 20000);
		expect(mm.status?.end?.outcome).toBe('completed');
		expect(mm.log.some((e) => e.text.includes('out of room'))).toBe(true);
	}, 30000);

	it('under in_turn only the last tone limits', async () => {
		const { mm } = await screen();
		mm.addTone();
		mm.setup.laws.ceiling_rule = 'in_turn';
		await applied(mm);
		await mm.start();
		await until(() => mm.status?.state === 'review');
		await mm.proceed();
		await until(() => mm.status?.limiting === true, 20000);
		const tones = mm.status?.parts.filter((p) => p.kind === 'tone') ?? [];
		expect(tones.map((t) => t.limiting)).toEqual([false, true]);
		expect(mm.status?.scale).toBeNull();
		await mm.abort();
	}, 30000);

	it('holds and resumes, switches a tone off, refuses the floor, and stops', async () => {
		const { mm } = await screen();
		await applied(mm);
		await mm.start();
		await until(() => mm.status?.state === 'review');
		await mm.proceed();
		await until(() => mm.status?.phase === 'engaged');
		expect(await mm.hold()).toBe(true);
		expect(mm.status?.held).toBe(true);
		expect(await mm.resume()).toBe(true);
		expect(mm.status?.held).toBe(false);
		expect(await mm.part('floor', false)).toBe(false);
		expect(mm.refusal?.reason).toBe('part_refused');
		expect(await mm.part('t1', false)).toBe(true);
		await until(() => mm.status?.parts.find((p) => p.id === 't1')?.enabled === false);
		expect(await mm.stop()).toBe(true);
		await until(() => mm.status?.state === 'finished');
		expect(mm.status?.end?.outcome).toBe('operator_stop');
	}, 30000);

	it('holds on a forecast that cannot fit, and proceeds only with words', async () => {
		const { mm } = await screen();
		for (const d of mm.setup.drives) {
			d.ceiling_vrms = 1;
			d.peak_v = 1.2;
		}
		await applied(mm);
		await mm.start();
		await until(() => mm.status?.state === 'review');
		expect(mm.status?.phase).toBe('prediction_exceeded');
		expect(mm.status?.prediction?.forecast.cannot_fit?.drive).toBe(0);
		expect(mm.actions.proceed.enabled).toBe(false);
		expect(await mm.proceed()).toBe(false);
		mm.acknowledgement = 'Checked the amplifier: it has the room.';
		expect(await mm.override()).toBe(true);
		expect(mm.status?.override?.text).toBe('Checked the amplifier: it has the room.');
		await mm.abort();
		expect(mm.status?.end?.outcome).toBe('operator_abort');
	}, 30000);

	it('shares the run slot: another test type cannot arm while this run is live', async () => {
		const { mm, client } = await screen();
		await applied(mm);
		await mm.start();
		await until(() => mm.status?.state === 'review');
		await expect(client.request('mimo_sine.arm')).rejects.toBeTruthy();
		await mm.abort();
	}, 30000);
});
