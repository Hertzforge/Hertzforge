/**
 * The Modal Test store over the real client and codec, against the mock control service: the DOFs
 * set through the channel table's own edit and sent with it, the dry run with the service's check
 * and its findings, the setup applied and started, hits taken and judged by the service (clean,
 * double, overloaded) with every count read from its answers, accept, reject and undo in both
 * accept modes, the force target set from a hit, the point moves, the keys, the shaker drive to
 * its averages, the rank-one refusal, and an observer refused the operator verbs.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ChannelRow } from '$lib/protocol/types';
import { AcquireState } from '../store.svelte';
import { workspaceForRun } from '../runOpen.svelte';
import { ModalTestStore } from './modalTestStore.svelte';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

function row(base: ChannelRow, index: number, description: string, role: ChannelRow['role'], units: string, extra: Partial<ChannelRow> = {}): ChannelRow {
	return { ...base, abort: { ...base.abort }, index, description, role, units, physical: null, ...extra };
}

/** A screen on the mock: a hammer (row 1, N), two accelerometers (rows 3 and 4, g), a drive (row 2). */
async function screen(shaker = false) {
	const server = createControlServer({ speed: 3 });
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	const [c, d] = acq.rows;
	acq.rows = shaker
		? [
				row(c, 1, 'Force 1', 'monitor', 'N', { range_max: 500 }),
				row(d, 2, 'Shaker 1', 'drive', 'V'),
				row(c, 3, 'Accel A', 'control', 'g'),
				row(c, 4, 'Force 2', 'monitor', 'N', { range_max: 500 }),
				row(d, 5, 'Shaker 2', 'drive', 'V')
			]
		: [row(c, 1, 'Hammer', 'monitor', 'N', { range_max: 500 }), row(d, 2, 'Drive', 'drive', 'V'), row(c, 3, 'Accel A', 'control', 'g'), row(c, 4, 'Accel B', 'monitor', 'g')];
	expect(await acq.applyTable()).toBe(true);
	const modal = new ModalTestStore(acq, { local: null, checkDelayMs: 0 });
	await modal.sync();
	const observer = async () => {
		const o = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		o.connect();
		await o.whenOpen();
		return o;
	};
	return { acq, modal, observer };
}

/** The DOFs, through the table's own edit at each row's position. */
function dofs(modal: ModalTestStore, set: [number, number, string, 'reference' | 'response'][]) {
	for (const [index, node, direction, role] of set) {
		const p = modal.rows.findIndex((r) => r.index === index);
		expect(modal.editDof(p, { field: 'node', value: node })).toBe(true);
		expect(modal.editDof(p, { field: 'direction', value: direction })).toBe(true);
		expect(modal.editDof(p, { field: 'analysis_role', value: role })).toBe(true);
	}
}

async function impactLive(accept: 'auto' | 'manual' = 'auto', averages = 3) {
	const s = await screen();
	dofs(s.modal, [
		[1, 1, 'Z+', 'reference'],
		[3, 2, 'Z+', 'response'],
		[4, 3, 'Z+', 'response']
	]);
	Object.assign(s.modal.setup, { mode: 'impact', averages });
	s.modal.setup.impact.accept = accept;
	s.modal.setup.roving = { ...s.modal.setup.roving, on: true, channels: [3], how: 'list', text: '2Z+ 5Z+ 7Z+' };
	expect(await s.modal.apply()).toBe(true);
	expect(await s.modal.start()).toBe(true);
	await until(() => s.modal.now?.state === 'idle');
	return s;
}

describe('the Modal Test store', () => {
	it('modal test sets each DOF through the channel table and refuses a setup with no reference', async () => {
		const { modal, acq } = await screen();
		await modal.checkSetup();
		expect(modal.current?.check).toBeNull();
		expect(modal.current?.errors.map((e) => e.reason)).toContain('no_reference');
		dofs(modal, [
			[1, 1, 'Z+', 'reference'],
			[3, 2, 'Z+', 'response']
		]);
		expect(acq.tableDirty).toBe(true);
		expect(await modal.apply()).toBe(true);
		// the table went first, with the DOFs in its own columns
		expect(acq.tableDirty).toBe(false);
		expect(acq.rows.find((r) => r.index === 1)).toMatchObject({ node: 1, direction: 'Z+', analysis_role: 'reference' });
		expect(modal.runCheck?.references.map((r) => [r.index, r.node, r.direction])).toEqual([[1, 1, 'Z+']]);
		expect(modal.setupStatus).toBe('applied');
	});

	it('modal test counts every hit from the service and rejects a double hit and an overload in automatic mode', async () => {
		const { modal } = await impactLive('auto', 3);
		expect(modal.now?.counts.point).toEqual({ index: 0, accepted: 0, rejected: 0, asked: 3 });
		expect(modal.now?.counts.totals.points_planned).toBe(3);
		expect(await modal.simHit('double')).toBe(true);
		expect(await modal.verb('arm')).toBe(true);
		await until(() => (modal.now?.hits.length ?? 0) === 1);
		expect(modal.now?.latest).toMatchObject({ status: 'rejected', reason: 'double_hit', double_hit: true });
		expect(modal.now?.latest?.report?.double_hit?.frame).toBeGreaterThan(0);
		expect(await modal.simHit('overload')).toBe(true);
		await until(() => (modal.now?.hits.length ?? 0) === 2);
		expect(modal.now?.latest).toMatchObject({ status: 'rejected', reason: 'overload', overload: true });
		expect(modal.now?.counts.point).toMatchObject({ accepted: 0, rejected: 2 });
		// the operator overrides the service's verdict on the latest hit
		expect(await modal.verb('accept')).toBe(true);
		expect(modal.now?.counts.point).toMatchObject({ accepted: 1, rejected: 1 });
		expect(await modal.verb('undo')).toBe(true);
		expect(modal.now?.counts.point.accepted).toBe(0);
		expect(modal.now?.hits.map((h) => h.status)).toEqual(['rejected', 'undone']);
		await modal.abort();
	});

	it('modal test advances to the next point on its own once a point has its hits', async () => {
		const { modal } = await impactLive('auto', 2);
		await modal.simHit('clean');
		await modal.verb('arm');
		await until(() => (modal.now?.counts.totals.accepted ?? 0) >= 2);
		await until(() => modal.now?.point === 1);
		expect(modal.now?.counts.points[0]).toMatchObject({ measured: true, accepted: 2 });
		expect(modal.now?.counts.totals.points_measured).toBe(1);
		expect(modal.now?.point_dofs.responses.find((r) => r.index === 3)?.node).toBe(5);
		await modal.abort();
	});

	it('modal test holds each hit in manual mode until it is accepted or rejected, and the keys do it', async () => {
		const { modal } = await impactLive('manual', 2);
		modal.step = 'run';
		expect(modal.onKey({ key: ' ' })).toBe('arm');
		await until(() => modal.now?.state === 'captured');
		expect(modal.now?.latest?.status).toBe('pending');
		// typing in a field never accepts
		const input = { tagName: 'INPUT', type: 'text' } as unknown as EventTarget;
		expect(modal.onKey({ key: 'Enter', target: input })).toBeNull();
		expect(modal.onKey({ key: 'Backspace' })).toBe('reject');
		await until(() => modal.now?.latest?.status === 'rejected');
		expect(modal.now?.latest?.reason).toBe('operator');
		await until(() => modal.now?.state === 'captured' && modal.now.hits.length === 2);
		expect(modal.onKey({ key: 'a' })).toBe('accept');
		await until(() => modal.now?.counts.point.accepted === 1);
		expect(modal.onKey({ key: 'u' })).toBe('undo');
		await until(() => modal.now?.counts.point.accepted === 0);
		await until(() => modal.now?.state === 'captured' && modal.now.hits.length === 3);
		expect(modal.onKey({ key: 'Enter' })).toBe('accept');
		await until(() => modal.now?.state === 'armed');
		expect(modal.onKey({ key: ' ' })).toBe('disarm');
		await until(() => modal.now?.state === 'idle');
		expect(modal.onKey({ key: 'ArrowRight' })).toBe('next');
		await until(() => modal.now?.point === 1);
		expect(modal.onKey({ key: 'ArrowLeft' })).toBe('previous');
		await until(() => modal.now?.point === 0);
		await modal.abort();
	});

	it('modal test flags a hit High or Low against a target from a hit, and never rejects it for that', async () => {
		const { modal } = await impactLive('auto', 5);
		await modal.simHit('clean');
		await modal.verb('arm');
		await until(() => (modal.now?.hits.length ?? 0) === 1);
		expect(await modal.verb('target')).toBe(true);
		await until(() => (modal.now?.hits.length ?? 0) >= 4);
		const flagged = modal.now!.hits.filter((h) => h.target_verdict);
		expect(flagged.length).toBeGreaterThan(0);
		// a force off the target is a signal only: a flagged hit is rejected only for an overload or a double hit
		for (const h of flagged) if (h.status === 'rejected') expect(['overload', 'double_hit']).toContain(h.reason);
		expect(flagged.some((h) => h.status === 'accepted')).toBe(true);
		await modal.abort();
	});

	it('modal test drives two shakers frame by frame to the averages and refuses a repeated pseudo-random frame', async () => {
		const { modal } = await screen(true);
		dofs(modal, [
			[1, 1, 'Z+', 'reference'],
			[4, 6, 'X+', 'reference'],
			[3, 2, 'Z+', 'response']
		]);
		Object.assign(modal.setup, { mode: 'shaker', averages: 4 });
		modal.setup.shaker = { ...modal.setup.shaker, source: 'pseudo_random', drives: [{ row: 2, level_vrms: 0.05 }, { row: 5, level_vrms: 0.05 }] };
		// the service checks the DOFs in the table it holds: a check waits for the table to be applied
		await modal.checkSetup();
		expect(modal.current).toBeNull();
		expect(await modal.acq.applyTable()).toBe(true);
		await modal.checkSetup();
		expect(modal.current?.errors.find((e) => e.reason === ('source_refused' as never))?.message).toMatch(/force matrix cannot be inverted/);
		modal.setup.shaker.source = 'burst_random';
		await modal.checkSetup();
		expect(modal.current?.check?.estimator).toBe('matrix H1');
		expect(await modal.apply()).toBe(true);
		expect(await modal.start()).toBe(true);
		await until(() => modal.now?.state === 'idle');
		expect(await modal.verb('drive_start')).toBe(true);
		await until(() => (modal.now?.counts.point.accepted ?? 0) >= 2);
		expect(modal.now?.counts.unit).toBe('frames');
		await until(() => modal.live?.references?.autospectra.length === 2);
		expect(modal.live?.references?.autospectra.map((a) => a.index)).toEqual([1, 4]);
		// the latest frame taken, whole: every channel over the block, the burst's end and the input delay
		await until(() => !!modal.live?.frame);
		const f = modal.live!.frame!;
		expect(f.samples).toBe(modal.runCheck?.block);
		// the inputs in table order (rows 1, 3, 4), then the drives
		expect(f.channels.map((c) => c.index)).toEqual([1, 3, 4, 2, 5]);
		expect(f.channels.map((c) => c.role)).toEqual(['reference', 'response', 'reference', 'drive', 'drive']);
		expect(f.channels.filter((c) => c.role === 'drive').map((c) => c.index)).toEqual([2, 5]);
		expect(f.channels.every((c) => c.values.length === (f.decimation === 1 ? f.samples : 2 * Math.ceil(f.samples / f.decimation)))).toBe(true);
		expect(f.burst_end_frame).toBe(f.start_frame + Math.round(((modal.runCheck?.shaker?.burst_pct ?? 0) / 100) * f.samples));
		expect(f.input_delay_frames).toBeGreaterThan(0);
		await until(() => modal.now?.state === 'point_done', 15000);
		expect(modal.now?.counts.point).toMatchObject({ accepted: 4, asked: 4 });
		expect(await modal.stop()).toBe(true);
		await until(() => modal.now?.state === 'finished');
		expect(modal.now?.end?.outcome).toBe('ended');
		expect(workspaceForRun('modal_test')).toBe('modal');
	});

	it('modal test arms through arming, draws the own block of the hit with its windows, clears a point and refuses what starts after an end', async () => {
		const { modal } = await impactLive('auto', 3);
		await modal.simHit('double');
		expect(await modal.verb('arm')).toBe(true);
		expect(modal.now?.state).toBe('arming');
		expect(modal.verbs.disarm.enabled).toBe(true);
		await until(() => (modal.now?.hits.length ?? 0) === 1);
		const d = modal.now?.latest?.report?.double_hit;
		expect(d && d.peak > 0 && d.ratio > 0).toBe(true);
		await until(() => modal.live?.hit?.seq === modal.now?.latest?.seq);
		expect(modal.live?.hit?.channels.map((c) => c.role)).toEqual(['reference', ...modal.now!.point_dofs.responses.map(() => 'response')]);
		expect(modal.live?.hit?.windows.force).toEqual(modal.runCheck?.impact?.force_window);
		expect(modal.live?.hit?.force_spectrum.tip_drop_db).toBe(modal.now?.latest?.report?.tip_drop_db);
		await modal.simHit('clean');
		await until(() => (modal.now?.counts.point.accepted ?? 0) === 1);
		await until(() => modal.now?.state === 'armed');
		expect(await modal.verb('disarm')).toBe(true);
		expect(modal.verbs.clear.enabled).toBe(true);
		expect(await modal.verb('clear')).toBe(true);
		expect(modal.now?.counts.point.accepted).toBe(0);
		expect(modal.now?.hits.filter((h) => h.status === 'undone')).toHaveLength(1);
		expect(modal.verbs.clear.enabled).toBe(false);
		// once an end is asked nothing new starts: the service's refusal, in the screen's words
		await modal.control.request('modal_test.end');
		const err = (await modal.control.request('modal_test.arm').catch((e: unknown) => e)) as { reason?: string };
		expect(['ending', 'run_finished']).toContain(err.reason);
	});

	it('modal test refuses an observer the operator verbs and lets it end the test', async () => {
		const { modal, observer } = await impactLive('auto', 3);
		const o = await observer();
		const err = (await o.request('modal_test.arm').catch((e: unknown) => e)) as { reason?: string };
		expect(err.reason).toBe('not_controller');
		await o.request('modal_test.end');
		await until(() => modal.now?.state === 'finished');
	});
	it('modal test roves the hammer over the points in a roving hammer survey, and refuses a roving force row with shakers', async () => {
		const { modal } = await screen();
		dofs(modal, [
			[1, 1, 'Z+', 'reference'],
			[3, 2, 'Z+', 'response']
		]);
		modal.setup.roving = { ...modal.setup.roving, on: true, channels: [1], how: 'fill', fill: { start: 4, step: 2, count: 3 } };
		expect(await modal.apply()).toBe(true);
		expect(modal.runCheck?.points.map((p) => [p.references.map((r) => `${r.node}${r.direction}`), p.responses.map((r) => `${r.node}${r.direction}`)])).toEqual([
			[['4Z+'], ['2Z+']],
			[['6Z+'], ['2Z+']],
			[['8Z+'], ['2Z+']]
		]);
		modal.setup.mode = 'shaker';
		modal.setup.shaker.drives = [{ row: 2, level_vrms: 0.05 }];
		await modal.checkSetup();
		expect(modal.current?.errors.map((e) => [e.reason, e.field])).toContainEqual(['roving_invalid', 'config.roving.channels']);
		expect(modal.fieldErrors['roving.channels']?.[0]).toMatch(/force row does not rove/);
	});
});
