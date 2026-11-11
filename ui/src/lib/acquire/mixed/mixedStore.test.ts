/**
 * The Mixed Mode store over the real client and codec, against the mock control service: the two
 * law catalogs, the dry run and its derived values, a refusal placed on the row it concerns, a run
 * of each kind of test (sine-on-random, random-on-random and both) from identification through the
 * review hold, the floor alone and the parts coming in, a part switched off and on live and each
 * reason a switch is refused, a hold that stops parts time, the manual ladder, a live parameter of
 * the tone law, stop and abort, a trip that names its part, replies and frames of another run left
 * unapplied, and a window's subscription for every page and every plot type.
 */
import { describe, expect, it, vi } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { MixedData, MixedState } from '$lib/protocol/mixed';
import { AcquireState } from '../store.svelte';
import { defaultMixedScreen, MIXED_PLOT_TYPES, mixedSubscription } from './mixedLayout';
import { newBand, newTone } from './mixedSetup';
import { MixedStore } from './mixedStore.svelte';
import { MixedViewsState } from './mixedViews.svelte';
import { partMotion } from './mixedWords';
import type { PlotPanel } from '../screen/screen.svelte';

vi.setConfig({ testTimeout: 30000 });

const until = async (cond: () => boolean, ms = 10000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

async function screen() {
	const server = createControlServer({ speed: 20 });
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	const mixed = new MixedStore(acq, { local: null, checkDelayMs: 0 });
	await mixed.sync();
	mixed.setup.limits.drive_vpk_ceiling = 4;
	mixed.setup.limits.drive_vpk_abort = 6;
	// the manual ladder: the run goes on until it is stopped, at the mock's 20 run seconds a second
	mixed.setup.schedule.mode = 'manual';
	mixed.setup.schedule.levels = [{ db: -6, dwell_s: 2 }, { db: 0, dwell_s: 0 }];
	mixed.setup.schedule.full_level_s = 4;
	return { client, acq, mixed };
}

/** The combined test: a fixed tone, a swept tone with two followers, a fixed and a swept band. */
function combined(mixed: MixedStore) {
	const d = mixed.setup;
	d.tones = [newTone('t1', 1, 120), newTone('t2', 2, 300), newTone('t3', 3), newTone('t4', 4)];
	d.tones[1].schedule.kind = 'sweep';
	d.tones[1].schedule.band_hz = [300, 600];
	d.tones[1].schedule.oct_per_min = 6;
	for (const [i, r] of [[2, 2], [3, 3]] as const) {
		d.tones[i].schedule.kind = 'follow';
		d.tones[i].schedule.of = 't2';
		d.tones[i].schedule.ratio = r;
		d.tones[i].amplitude.g = 0.5;
	}
	d.bands = [newBand('b1', 1, 250), newBand('b2', 2, 1300)];
	d.bands[1].schedule.kind = 'sweep';
	d.bands[1].schedule.band_hz = [1300, 1600];
	d.bands[1].schedule.law = 'linear';
	d.bands[1].schedule.hz_per_s = 20;
	d.bands[1].width = { by: 'share', hz: 20, share: 0.05 };
}

async function toParts(mixed: MixedStore) {
	expect(await mixed.apply()).toBe(true);
	expect(await mixed.start()).toBe(true);
	await until(() => mixed.status?.state === 'review');
	expect(await mixed.proceed()).toBe(true);
	await until(() => mixed.status?.phase === 'floor_only');
	await until(() => (mixed.now?.phase === 'engaged' || mixed.now?.phase === 'full_level') && mixed.parts.filter((p) => p.kind !== 'floor').every((p) => p.on && p.scale >= 0.999));
}

describe('the setup against the service', () => {
	it('reads the floor laws from the single world and the tone laws from the sine world', async () => {
		const { mixed } = await screen();
		expect(mixed.floorLaws.map((l) => l.id)).toEqual(['hinv']);
		expect(mixed.toneLaws.map((l) => l.id)).toContain('servo_blend');
		expect(mixed.floorLaw?.id).toBe('hinv');
		expect(mixed.toneLaw?.id).toBe('servo_blend');
	});

	it("starts the built-in tone law from Mixed Mode's settings, not Sine's catalog defaults", async () => {
		const { mixed } = await screen();
		// Sine's slow compression lets a sweeping tone fall behind its target and trip its abort
		expect(mixed.setup.lawTones.params).toMatchObject({ compression: 'custom', ki: 0.5, max_rate_db_per_s: 20 });
		// what the operator chose is kept
		mixed.setup.lawTones.params = { ...mixed.setup.lawTones.params, compression: 'slow' };
		mixed.chooseLaw('tones', 'servo_blend');
		expect(mixed.setup.lawTones.params.compression).toBe('slow');
	});

	it("the dry run gives the service's derived values for the draft", async () => {
		const { mixed } = await screen();
		combined(mixed);
		await mixed.checkSetup();
		const d = mixed.derived!;
		expect(d.parts.map((p) => p.id)).toEqual(['floor', 't1', 't2', 't3', 't4', 'b1', 'b2']);
		expect(d.parts.find((p) => p.id === 't3')?.freq_hz).toEqual([600, 1200]);
		expect(d.closest_tones).not.toBeNull();
		expect(d.reference.total_ms_g2).toBeGreaterThan(d.reference.floor_ms_g2);
		expect(mixed.check?.views.map((v) => v.title)).toEqual(['Reference', 'Tones']);
	});

	it('a refusal of the service lands on the row and the field it concerns', async () => {
		const { mixed } = await screen();
		mixed.setup.tones.push(newTone('t2', 2, 120.3));
		await mixed.checkSetup();
		expect(mixed.fieldErrors['tones.1.row']?.[0]).toMatch(/at least 1 Hz apart/);
		expect(mixed.counts.parts.errors).toBeGreaterThan(0);
		mixed.setup.tones[1].schedule.freq_hz = 3000;
		await mixed.checkSetup();
		expect(mixed.fieldErrors['tones.1.row']?.[0]).toMatch(/outside the floor's band/);
	});
});

describe('a run of each kind', () => {
	it('the combined test: floor alone first, then every part; a part off and on; hold; manual level; adjust; stop', async () => {
		const { client, mixed } = await screen();
		combined(mixed);
		await toParts(mixed);
		const t3 = () => mixed.parts.find((p) => p.id === 't3')!;
		const t2 = () => mixed.parts.find((p) => p.id === 't2')!;
		// a follower stays at its ratio of its leader
		expect(t3().freq_hz! / t2().freq_hz!).toBeCloseTo(2, 1);
		// switch a part off and back on: a ramp each way
		expect(await mixed.part('b1', false)).toEqual({ id: 'b1', on: false });
		await until(() => !mixed.parts.find((p) => p.id === 'b1')!.on);
		expect(partMotion(mixed.parts.find((p) => p.id === 'b1')!, false)).toBe('Off');
		expect(await mixed.part('b1', true)).toEqual({ id: 'b1', on: true });
		await until(() => mixed.parts.find((p) => p.id === 'b1')!.on);
		// the floor cannot be switched off, and the refusal sits on its row in words
		expect(await mixed.part('floor', false)).toBeNull();
		expect(mixed.partRefusal?.id).toBe('floor');
		expect(mixed.partRefusal?.message).toMatch(/whole run/);
		// hold stops parts time
		expect(await mixed.hold()).toBe(true);
		await until(() => !!mixed.frameOfRun?.held);
		const at = mixed.now!.parts_s!;
		await new Promise((r) => setTimeout(r, 150));
		expect(mixed.now!.parts_s).toBe(at);
		expect(await mixed.resume()).toBe(true);
		await until(() => (mixed.now?.parts_s ?? 0) > at);
		// manual ladder and the tone law's live parameter
		expect(await mixed.setMode('manual')).toBe(true);
		expect(await mixed.level(3)).toBe(-3);
		expect(mixed.liveParams.find((p) => p.key === 'blend_w')?.enabled).toBe(true);
		expect(await mixed.adjust('blend_w', 0.7)).toEqual({ key: 'blend_w', value: 0.7 });
		expect((await client.request<MixedState>('mixed.state')).law?.tones.live.blend_w).toBe(0.7);
		expect(await mixed.adjust('kp', 0.4)).toBeNull();
		expect(mixed.refusal?.reason).toBe('adjust_refused');
		// stop: every part ramps down together
		expect(await mixed.stop()).toBe(true);
		await until(() => mixed.status?.state === 'finished');
		expect(mixed.status?.end?.outcome).toBe('operator_stop');
	});

	it('random-on-random: the bands come in and are judged in g²/Hz', async () => {
		const { mixed } = await screen();
		combined(mixed);
		mixed.setup.tones = [];
		await toParts(mixed);
		const b2 = mixed.parts.find((p) => p.id === 'b2')!;
		expect(b2.unit).toBe('g²/Hz');
		expect(b2.width_hz).toBeGreaterThan(0);
		expect(b2.direction).not.toBeNull();
		expect(await mixed.abort()).toBe(true);
		expect(mixed.status?.end?.outcome).toBe('operator_abort');
	});

	it('sine-on-random: a tone that trips names itself in the run-end', async () => {
		const { mixed } = await screen();
		mixed.setup.title = 'Sine on random, abort tone';
		await toParts(mixed).catch(() => undefined);
		await until(() => mixed.status?.state === 'finished');
		const end = mixed.status!.end!;
		expect(end.outcome).toBe('aborted');
		expect(end.trip?.part).toBe('t1');
		expect(end.message).toMatch(/^Tone 1 abort/);
	});
});

describe('replies and frames of another run', () => {
	it('a state older than the run having finished is not applied', async () => {
		const { mixed, client } = await screen();
		await toParts(mixed);
		await mixed.abort();
		const done = mixed.status!;
		(mixed as unknown as { onStatus(s: MixedState): void }).onStatus({ ...done, state: 'running', phase: 'engaged', end: null });
		expect(mixed.status?.state).toBe('finished');
	});

	it("a window draws no frame of another run than the store's", async () => {
		const { client, mixed } = await screen();
		expect(await mixed.apply()).toBe(true);
		const views = new MixedViewsState(client, { setupTitle: () => null, showing: () => true, storage: null, session: null, events: null, runId: () => mixed.status?.run_id ?? null });
		views.data = { run_id: 'mixed-9999', plots: [{ id: 'pl-1', quantity: 'control', channels: null, parts: null, views: [{ view: 'reading', title: 'x', rows: [] }] }] } as unknown as MixedData;
		expect(views.frame('pl-1')).toBeNull();
		expect(views.views('pl-1')).toEqual([]);
		views.data = { ...views.data!, run_id: mixed.status!.run_id! };
		expect(views.views('pl-1')).toHaveLength(1);
	});
});

describe("a window's subscription, as the mock takes it", () => {
	it('every default page, and every plot type on its own, is accepted, and per-part plots resolve their parts', async () => {
		const { client, mixed } = await screen();
		combined(mixed);
		await toParts(mixed);
		const layout = defaultMixedScreen();
		for (const page of layout.pages) {
			const plots = page.panels.filter((p): p is PlotPanel => p.kind === 'plot');
			await client.request('mixed.subscribe', mixedSubscription(plots, 5));
		}
		for (const t of MIXED_PLOT_TYPES) {
			const panel = { id: 'p', kind: 'plot', at: { x: 0, y: 0, w: 1, h: 1 }, plot: { quantity: t.id, channels: 'all', parts: t.parts ? 'all' : undefined, arrange: 'split', points: 512, title: null, display: {} } } as unknown as PlotPanel;
			const res = await client.request<{ plots: { quantity: string }[] }>('mixed.subscribe', mixedSubscription([panel], 5));
			expect(res.plots[0].quantity).toBe(t.id);
		}
		const tones = layout.pages.find((p) => p.name === 'Tones')!.panels.filter((p): p is PlotPanel => p.kind === 'plot');
		let frame: MixedData | null = null;
		client.on('mixed.data', (d) => (frame = d as unknown as MixedData));
		await client.request('mixed.subscribe', mixedSubscription(tones, 10));
		await until(() => !!frame && frame.plots[0]?.views.length === 4);
		expect(frame!.plots[0].parts).toEqual(['t1', 't2', 't3', 't4']);
		await mixed.abort();
	});

	it('the mock refuses a part choice on a plot that takes none, and channels on a per-part plot', async () => {
		const { client } = await screen();
		await expect(client.request('mixed.subscribe', { plots: [{ id: 'a', quantity: 'control', parts: 'all' }] })).rejects.toMatchObject({ reason: 'bad_params' });
		await expect(client.request('mixed.subscribe', { plots: [{ id: 'a', quantity: 'amplitude', channels: 'all' }] })).rejects.toMatchObject({ reason: 'bad_params' });
	});
});
