/**
 * The TWR store over the real client and codec, against the mock control service: the law catalog
 * of the TWR world, a reference read from pasted text and from a file by its path, the service's
 * preparation of it, the dry run of the draft, a run on the automatic ladder from identification
 * through the review hold to the end of playback, the manual ladder's verbs and each reason they are
 * refused, the coherence hold and its only release, a predicted drive over the ceiling, Stop and
 * Abort, a law that misses, one play's records, the run slot shared with the other test types, and
 * a window's subscription. The mock refuses as the service does, and the store reads its numbers
 * from whichever frame comes, `twr.live` or a subscribed screen's `twr.data`.
 */
import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { TWR_TOPICS, type TwrData, type TwrLive, type TwrPlaysResult } from '$lib/protocol/twr';
import type { SpectrumView } from '$lib/protocol/types';
import { SineStore } from '../sine/sineStore.svelte';
import { AcquireState } from '../store.svelte';
import { twrPagePlots, twrSubscribeParams, TWR_PAGES } from './twrLayout';
import { TwrStore } from './twrStore.svelte';
import { TwrViewsState } from './twrViews.svelte';
import { twrLevelStatus } from './twrWords';

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
	const twr = new TwrStore(acq, { local: null, checkDelayMs: 0 });
	await twr.sync();
	/** A second screen on the same service, observing. */
	const observer = async () => {
		const c = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		c.connect();
		await c.whenOpen();
		return c;
	};
	return { client, acq, twr, observer, server };
}

/** A short field-like record as pasted text: a time column and a value, `n` samples at `fs`. */
function refText(n = 2000, fs = 12800): string {
	const lines = ['time_s, accel_g'];
	for (let i = 0; i < n; i++) {
		const t = i / fs;
		lines.push(`${t.toFixed(10)}, ${(0.2 * Math.exp(-3 * t) * Math.sin(2 * Math.PI * 250 * t)).toFixed(6)}`);
	}
	return lines.join('\n');
}

async function ready(twr: TwrStore, ceiling = 1) {
	expect(await twr.importReference({ text: refText() })).toBe(true);
	twr.setup.limits.drive_vpk_ceiling = ceiling;
}

const refusal = async (p: Promise<unknown>) => (await p.catch((e: unknown) => e)) as RequestError;

async function toReview(twr: TwrStore) {
	if (twr.setupStatus !== 'applied') expect(await twr.apply()).toBe(true);
	expect(await twr.start()).toBe(true);
	await until(() => twr.status?.state === 'review');
}

describe('the TWR store', () => {
	it("reads the TWR world's laws only, and starts the law at its defaults", async () => {
		const { twr } = await screen();
		expect(twr.twrLaws.map((l) => l.id)).toEqual(['twr_correct']);
		expect(twr.law?.name).toBe('frequency-domain correction');
		expect(twr.setup.law.params).toEqual({ beta: 0.5, max_db_per_iter: 6, initial_backoff_db: 0 });
	});

	it('imports a TWR law file and shows its parameters from its manifest; another world is not chosen', async () => {
		const { twr } = await screen();
		expect(await twr.importLaw('/laws/law_fixture_twr_template.wasm')).toBe(true);
		expect(twr.twrLaws.map((l) => l.name)).toEqual(['frequency-domain correction', 'TWR template']);
		expect(twr.law?.name).toBe('TWR template');
		expect(Object.keys(twr.setup.law.params).sort()).toEqual(['beta', 'correct', 'model_error_db']);
		expect(await twr.importLaw('/laws/sine_template.wasm')).toBe(true);
		expect(twr.law?.name).toBe('TWR template');
		expect(twr.acq.note?.text).toMatch(/not a TWR law/);
	});

	it('reads a reference from pasted text and from a file by its path; the draft names it by its SHA-256', async () => {
		const { twr } = await screen();
		expect(await twr.importReference({ text: refText(2000) })).toBe(true);
		const pasted = twr.setup.facts!;
		expect([pasted.format, pasted.samples, pasted.fs_hz, pasted.units, pasted.path, pasted.name]).toEqual(['text', 2000, 12800, 'g', null, 'pasted']);
		expect(twr.setup.reference).toEqual({ sha256: pasted.sha256, path: null });
		expect(pasted.sha256).toMatch(/^[0-9a-f]{64}$/);
		expect(twr.imported?.views.map((v) => v.title)).toEqual(['Reference']);
		expect(await twr.importReference({ path: '/data/references/r1.csv' })).toBe(true);
		expect(twr.setup.reference).toEqual({ sha256: twr.setup.facts!.sha256, path: '/data/references/r1.csv' });
		expect([twr.setup.facts!.format, twr.setup.facts!.name]).toEqual(['csv', 'r1']);
		expect(twr.setup.facts!.sha256).not.toBe(pasted.sha256);
	});

	it('refuses a reference it cannot read by its reason, and a relative path as a bad parameter', async () => {
		const { twr } = await screen();
		expect(await twr.importReference({ text: 'time, g\n0, 0\n0.0001, x\n' })).toBe(false);
		expect([twr.refusal?.reason, twr.refusal?.title, twr.refusal?.facts]).toEqual(['reference_unreadable', 'The Reference Could Not Be Read', ['Line 3']]);
		expect(await twr.importReference({ text: '0.1\n0.2\n0.3\n' })).toBe(false);
		expect(twr.refusal?.message).toMatch(/give the rate/);
		expect(await twr.importReference({ text: '0.1\n0.2\n0.3\n' }, { fs_hz: 1000 })).toBe(true);
		expect(await twr.importReference({ path: '/data/missing.csv' })).toBe(false);
		expect(twr.refusal?.reason).toBe('reference_unreadable');
		expect(await twr.importReference({ path: 'r1.csv' })).toBe(false);
		expect([twr.refusal?.reason, twr.refusal?.detail?.param]).toEqual(['bad_params', 'path']);
	});

	it("prepares the reference: the record's grid, the section, its coverage and kinematics, four views", async () => {
		const { twr } = await screen();
		await ready(twr);
		await twr.refreshPreview();
		const p = twr.prepared!;
		expect(p.derived.grid).toMatchObject({ fs_hz: 12800, record_len: 4096, section_start: 512, section_len: 2000, actual: false });
		expect(p.derived.grid.record_s).toBeCloseTo(0.32, 9);
		expect(p.views.map((v) => v.title)).toEqual(['Section', 'Spectrum', 'Velocity', 'Displacement']);
		expect(p.derived.feasibility.verdict).toBe('not_evaluated');
		twr.setup.feasibility.disp_pkpk_in = 1e-9;
		await twr.refreshPreview();
		expect(twr.prepared!.derived.feasibility.verdict).toBe('exceeded');
		expect(twr.prepared!.derived.feasibility.checks.map((c) => [c.name, c.pass])).toEqual([['disp_pkpk_in', false]]);
	});

	it("checks the draft in a dry run, its findings on the fields and sections they name", async () => {
		const { twr } = await screen();
		await ready(twr);
		const clean = await twr.checkSetup();
		expect(clean?.errors).toEqual([]);
		expect(clean?.derived?.plays_target).toBe(1);
		expect(clean?.derived?.call_budget_s).toBeCloseTo(1, 9);
		twr.setup.control.max_peak_rise_db = 6.5;
		twr.setup.correction.band_hz = [10, 2000];
		await twr.checkSetup();
		expect(Object.keys(twr.fieldErrors).sort()).toEqual(['control.max_peak_rise_db', 'correction.band_hz']);
		expect(twr.counts.control.errors).toBe(2);
	});

	it('says a reference in another unit is not the control row unit', async () => {
		const { twr } = await screen();
		expect(await twr.importReference({ text: refText() }, { units: 'm/s^2' })).toBe(true);
		twr.setup.limits.drive_vpk_ceiling = 1;
		await twr.checkSetup();
		expect(twr.fieldErrors.reference?.[0]).toMatch(/m\/s\^2.*control row is in g/);
		expect(twr.counts.reference.errors).toBe(1);
	});

	it('runs the automatic ladder from identification through review to the end of playback', async () => {
		const { client, twr } = await screen();
		await ready(twr);
		await toReview(twr);
		expect(twr.status?.identification).toMatchObject({ passed: true, lag_source: 'measured' });
		expect(twrLevelStatus(twr.now).text).toBe('Holding for review');
		expect(await twr.proceed()).toBe(true);
		await until(() => twr.status?.state === 'finished', 15000);
		expect(twr.status?.end?.outcome).toBe('completed');
		// every level gated, the first after one correction; then one playback play
		const rows = (await client.request<TwrPlaysResult>('twr.plays', { since: 0 })).plays;
		expect(rows.map((p) => [p.kind, p.level_db, p.gated])).toEqual([
			['iteration', -12, false],
			['iteration', -12, true],
			['iteration', -9, true],
			['iteration', -6, true],
			['iteration', -3, true],
			['iteration', 0, true],
			['playback', 0, true]
		]);
		await until(() => twr.plays.length === rows.length);
		expect(twr.plays).toEqual(rows);
		// the seed made the first play's drive; the correction after play 1 made the second's; gated plays carry it
		expect(rows.map((p) => p.law?.used ?? null)).toEqual([true, true, null, null, null, null, null]);
		// station time counts the plays, never the service's clock
		for (let i = 1; i < rows.length; i++) expect(rows[i].at_s).toBeGreaterThan(rows[i - 1].at_s);
		expect(twr.status?.end?.at_s).toBeGreaterThan(rows.at(-1)!.at_s);
	});

	it('on the manual ladder: Iterate, Play once the level has gated, the level moves, each refusal by name', async () => {
		const { twr } = await screen();
		await ready(twr, 0.09);
		twr.setup.schedule.mode = 'manual';
		await toReview(twr);
		expect(await twr.proceed()).toBe(true);
		await until(() => twr.now?.phase === 'waiting');
		expect(twrLevelStatus(twr.now).text).toBe('Waiting for Iterate or Play at -12.0 dB');
		// Play before the level has gated
		expect(await twr.play()).toBe(false);
		expect([twr.refusal?.reason, twr.refusal?.title, twr.refusal?.facts]).toEqual(['not_converged', 'The Level Has Not Met Its Gate', ['Gate: 10% rms error']]);
		expect(await twr.iterate()).toBe(true);
		await until(() => twr.plays.length === 1 && twr.now?.phase === 'waiting');
		expect(twr.plays[0].gated).toBe(false);
		expect(twr.verbs.play.enabled).toBe(false);
		expect(await twr.iterate()).toBe(true);
		await until(() => twr.plays.length === 2 && twr.now?.phase === 'waiting');
		expect(twr.verbs.play.enabled).toBe(true);
		// the level: a step past the table's first level, a rung it does not have, a step over the ceiling
		expect(await twr.level(-1)).toBeNull();
		expect(twr.refusal?.reason).toBe('not_a_rung');
		expect(await twr.toRung(9)).toBeNull();
		expect(twr.refusal?.reason).toBe('not_a_rung');
		expect(await twr.level(1)).toBe(-11);
		expect(twr.verbs.play.enabled).toBe(false);
		expect(await twr.toRung(4)).toBeNull();
		expect([twr.refusal?.reason, twr.refusal?.title]).toEqual(['drive_ceiling', 'Over the Drive Ceiling']);
		expect(twr.refusal?.facts[0]).toMatch(/^Drive [\d.]+ V pk at 0 dB; the drive ceiling is 0.09 V pk$/);
		expect(twr.refusal?.detail).toMatchObject({ ceiling_vpk: 0.09, level_db: 0 });
		expect(twr.now?.level_db).toBe(-11);
		// gate at the new level, then playback runs its schedule
		expect(await twr.iterate()).toBe(true);
		await until(() => twr.plays.length === 3 && twr.now?.phase === 'waiting');
		expect(await twr.play()).toBe(true);
		// once playback has begun the manual verbs wait for the end
		expect(await twr.iterate()).toBe(false);
		expect(twr.refusal?.reason).toBe('wrong_state');
		await until(() => twr.status?.state === 'finished', 10000);
		expect(twr.status?.end?.outcome).toBe('completed');
		expect(twr.plays.at(-1)?.kind).toBe('playback');
	});

	it('a level move from the review hold steps from the first level, and the run proceeds at it', async () => {
		const { twr } = await screen();
		await ready(twr);
		twr.setup.schedule.mode = 'manual';
		await toReview(twr);
		expect([twr.status?.level_db, twr.status?.rung]).toEqual([null, null]);
		expect(await twr.level(1)).toBe(-11);
		expect(await twr.proceed()).toBe(true);
		await until(() => twr.now?.phase === 'waiting');
		expect([twr.now?.level_db, twr.now?.rung]).toEqual([-11, null]);
	});

	it('at the coherence hold the manual verbs are refused coherence_hold too', async () => {
		const { client, twr } = await screen();
		await ready(twr);
		twr.setup.schedule.mode = 'manual';
		twr.setup.identification.drive_vrms = 0.0005;
		await twr.apply();
		await twr.start();
		await until(() => twr.status?.state === 'held');
		for (const m of ['twr.iterate', 'twr.play']) {
			const r = await refusal(client.request(m));
			expect([r.reason, r.detail]).toEqual(['coherence_hold', { coherence_mean: 0.42, min_mean_coherence: 0.8 }]);
		}
	});

	it('on the automatic ladder a level held for its gate takes a level move, and the run goes on from the new level', async () => {
		const { twr } = await screen();
		await ready(twr);
		twr.setup.identification.review = false;
		twr.setup.schedule.max_iterations = 1;
		twr.setup.schedule.on_gate_fail = 'hold';
		await twr.apply();
		await twr.start();
		await until(() => twr.now?.phase === 'holding');
		expect(twrLevelStatus(twr.now).text).toBe('Holding at -12.0 dB: the level did not meet its gate in 1 play');
		expect(twr.verbs.levelUp.enabled).toBe(true);
		expect(await twr.level(1)).toBe(-11);
		await until(() => twr.plays.length === 2);
		expect(twr.plays[1].level_db).toBe(-11);
		await twr.abort();
	});

	it("refuses a preparation by its shape as a bad parameter, and what a configuration would be refused as config_invalid", async () => {
		const { client, twr } = await screen();
		expect((await refusal(client.request('twr.reference.preview', { reference: { sha256: 'ABC' } }))).detail?.param).toBe('reference.sha256');
		const unknown = await refusal(client.request('twr.reference.preview', { reference: { sha256: 'a'.repeat(64) } }));
		expect([unknown.code, unknown.reason]).toEqual(['bad_request', 'config_invalid']);
		expect((unknown.detail?.errors as { reason: string; field: string }[]).map((e) => [e.reason, e.field])).toEqual([['reference_unknown', 'reference.sha256']]);
		await ready(twr);
		twr.setup.prep.highpass_hz = 30;
		await twr.refreshPreview();
		expect(twr.prepared).toBeNull();
		expect(twr.previewError).toMatch(/high-pass at 30 Hz is above the correction band's low edge/);
	});

	it('logs, after every judged play, the frames the station took before it started', async () => {
		const { twr } = await screen();
		await ready(twr);
		twr.setup.identification.review = false;
		await twr.apply();
		await twr.start();
		await until(() => twr.status?.state === 'finished', 15000);
		await until(() => twr.log.filter((e) => /frames committed before start/.test(e.text)).length === twr.plays.length);
		expect(twr.log.find((e) => /^Play 0: \d+ frames committed before start\.$/.test(e.text))?.kind).toBe('note');
	});

	it('refuses the manual verbs on the automatic ladder, wrong_mode', async () => {
		const { client, twr } = await screen();
		await ready(twr);
		await toReview(twr);
		const it = await refusal(client.request('twr.iterate'));
		expect([it.reason, it.code]).toEqual(['wrong_mode', 'refused']);
		expect((await refusal(client.request('twr.level', { delta_db: 1 }))).reason).toBe('wrong_mode');
		expect((await refusal(client.request('twr.play'))).reason).toBe('wrong_mode');
		expect((await refusal(client.request('twr.level', { delta_db: 2 }))).reason).toBe('bad_params');
	});

	it('holds at the coherence floor: Proceed is refused coherence_hold, and New Run releases it', async () => {
		const { twr } = await screen();
		await ready(twr);
		twr.setup.identification.drive_vrms = 0.0005;
		expect(await twr.apply()).toBe(true);
		expect(await twr.start()).toBe(true);
		await until(() => twr.status?.state === 'held');
		expect(twr.isLive).toBe(true);
		expect(twr.locked).toBe(true);
		expect(twr.primary).toBe('reset');
		expect(twrLevelStatus(twr.now).text).toBe('Holding: the identification did not reach its coherence floor');
		expect(await twr.proceed()).toBe(false);
		expect([twr.refusal?.reason, twr.refusal?.title]).toEqual(['coherence_hold', 'Held at the Coherence Floor']);
		expect(twr.refusal?.facts[0]).toMatch(/^Mean coherence 0.42, under 0.8$/);
		// the measured mean, and no coverage at the hold
		expect([twr.status?.identification?.coherence_mean, twr.status?.identification?.coverage]).toEqual([0.42, null]);
		const was = twr.status?.run_id;
		expect(await twr.reset()).toBe(true);
		expect(twr.status?.state).toBe('configured');
		expect(twr.status?.run_id).not.toBe(was);
		expect(twr.setupStatus).toBe('applied');
	});

	it('ends an automatic run before any play when the full-level drive is predicted over the ceiling', async () => {
		const { twr } = await screen();
		await ready(twr, 0.05);
		twr.setup.identification.review = false;
		await twr.apply();
		await twr.start();
		await until(() => twr.status?.state === 'finished');
		expect(twr.status?.end?.trip).toMatchObject({ kind: 'drive_vpk', predicted: true, ceiling_vpk: 0.05, play: null });
		expect(twr.status?.end?.outcome).toBe('aborted');
		expect(twr.plays).toEqual([]);
	});

	it('stops a play in flight at the next block and ends operator_stop with the frames done; an observer may abort but not iterate', async () => {
		const { twr, observer } = await screen({ speed: 4 });
		expect(await twr.importReference({ path: '/data/r1.csv' })).toBe(true);
		twr.setup.limits.drive_vpk_ceiling = 1;
		await toReview(twr);
		await twr.proceed();
		await until(() => (twr.now?.play?.frames_done ?? 0) > 0, 15000);
		const frames = twr.now!.play!;
		// the record and the rest pad behind it
		expect(frames.frames).toBe(65536 + 1024);
		expect(twr.verbs.iterate.visible).toBe(false);
		expect(await twr.stop()).toBe(true);
		await until(() => twr.status?.state === 'finished', 10000);
		expect(twr.status?.end).toMatchObject({ outcome: 'operator_stop', play: 0 });
		expect(twr.status?.end?.frames_done).toBeGreaterThan(0);

		await twr.reset();
		twr.setup.schedule.mode = 'manual';
		await twr.apply();
		await toReview(twr);
		const b = await observer();
		expect((await refusal(b.request('twr.proceed'))).reason).toBe('not_controller');
		await b.request('twr.abort');
		await until(() => twr.status?.end?.outcome === 'operator_abort');
	}, 30000);

	it('ends a law file that answers late three times in a row with law_fault, the drive in force replayed through the misses', async () => {
		const { twr } = await screen();
		await ready(twr);
		twr.setup.schedule.mode = 'manual';
		expect(await twr.importLaw('/laws/law_fixture_twr_misbehave.wasm')).toBe(true);
		await toReview(twr);
		await twr.proceed();
		for (let i = 0; i < 3; i++) {
			await until(() => twr.now?.phase === 'waiting');
			if (twr.status?.state === 'finished') break;
			await twr.iterate();
			await until(() => twr.plays.length === i + 1);
		}
		await until(() => twr.status?.state === 'finished');
		expect(twr.status?.end?.trip).toMatchObject({ kind: 'law_fault', reason: 'over_budget', consecutive: 3 });
		expect(twr.status?.end?.outcome).toBe('law_fault');
		expect(twr.plays.slice(1).map((p) => p.law?.used)).toEqual([false, false]);
	});

	it("opens one play's records; an older play's are refused not_found and its row stays", async () => {
		const { twr } = await screen();
		await ready(twr);
		twr.setup.run = { schedule: 'loop', count: 6, duration_s: 60 };
		twr.setup.identification.review = false;
		await twr.apply();
		await twr.start();
		await until(() => twr.status?.state === 'finished', 15000);
		await until(() => twr.plays.length === 12);
		await twr.toggleRecord(11);
		expect(twr.record?.index).toBe(11);
		expect(twr.record?.views.map((v) => v.title)).toEqual(['Replication', 'Error', 'Drive']);
		expect((twr.record?.views[0] as SpectrumView).series.map((s) => [s.name, s.role])).toEqual([
			['Response', 'measured'],
			['Target', 'reference']
		]);
		await twr.toggleRecord(11);
		expect(twr.record).toBeNull();
		await twr.toggleRecord(0);
		expect([twr.refusal?.code, twr.refusal?.title]).toEqual(['not_found', 'Not Held Any More']);
		expect(twr.record).toBeNull();
		expect(twr.plays[0].index).toBe(0);
	});

	it('freezes the corrections and resumes them', async () => {
		const { twr } = await screen();
		await ready(twr);
		twr.setup.schedule.mode = 'manual';
		await toReview(twr);
		expect(await twr.freeze(true)).toBe(true);
		expect(twr.now?.frozen).toBe(true);
		expect([twr.verbs.freeze.visible, twr.verbs.unfreeze.visible]).toEqual([false, true]);
		await twr.proceed();
		await until(() => twr.now?.phase === 'waiting');
		await twr.iterate();
		await until(() => twr.plays.length === 1 && twr.now?.phase === 'waiting');
		await twr.iterate();
		await until(() => twr.plays.length === 2 && twr.now?.phase === 'waiting');
		// frozen: no call behind the second play, and its error is the first's
		expect(twr.plays[1].law).toBeNull();
		expect(twr.plays[1].rms_error_pct).toBe(twr.plays[0].rms_error_pct);
		expect(await twr.freeze(false)).toBe(true);
		expect(twr.now?.frozen).toBe(false);
	});

	it('a freeze asked for during a play takes effect once that play has been corrected, and the state then says so', async () => {
		const { twr } = await screen({ speed: 4 });
		expect(await twr.importReference({ path: '/data/r1.csv' })).toBe(true);
		twr.setup.limits.drive_vpk_ceiling = 1;
		await toReview(twr);
		await twr.proceed();
		await until(() => (twr.now?.play?.frames_done ?? 0) > 0, 15000);
		expect(await twr.freeze(true)).toBe(true);
		expect(twr.status?.frozen).toBe(false);
		await until(() => twr.plays.length === 1, 15000);
		await until(() => twr.status?.frozen === true);
		// the play in flight was corrected as it would have been: the next play has a call behind it
		await until(() => twr.plays.length === 2, 15000);
		expect(twr.plays[1].law?.used).toBe(true);
		await twr.abort();
	}, 40000);

	it('shares the run slot: a live TWR run holds off Sine, and is refused while another run is live', async () => {
		const { acq, twr } = await screen();
		await ready(twr);
		await toReview(twr);
		const sine = new SineStore(acq, { local: null, checkDelayMs: 0 });
		await sine.sync();
		sine.setup.limits.drive_vpk_ceiling = 0.5;
		sine.setup.limits.drive_vpk_abort = 0.8;
		expect(await sine.apply()).toBe(true);
		expect(await sine.start()).toBe(false);
		expect(sine.refusal?.reason).toBe('run_active');
		await twr.abort();
		await until(() => twr.status?.state === 'finished');
		expect(await sine.start()).toBe(true);
		expect(await twr.reset()).toBe(true);
		expect(await twr.start()).toBe(false);
		expect(twr.refusal?.reason).toBe('run_active');
		await sine.abort();
	});
});

describe("the TWR mock's subscriptions, as the service takes them", () => {
	it('refuses a key a quantity does not take: any channel selector, a point count off the record plots', async () => {
		const { client } = await screen();
		const sub = (plots: unknown[]) => refusal(client.request('twr.subscribe', { rate_hz: 5, plots }));
		expect((await sub([{ id: 'a', quantity: 'response', channels: 'all' }])).detail?.param).toBe('plots[0].channels');
		expect((await sub([{ id: 'a', quantity: 'frf', points: 512 }])).detail?.param).toBe('plots[0].points');
		expect((await sub([{ id: 'a', quantity: 'pulse' }])).detail?.param).toBe('plots[0].quantity');
		expect((await sub([{ id: 'a', quantity: 'response', points: 32 }])).detail?.param).toBe('plots[0].points');
		const ok = (await client.request('twr.subscribe', { plots: [{ id: 'a', quantity: 'response' }] })) as { plots: { points?: number }[]; rate_hz: number };
		expect([ok.rate_hz, ok.plots[0].points]).toEqual([5, 1024]);
	});

	it("every page the shell offers is a subscription the mock accepts", async () => {
		const { client } = await screen();
		for (const kind of TWR_PAGES.kinds) {
			const params = twrSubscribeParams(twrPagePlots({ id: 'p', name: kind, kind }), 5);
			await expect(client.request('twr.subscribe', params)).resolves.toMatchObject({ default: false });
		}
	});

	it('sends a subscribed screen its own data frame after identification and after each play, with views for its plots', async () => {
		const { client, twr } = await screen();
		await ready(twr);
		const frames: TwrData[] = [];
		client.on(TWR_TOPICS.data, (d) => frames.push(d as unknown as TwrData));
		await client.request('twr.subscribe', { rate_hz: 10, plots: [{ id: 'r', quantity: 'response', points: 256 }, { id: 'f', quantity: 'frf' }, { id: 'c', quantity: 'convergence' }] });
		await toReview(twr);
		await until(() => frames.some((f) => f.plots.find((p) => p.id === 'f')!.views.length === 2));
		const id = frames.find((f) => f.plots.find((p) => p.id === 'f')!.views.length === 2)!;
		expect(id.plots.find((p) => p.id === 'f')!.views.map((v) => v.title)).toEqual(['Identification Gain', 'Identification Phase']);
		expect(id.plots.find((p) => p.id === 'r')!.views).toEqual([]);
		await twr.proceed();
		await until(() => twr.status?.state === 'finished', 15000);
		await until(() => frames.at(-1)?.state === 'finished');
		const last = frames.at(-1)!;
		expect(last.plots.find((p) => p.id === 'r')!.views.map((v) => v.title)).toEqual(['Response']);
		expect((last.plots.find((p) => p.id === 'c')!.views[0] as SpectrumView).series[0].x.length).toBe(7);
		expect(Object.keys(last).sort()).toEqual(['last', 'level_db', 'mode', 'phase', 'play', 'plays', 'plots', 'rung', 'run_id', 'source', 'state', 't_s'].sort());
	});

	it("reads the run's numbers from a subscribed screen's data frames, which come in place of the live frame, at its rate through a play", async () => {
		const { client, twr } = await screen({ speed: 4 });
		let live = 0;
		client.on(TWR_TOPICS.live, () => live++);
		const views = new TwrViewsState(client, { setupTitle: () => null, showing: () => true, storage: null, session: null, events: null, subscribeDelayMs: 0 });
		const stop = views.watch();
		expect(await twr.importReference({ path: '/data/r1.csv' })).toBe(true);
		twr.setup.limits.drive_vpk_ceiling = 1;
		await toReview(twr);
		await twr.proceed();
		// the default page plots no scope: the data frames come at the window's rate through the play all the same
		await until(() => (twr.now?.play?.frames_done ?? 0) > 0, 15000);
		const before = twr.now!.play!.frames_done;
		await until(() => (twr.now?.play?.frames_done ?? 0) > before, 5000);
		expect(live).toBe(0);
		expect(twr.frameOfRun?.plots).toBeDefined();
		await twr.abort();
		stop();
	}, 30000);

	it('sends twr.live to every connection without a subscription, the play in flight and the Scope view while it plays', async () => {
		const { twr, observer } = await screen({ speed: 4 });
		const b = await observer();
		const frames: TwrLive[] = [];
		b.on(TWR_TOPICS.live, (d) => frames.push(d as unknown as TwrLive));
		expect(await twr.importReference({ path: '/data/r1.csv' })).toBe(true);
		twr.setup.limits.drive_vpk_ceiling = 1;
		await toReview(twr);
		await twr.proceed();
		await until(() => frames.some((f) => f.play && f.play.frames_done > 0), 15000);
		const f = frames.find((x) => x.play && x.play.frames_done > 0)!;
		expect(f.views.map((v) => v.title)).toEqual(['Scope']);
		expect((f.views[0] as SpectrumView).series.length).toBe(1);
		expect(f.play).toMatchObject({ index: 0, kind: 'iteration', iteration: 1, frames: 65536 + 1024 });
		await twr.abort();
	}, 30000);
});
