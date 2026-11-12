/**
 * The MIMO Shock store over the real client and codec, against the mock control service: the law
 * catalog of the shock_mimo world as listed, filtered by target kind; the setup applied; a run from
 * identification through the review to completion; the unreachable hold and its acknowledgement;
 * the guards' stops in plain words; the scaled abort switch, its lock and its standing notice; the
 * absolute line; refusals; and the run slot shared with the other test types.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ChannelRow } from '$lib/protocol/types';
import { AcquireState } from '../store.svelte';
import { lawAccepts, MimoShockStore } from './msStore.svelte';
import { abortLineOf, endWords, ladderRungs, scaledCrossingWords } from './msWords';

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

async function screen(options: ControlServerOptions = {}, controls = 4, drives = 4) {
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 40, ...options }), 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	acq.rows = [...Array.from({ length: controls }, (_, i) => row(i + 1, 'control')), ...Array.from({ length: drives }, (_, i) => row(controls + i + 1, 'drive'))];
	expect(await acq.applyTable()).toBe(true);
	const ms = new MimoShockStore(acq, { local: null, checkDelayMs: 0 });
	await ms.sync();
	for (const x of ms.setup.drives) {
		x.ceiling_vpk = 4;
		x.abort_vpk = 6;
	}
	return { client, acq, ms };
}

async function toReview(ms: MimoShockStore) {
	expect(await ms.apply()).toBe(true);
	expect(await ms.start()).toBe(true);
	await until(() => ms.status?.state === 'review');
}

describe('the MIMO Shock store', () => {
	it("opens a new setup on the service's default law for its target kind, and follows the default when the kind changes", async () => {
		const { ms, client, acq } = await screen();
		const all = (await client.request<{ laws: { id: string; default_for?: string[] }[] }>('laws.list', { world: 'shock_mimo' })).laws;
		const defaultFor = (kind: 'waveform' | 'srs') => all.find((l) => l.default_for?.includes(kind))?.id;
		expect(defaultFor('waveform')).toBe('inverse');
		expect(defaultFor('srs')).toBe('shape');
		expect(ms.setup.law.id).toBe('inverse');
		ms.setKind('srs');
		expect(ms.setup.law.id).toBe('shape');
		ms.setKind('waveform');
		expect(ms.setup.law.id).toBe('inverse');
		// a law the operator chose that accepts the new kind stays
		ms.chooseLaw('paired');
		ms.setKind('srs');
		expect(ms.setup.law.id).toBe('paired');
		// a new SRS setup with no law yet opens on the SRS default, not the first law that accepts SRS
		const fresh = new MimoShockStore(acq, { local: null, checkDelayMs: 0 });
		fresh.setup.kind = 'srs';
		fresh.setup.law = { id: '', params: {} };
		await fresh.refreshLaws();
		expect(fresh.setup.law.id).toBe('shape');
	});

	it("lists the shock_mimo world's laws as the catalog gives them, those that accept the target kind", async () => {
		const { ms, client } = await screen();
		const all = (await client.request<{ laws: { id: string; world: string; targets?: string[] }[] }>('laws.list', { world: 'shock_mimo' })).laws;
		expect(ms.laws?.map((l) => l.id)).toEqual(all.map((l) => l.id));
		expect(ms.kindLaws.map((l) => l.id)).toEqual(all.filter((l) => !l.targets || l.targets.includes('waveform')).map((l) => l.id));
		// the draft takes the first law that accepts its kind, with that law's declared parameters
		expect(ms.setup.law.id).toBe(ms.kindLaws[0].id);
		expect(Object.keys(ms.setup.law.params).sort()).toEqual(ms.kindLaws[0].params.map((p) => p.key).sort());
		ms.setKind('srs');
		expect(ms.kindLaws.every((l) => lawAccepts(l, 'srs'))).toBe(true);
		expect(ms.kindLaws.length).toBe(all.filter((l) => !l.targets || l.targets.includes('srs')).length);
		// a law that does not accept the new kind gives way
		const waveOnly = ms.laws!.find((l) => !lawAccepts(l, 'waveform'));
		if (waveOnly) {
			ms.chooseLaw(waveOnly.id);
			ms.setKind('waveform');
			expect(ms.setup.law.id).not.toBe(waveOnly.id);
		}
	});

	it('fits the draft to the table and asks for the drive ceilings before it applies', async () => {
		const { ms } = await screen();
		expect(ms.setup.controls).toEqual([1, 2, 3, 4]);
		ms.setup.drives[0].ceiling_vpk = NaN;
		expect(ms.actions.apply.reason).toBe('Fix the error in the setup first.');
		expect(ms.counts.channels.errors).toBe(1);
		ms.setup.drives[0].ceiling_vpk = 4;
		await ms.checkSetup();
		expect(ms.check?.errors).toEqual([]);
		expect(ms.actions.apply.enabled).toBe(true);
	});

	it("a new SRS setup's targets cover the default band grid, so it checks clean as it opens", async () => {
		const { ms } = await screen();
		ms.setKind('srs');
		await ms.checkSetup();
		expect(ms.check?.errors).toEqual([]);
		expect(ms.counts.target.errors).toBe(0);
	});

	it("puts the service's dry-run findings on the setup's fields", async () => {
		const { ms } = await screen();
		ms.setup.srs.bands.f_hi_hz = 10;
		ms.setKind('srs');
		await ms.checkSetup();
		expect(ms.fieldErrors['srs.bands']).toEqual(['The band grid holds no bands.']);
		expect(ms.counts.target.errors).toBeGreaterThan(0);
	});

	it('runs from identification through the review and up the ladder to completion', async () => {
		const { ms } = await screen();
		ms.setup.simulator.plant = 'shock-linear-4x4';
		await toReview(ms);
		expect(ms.status?.identification?.passed).toBe(true);
		expect(ms.mustAcknowledge).toBe(false);
		expect(ms.primary).toBe('proceed');
		expect(await ms.proceed()).toBe(true);
		await until(() => ms.status?.state === 'finished', 20000);
		expect(ms.status?.end?.outcome).toBe('completed');
		expect(endWords(ms.status!.end!)).toMatch(/^Full level passed its pulses/);
		// every pulse row came, the verdicts improved and the arrival spread was warned about once
		expect(ms.pulses.length).toBe(ms.status!.pulses.fired);
		expect(ms.pulses.at(-1)?.passed).toBe(true);
		expect(ms.pulses.some((p) => p.warnings.some((w) => /spread/.test(w)))).toBe(true);
		expect(ms.primary).toBe('reset');
	}, 30000);

	it('holds before the first pulse when a floor is above tolerance, until the operator says why in words', async () => {
		const { ms, client } = await screen({}, 6, 3);
		await toReview(ms);
		expect(ms.mustAcknowledge).toBe(true);
		expect(ms.status?.review?.controls.filter((c) => !c.reachable_ok).length).toBeGreaterThan(0);
		expect(ms.actions.proceed.enabled).toBe(false);
		expect(ms.actions.proceed.reason).toMatch(/own words/);
		// the service refuses a proceed without words, naming the floors
		const refused = await client.request('mimo_shock.proceed', {}).then(
			() => null,
			(err: { reason?: string; detail?: { controls?: unknown[] } }) => [err.reason, err.detail?.controls?.length]
		);
		expect(refused).toEqual(['unreachable_target', 6]);
		ms.acknowledgement = 'Controls 1, 3 and 5 are known to be short here.';
		expect(ms.actions.proceed.enabled).toBe(true);
		expect(await ms.proceed()).toBe(true);
		expect(ms.status?.review?.acknowledged).toBe('Controls 1, 3 and 5 are known to be short here.');
		expect(ms.status?.state).toBe('running');
		await ms.abort();
	});

	it("a new run's pulse rows and ladder are its own, not the run before's", async () => {
		const { ms } = await screen();
		ms.setup.simulator.plant = 'shock-jointed-4x4-severe';
		ms.setup.identification.review = false;
		expect(await ms.apply()).toBe(true);
		expect(await ms.start()).toBe(true);
		await until(() => ms.status?.state === 'finished', 20000);
		const first = ms.status!.run_id;
		expect(ms.pulses.some((p) => !p.passed)).toBe(true);
		expect(await ms.reset()).toBe(true);
		ms.setup.simulator.plant = 'shock-linear-4x4';
		expect(await ms.apply()).toBe(true);
		await until(() => ms.status?.run_id !== first && ms.status?.state === 'configured');
		expect(ms.pulses).toEqual([]);
		expect(await ms.start()).toBe(true);
		await until(() => ms.status?.state === 'finished', 20000);
		expect(ms.status?.end?.outcome).toBe('completed');
		expect(ms.pulses.length).toBe(ms.status!.pulses.fired);
		expect(ms.pulses.map((p) => p.index)).toEqual(Array.from({ length: ms.status!.pulses.fired }, (_, i) => i));
		expect(ms.pulses.at(-1)?.passed).toBe(true);
		const rungs = ladderRungs(ms.status!.levels_db, ms.pulses, ms.status!.level_db);
		expect(rungs.reduce((n, r) => n + r.fired, 0)).toBe(ms.status!.pulses.fired);
	}, 40000);

	it('stops on the scaled line while scaled aborts are on, and says so in plain words', async () => {
		const { ms } = await screen();
		ms.setup.simulator.plant = 'shock-jointed-4x4-severe';
		ms.setup.identification.review = false;
		expect(await ms.apply()).toBe(true);
		expect(ms.unscaledNotice).toBeNull();
		expect(await ms.start()).toBe(true);
		await until(() => ms.status?.state === 'finished', 20000);
		const end = ms.status!.end!;
		expect([end.outcome, end.reason, abortLineOf(end)]).toEqual(['safe_stop', 'control_abort_scaled', 'scaled']);
		expect(endWords(end)).toMatch(/scaled to the level played, and scaled aborts were on/);
	}, 30000);

	it('with scaled aborts off, shows a standing notice, flags each crossing, and stops on the divergence watch', async () => {
		const { ms } = await screen();
		ms.setup.simulator.plant = 'shock-jointed-4x4-severe';
		ms.setup.identification.review = false;
		expect(ms.setScaledAborts(false)).toBe(true);
		expect(await ms.apply()).toBe(true);
		expect(ms.unscaledNotice).toMatch(/^Level-scaled aborts are off/);
		expect(await ms.start()).toBe(true);
		// the switch is locked while the run is live
		expect(ms.scaledToggleWhy).toMatch(/Locked while the run is live/);
		expect(ms.setScaledAborts(true)).toBe(false);
		expect(ms.setup.control.scaled_aborts).toBe(false);
		await until(() => ms.status?.state === 'finished', 20000);
		const flagged = ms.pulses.filter((p) => scaledCrossingWords(p));
		expect(flagged.length).toBeGreaterThan(0);
		expect(scaledCrossingWords(flagged[0])).toMatch(/^Pulse \d+ went over the scaled abort line at control 2 by [\d.]+ dB\.$/);
		// a flagged pulse is judged failing, and the ladder does not climb on it
		expect(flagged.every((p) => !p.passed)).toBe(true);
		expect(ms.status?.end?.reason).toBe('divergence');
		expect(endWords(ms.status!.end!)).toMatch(/^The divergence watch stopped the run/);
		// the notice stands while the run's word stands
		expect(ms.unscaledNotice).not.toBeNull();
		expect(ms.setScaledAborts(true)).toBe(true);
	}, 30000);

	it('ends on the absolute line whatever the switch says, and names it', async () => {
		const { ms } = await screen({ mimoShockAbsoluteAbort: true });
		ms.setup.simulator.plant = 'shock-linear-4x4';
		ms.setup.identification.review = false;
		ms.setScaledAborts(false);
		expect(await ms.apply()).toBe(true);
		expect(await ms.start()).toBe(true);
		await until(() => ms.status?.state === 'finished', 20000);
		const end = ms.status!.end!;
		expect([end.outcome, end.reason, abortLineOf(end)]).toEqual(['aborted', 'control_abort_absolute', 'absolute']);
		expect(ms.pulses.at(-1)?.controls[0].absolute_margin_db).toBeGreaterThan(0);
	}, 30000);

	it('holds a halved step with its note on a plant that predicts an over-test', async () => {
		const { ms } = await screen();
		ms.setup.simulator.plant = 'shock-jointed-4x4-moderate';
		ms.setup.identification.review = false;
		ms.setup.schedule.max_pulses_per_level = 5;
		expect(await ms.apply()).toBe(true);
		expect(await ms.start()).toBe(true);
		await until(() => ms.pulses.some((p) => p.step_note) || ms.status?.state === 'finished', 20000);
		const p = ms.pulses.find((x) => x.step_note)!;
		expect(p.step_note?.taken_db).toBeGreaterThan(p.step_note!.from_db);
		expect(p.step_note?.taken_db).toBeLessThan(p.step_note!.planned_db);
		await ms.abort();
	}, 30000);

	it('fires on Fire in the manual mode, and Fire waits with its reason', async () => {
		const { ms } = await screen();
		ms.setup.schedule.mode = 'manual';
		ms.setup.identification.review = false;
		expect(await ms.apply()).toBe(true);
		expect(ms.fireAction.visible).toBe(false);
		expect(await ms.start()).toBe(true);
		await until(() => ms.status?.phase === 'waiting');
		expect(ms.fireAction.enabled).toBe(true);
		expect(await ms.fire()).toBe(true);
		await until(() => ms.pulses.length === 1);
		await ms.stop();
		await until(() => ms.status?.state === 'finished');
		expect(ms.status?.end?.outcome).toBe('operator_stop');
	}, 30000);

	it('locks the setup while a run is live, and keeps other runs out', async () => {
		const { ms, acq } = await screen();
		expect(await ms.apply()).toBe(true);
		expect(await ms.arm()).toBe(true);
		expect(ms.locked).toBe(true);
		expect(ms.actions.apply.visible).toBe(false);
		// the channel table cannot change under the run
		expect(await acq.applyTable()).toBe(false);
		expect(acq.refusal?.reason).toBe('run_active');
		await ms.stop();
		await until(() => ms.status?.state === 'finished');
		expect(ms.locked).toBe(false);
	});
});
