/**
 * The MIMO Mixed Mode store over the real client and codec, against the mock control service: the
 * two law catalogs read from their own worlds, the refusals by name at their field, the reference
 * built from the breakpoints, and a session from identification through the review, the floor
 * alone, the tones in, a drive running out of room and the tones limiting, to completion, with
 * hold, resume, a part switched off and a stop on the way; the two review holds and their
 * overrides, a forecast no override passes, the mode, the level by hand, and what the verbs send.
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

/** Every `mimo_mixed.*` verb the store sends from here on, with its parameters. */
function spy(client: ServiceClient, only: (method: string) => boolean) {
	const sent: [string, Record<string, unknown> | undefined][] = [];
	const request = client.request.bind(client);
	client.request = (<R>(method: string, params?: Record<string, unknown>) => {
		if (only(method)) sent.push([method, params]);
		return request<R>(method, params);
	}) as typeof client.request;
	return sent;
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
		expect(mm.check?.derived?.reference_rms).toHaveLength(4);
		expect(mm.check?.derived?.law.floor.world).toBe('mimo');
		expect(mm.check?.derived?.law.tones.world).toBe('sine_mimo');
	});

	it("names the controller's refusal by name at the configuration: a band that would take from the floor", async () => {
		const { mm } = await screen();
		expect(mm.addBand()).toBe(true);
		const b = mm.setup.bands[0];
		b.statement.kind = 'raise';
		b.statement.db[2] = -3;
		await mm.checkSetup();
		const item = mm.check?.errors.find((e) => e.reason === 'band_not_additive');
		expect(item?.field).toBe('config');
		expect(mm.findings.errors.some((e) => e.reason === 'band_not_additive')).toBe(true);
		expect(mm.counts.parts.errors).toBeGreaterThan(0);
	});

	it("refuses a tone out of the floor's band, by name", async () => {
		const { mm } = await screen();
		mm.setup.tones[0].schedule.freq_hz = 5000;
		await mm.checkSetup();
		const errs = mm.check?.errors ?? [];
		const local = Object.keys(mm.fieldErrors);
		expect(errs.some((e) => e.reason === 'part_out_of_band') || local.some((k) => k.startsWith('tones.0'))).toBe(true);
	});

	it('refuses a drive the table does not have at its row, as the service does', async () => {
		const { mm, client } = await screen();
		await mm.checkSetup();
		const config = JSON.parse(JSON.stringify(mm.wire));
		config.drives[1].channel_index = 12;
		const r = await client.request<{ errors: unknown[] }>('mimo_mixed.configure', { config, dry_run: true });
		expect(r.errors).toEqual([{ reason: 'channel_unknown', field: 'drives[1]', message: 'Channel 12 is not an enabled drive of the table.' }]);
		expect(mm.wire.drives.map((d) => 'abort_vpk' in d)).toEqual([false, false]);
	});

	it('runs a session: identify, review, the floor equalizes, the tones come in, a drive runs out of room, completed', async () => {
		const { mm } = await screen();
		await applied(mm);
		expect(mm.status?.drives.map((d) => d.abort_vpk)).toEqual([4.488, 4.488]);
		expect(await mm.start()).toBe(true);
		await until(() => mm.status?.state === 'review');
		expect(mm.status?.phase).toBe('awaiting_review');
		expect(mm.status?.identification?.passed).toBe(true);
		expect(mm.status?.prediction?.drives.map((d) => d.drive)).toEqual([5, 6]);
		expect(mm.status?.prediction?.forecast.cannot_fit).toBeNull();
		expect(mm.holdInForce).toBeNull();
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
		expect(mm.runDriveLabels).toEqual(['drive 5', 'drive 6']);
		await until(() => mm.status?.state === 'finished', 20000);
		expect(mm.status?.end?.outcome).toBe('completed');
		expect(mm.status?.end?.name).toBe('completed');
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
		expect(tones.map((t) => (t.scale ?? 1) < 1)).toEqual([false, true]);
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
		expect(mm.refusal?.detail).toEqual({ id: 'floor', why: 'floor' });
		expect(mm.refusal?.title).toBe('The Part Cannot Be Switched');
		expect(await mm.part('t1', false)).toBe(true);
		await until(() => mm.status?.parts.find((p) => p.id === 't1')?.enabled === false);
		expect(await mm.stop()).toBe(true);
		await until(() => mm.status?.state === 'finished');
		expect(mm.status?.end?.outcome).toBe('operator_stop');
	}, 30000);

	it('holds on a prediction over a ceiling, refuses the proceed, and engages on an override that names the hold', async () => {
		const { mm, client } = await screen();
		for (const d of mm.setup.drives) {
			d.ceiling_vrms = 1;
			d.peak_v = 1.2;
		}
		await applied(mm);
		await mm.start();
		await until(() => mm.status?.state === 'review');
		expect(mm.status?.phase).toBe('prediction_exceeded');
		expect(mm.status?.prediction?.within).toBe(false);
		expect(mm.status?.prediction?.forecast.cannot_fit).toBeNull();
		expect(mm.holdInForce).toBe('prediction_exceeded');
		expect(mm.actions.proceed.enabled).toBe(false);
		expect(mm.verbs.override.enabled).toBe(false);
		expect(await mm.proceed()).toBe(false);
		expect(mm.refusal?.reason).toBe('prediction_exceeded');
		// an override that names the other hold is refused, with the hold in force
		mm.acknowledgement = 'Checked the amplifier: it has the room.';
		expect(await mm.override('gate_failed')).toBe(false);
		expect([mm.refusal?.reason, mm.refusal?.detail]).toEqual(['override_not_needed', { holds: 'prediction_exceeded' }]);
		const sent = spy(client, (m) => m === 'mimo_mixed.override');
		expect(mm.verbs.override.enabled).toBe(true);
		expect(await mm.override()).toBe(true);
		expect(sent).toEqual([['mimo_mixed.override', { ack: 'Checked the amplifier: it has the room.', reason: 'prediction_exceeded' }]]);
		expect(mm.status?.override).toMatchObject({ text: 'Checked the amplifier: it has the room.', reason: 'prediction_exceeded' });
		expect(mm.status?.phase).toBe('floor_only');
		await mm.abort();
		expect(mm.status?.end?.outcome).toBe('operator_abort');
	}, 30000);

	it('holds on a failed coherence gate; an override goes on to the prediction and engages', async () => {
		const { mm } = await screen();
		mm.setup.gates.coherence_threshold = 0.95;
		await applied(mm);
		await mm.start();
		await until(() => mm.status?.state === 'review');
		expect(mm.status?.phase).toBe('gate_failed');
		expect(mm.status?.identification?.passed).toBe(false);
		expect(mm.status?.identification?.gate_coherence).toBe(0.95);
		expect(await mm.proceed()).toBe(false);
		expect(mm.refusal?.reason).toBe('gate_failed');
		expect(await mm.override()).toBe(false);
		expect(mm.refusal?.reason).toBe('bad_params');
		mm.acknowledgement = 'The fixture is as designed; the coherence is low at the ends of the band.';
		expect(await mm.override()).toBe(true);
		expect(mm.status?.overrides.map((o) => o.reason)).toEqual(['gate_failed']);
		expect(mm.status?.phase).toBe('floor_only');
		await mm.abort();
	}, 30000);

	it('refuses to engage on a forecast that cannot fit, and no override passes it', async () => {
		const { mm } = await screen();
		for (const d of mm.setup.drives) {
			d.ceiling_vrms = 0.4;
			d.peak_v = 0.5;
		}
		await applied(mm);
		await mm.start();
		await until(() => mm.status?.state === 'review');
		expect(mm.status?.phase).toBe('awaiting_review');
		expect(mm.status?.prediction?.forecast.refused).toBe('budget_cannot_fit');
		expect(mm.status?.prediction?.forecast.cannot_fit?.drive).toBe(5);
		expect(mm.actions.proceed.enabled).toBe(false);
		expect(mm.verbs.override.visible).toBe(false);
		expect(await mm.proceed()).toBe(false);
		expect(mm.refusal?.reason).toBe('budget_cannot_fit');
		expect(mm.refusal?.facts[0]).toMatch(/^Drive drive 5 needs /);
		mm.acknowledgement = 'Go on.';
		expect(await mm.override()).toBe(false);
		expect(mm.refusal?.reason).toBe('override_not_needed');
		await mm.stop();
		expect(mm.status?.end?.outcome).toBe('operator_stop');
	}, 30000);

	it('switches the mode, and moves the level by hand only in manual and only to a rung', async () => {
		const { mm } = await screen();
		await applied(mm);
		expect(mm.actions.mode.enabled).toBe(true);
		expect(await mm.setMode('manual')).toBe(true);
		expect(mm.status?.mode).toBe('manual');
		await mm.start();
		await until(() => mm.status?.state === 'review');
		await mm.proceed();
		expect(await mm.goTo(-3)).toBeNull();
		expect([mm.refusal?.reason, mm.refusal?.detail]).toEqual(['not_a_rung', { levels_db: [-12, -6, 0] }]);
		expect(await mm.goTo(-6)).toBe(-6);
		expect(await mm.setMode('auto')).toBe(true);
		expect(await mm.level(1)).toBeNull();
		expect([mm.refusal?.reason, mm.refusal?.detail]).toEqual(['manual_mode_required', { mode: 'auto' }]);
		await mm.abort();
	}, 30000);

	it('sends the arm, the start and the proceed with no parameters', async () => {
		const { mm, client } = await screen();
		await applied(mm);
		const sent = spy(client, (m) => ['mimo_mixed.arm', 'mimo_mixed.start', 'mimo_mixed.proceed'].includes(m));
		expect(await mm.arm()).toBe(true);
		await mm.start();
		await until(() => mm.status?.state === 'review');
		mm.acknowledgement = 'Some words.';
		expect(await mm.proceed()).toBe(true);
		expect(sent).toEqual([
			['mimo_mixed.arm', {}],
			['mimo_mixed.start', {}],
			['mimo_mixed.proceed', {}]
		]);
		await mm.abort();
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
