/**
 * The Acquire & Control store against the mock control service, through the real client and
 * codec: control of the service, the station, the channel table, the setup and the run.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory, type MockConnection, type MockServer } from '$lib/protocol/mock/socket';
import { defaultSetup, type SetupDraft } from './setup';
import { AcquireState } from './store.svelte';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

/** sessionStorage stand-in, so the lease survives a reconnect as it does in a tab. */
function memoryStorage(): Storage {
	const m = new Map<string, string>();
	return {
		get length() {
			return m.size;
		},
		clear: () => m.clear(),
		getItem: (k) => m.get(k) ?? null,
		key: (i) => [...m.keys()][i] ?? null,
		removeItem: (k) => void m.delete(k),
		setItem: (k, v) => void m.set(k, v)
	};
}

/** A short schedule so a run gets through its phases in a second or two at the mock's speed. */
function quick(d: SetupDraft = defaultSetup()): SetupDraft {
	d.acquisition = { fs_hz: 8192, n_pts: 1024, n_avg: 2 };
	d.schedule = {
		mode: 'auto',
		levels: [
			{ db: -6, dwell_s: 1 },
			{ db: 0, dwell_s: 0 }
		],
		full_level_s: 2
	};
	return d;
}

async function screen(server: MockServer, name = 'Shell on bench-pc', session: Storage | null = null, local: Storage | null = null) {
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
	const acq = new AcquireState(client, { name, session, local, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	return { client, acq };
}

/** Every method a client sends, in order, from the moment this is called. */
function methodsSent(client: ServiceClient): string[] {
	const sent: string[] = [];
	const request = client.request.bind(client);
	client.request = ((method: string, params?: Record<string, unknown>, options?: { timeoutMs?: number }) => {
		sent.push(method);
		return request(method, params, options);
	}) as typeof client.request;
	return sent;
}

/** The server side of each connection, as the mock attaches it. */
function connections(server: MockServer): MockConnection[] {
	const conns: MockConnection[] = [];
	const attach = server.attach?.bind(server);
	server.attach = (c) => {
		conns.push(c);
		attach?.(c);
	};
	return conns;
}

async function controller(options: ControlServerOptions = {}) {
	const server = createControlServer({ speed: 10, ...options });
	const s = await screen(server);
	expect(await s.acq.claim()).toBe(true);
	s.acq.setup = quick();
	return { server, ...s };
}

describe('control of the service', () => {
	it('starts as an observer, takes control, and a second screen is refused by name', async () => {
		const server = createControlServer({ speed: 10 });
		const a = await screen(server, 'Shell on bench-pc');
		expect(a.acq.role).toBe('observer');
		expect(a.acq.actions.apply.reason).toBe('Take control to apply the setup.');

		expect(await a.acq.claim()).toBe(true);
		expect(a.acq.role).toBe('controller');
		expect(a.acq.lease).toMatch(/^L-/);

		const b = await screen(server, 'Shell on laptop');
		expect(await b.acq.claim()).toBe(false);
		expect(b.acq.refusal?.reason).toBe('controller_held');
		expect(b.acq.refusal?.title).toBe('Another Screen Has Control');
		expect(b.acq.refusal?.facts).toContain('Held by Shell on bench-pc');
		await until(() => b.acq.session?.controller?.name === 'Shell on bench-pc');

		// An observer's verb is refused by reason, and the store says which.
		b.acq.setup = quick();
		expect(await b.acq.apply()).toBe(false);
		expect(b.acq.refusal?.reason).toBe('not_controller');
		expect(b.acq.refusal?.title).toBe('This Screen Is Observing');

		await a.acq.release();
		expect(a.acq.role).toBe('observer');
		expect(a.acq.lease).toBeNull();
		expect(await b.acq.claim()).toBe(true);
		a.client.close();
		b.client.close();
	});

	it('learns its own role from the seat event, without asking again', async () => {
		const server = createControlServer({ speed: 10 });
		const a = await screen(server, 'Shell on bench-pc');
		const b = await screen(server, 'Shell on laptop');
		const sentA = methodsSent(a.client);
		const sentB = methodsSent(b.client);
		expect(await a.acq.claim()).toBe(true);
		await until(() => b.acq.session?.controller?.name === 'Shell on bench-pc');
		expect(b.acq.role).toBe('observer');

		await a.acq.release();
		await until(() => b.acq.session?.controller === null);
		expect(await b.acq.claim()).toBe(true);
		await until(() => a.acq.session?.controller?.name === 'Shell on laptop');
		expect(a.acq.role).toBe('observer');
		expect(b.acq.role).toBe('controller');
		expect(a.acq.actions.apply.reason).toBe('Take control to apply the setup.');
		expect(sentA.filter((m) => m === 'session.state')).toEqual([]);
		expect(sentB.filter((m) => m === 'session.state')).toEqual([]);
		a.client.close();
		b.client.close();
	});

	it('resumes control with its lease after the connection drops, mid-run', async () => {
		const server = createControlServer({ speed: 10, timeoutS: 5 });
		const storage = memoryStorage();
		const s = await screen(server, 'Shell on bench-pc', storage);
		await s.acq.claim();
		const lease = s.acq.lease;
		s.acq.setup = quick();
		expect(await s.acq.apply()).toBe(true);
		expect(await s.acq.start()).toBe(true);

		// The connection goes; the run continues and the seat is kept, absent, for the timeout.
		s.client.close();
		const again = await screen(server, 'Shell on bench-pc', storage);
		expect(again.acq.role).toBe('controller');
		expect(again.acq.lease).toBe(lease);
		expect(again.acq.note?.text).toMatch(/still holds control/);
		expect(['running', 'review']).toContain(again.acq.run?.state);
		await again.acq.abort();
		again.client.close();
	});
});

describe('station and channel table', () => {
	it('selects driver-simulated NI devices, shows SIM, and refuses unbound rows until bound', async () => {
		const { acq, client } = await controller();
		expect(acq.selected?.kind).toBe('sim');
		acq.chooseStation('ni');
		acq.toggleDevice('cDAQ1Mod1');
		acq.toggleDevice('cDAQ1Mod2');
		expect(acq.stationDirty).toBe(true);
		expect(acq.actions.apply.reason).toMatch(/Use the chosen station/);
		expect(await acq.useStation()).toBe(true);
		expect(acq.selected?.class).toBe('ni_simulated');
		expect(acq.source?.source).toBe('SIM');

		await until(() => (acq.tableReport?.errors ?? []).some((e) => e.reason === 'channel_unbound'));
		expect(acq.findings.rows.get(1)?.[0].cell).toBe('physical');

		expect(acq.bindRows()).toBe(2);
		expect(acq.rows[0].physical).toEqual({ device: 'cDAQ1Mod1', channel: 'ai0' });
		expect(acq.rows[1].physical).toEqual({ device: 'cDAQ1Mod2', channel: 'ao0' });
		expect(acq.rows[0].interface).toBe('single_ended');
		await acq.checkTable();
		expect(acq.tableCheck?.errors).toEqual([]);
		expect(await acq.applyTable()).toBe(true);
		expect(acq.tableDirty).toBe(false);
		client.close();
	});

	it('never retries a stale write: it shows what changed and waits for the operator', async () => {
		const server = createControlServer({ speed: 10 });
		const a = await screen(server, 'A');
		const b = await screen(server, 'B');
		await a.acq.claim();
		// B's edit is based on revision 1; A writes revision 2 first.
		b.acq.rows[0].description = 'Table top';
		a.acq.rows[0].sensitivity_mv_per_eu = 10.2;
		expect(await a.acq.applyTable()).toBe(true);
		await until(() => b.acq.tableChange !== null);
		expect(b.acq.tableChange?.changes).toEqual(['Channel 1: sensitivity changed.']);
		expect(b.acq.rows[0].description).toBe('Table top');

		await a.acq.release();
		await b.acq.claim();
		// Applying from the old revision is refused; nothing is retried.
		expect(await b.acq.applyTable()).toBe(false);
		expect(b.acq.refusal?.reason).toBe('stale_revision');
		b.acq.keepEditsOnServer();
		expect(b.acq.base).toBe(2);
		expect(await b.acq.applyTable()).toBe(true);
		a.client.close();
		b.client.close();
	});
});

describe('setup', () => {
	it('is refused by reason, located by field, then applied once fixed', async () => {
		const { acq, client } = await controller();
		acq.setup.acquisition.fs_hz = 2048; // the grid stops at 800 Hz; the profile reaches 2000
		await acq.checkSetup();
		expect(acq.fieldErrors['acquisition.fs_hz']?.[0]).toMatch(/2000 Hz/);
		expect(acq.derived).toBeNull();
		expect(acq.actions.apply.enabled).toBe(false);

		expect(await acq.apply()).toBe(false);
		expect(acq.refusal?.reason).toBe('config_invalid');
		expect(acq.refusal?.title).toBe('The Setup Has Errors');

		acq.setup.acquisition.fs_hz = 8192;
		await acq.checkSetup();
		expect(acq.setupErrorCount).toBe(0);
		expect(acq.derived?.grms_g).toBeGreaterThan(5);
		expect(acq.derived?.grid.actual).toBe(false);
		expect(await acq.apply()).toBe(true);
		expect(acq.run?.state).toBe('configured');
		expect(acq.setupStatus).toBe('applied');
		expect(acq.primary).toBe('start');

		acq.setup.schedule.full_level_s = 5;
		expect(acq.setupStatus).toBe('changed');
		expect(acq.actions.start.reason).toMatch(/changed since it was applied/);
		acq.revertSetup();
		expect(acq.setupStatus).toBe('applied');
		client.close();
	});

	it('draws the profile from the dry run before anything is armed', async () => {
		const { acq, client } = await controller();
		expect(acq.reference).toBeNull();
		await acq.checkSetup();
		expect(acq.run?.state).toBe('unconfigured');
		const ref = acq.reference!;
		expect(ref.title).toBe('Reference');
		expect(ref.series.map((s) => s.name)).toEqual(['Reference', 'Alarm high', 'Alarm low', 'Abort high', 'Abort low']);
		expect(ref.series.every((s) => s.shape === 'steps' && !!s.edges)).toBe(true);
		// The lines are the requested grid's, inside the band: 8192 / 1024 = 8 Hz apart.
		const x = ref.series[0].x;
		expect(x[1] - x[0]).toBeCloseTo(8, 6);
		expect(x[0]).toBeGreaterThanOrEqual(20);
		expect(x[x.length - 1]).toBeLessThanOrEqual(2000);

		// A setup with errors has no profile to draw.
		acq.setup.acquisition.fs_hz = 2048;
		await acq.checkSetup();
		expect(acq.reference).toBeNull();
		client.close();
	});

	it('shows a second screen, and a reloaded one, the setup the service holds', async () => {
		const server = createControlServer({ speed: 10 });
		const localA = memoryStorage();
		const a = await screen(server, 'Shell on bench-pc', memoryStorage(), localA);
		// As the app runs it: the draft is kept in the browser's storage as it changes.
		const unwatch = a.acq.watch();
		await a.acq.claim();
		a.acq.setup = quick();
		a.acq.setup.title = 'Bench loopback';
		a.acq.setup.acquisition.n_avg = 8;
		await a.acq.checkSetup();
		expect(await a.acq.apply()).toBe(true);
		const grms = a.acq.derived?.grms_g;
		expect(grms).toBeGreaterThan(0);

		// A second screen with nothing of its own takes the service's setup into its editor.
		const b = await screen(server, 'Shell on laptop', null, memoryStorage());
		expect(b.acq.setupStatus).toBe('applied');
		expect(b.acq.setup.title).toBe('Bench loopback');
		expect(b.acq.setup.acquisition.n_avg).toBe(8);
		expect(b.acq.derived?.grms_g).toBe(grms);
		expect(b.acq.reference?.series[0].name).toBe('Reference');
		expect(b.acq.held?.runId).toBe(a.acq.run?.run_id);

		// The controller applies again: the second screen follows, since it changed nothing.
		a.acq.setup.title = 'Bench loopback, second';
		expect(await a.acq.apply()).toBe(true);
		await until(() => b.acq.setup.title === 'Bench loopback, second');
		expect(b.acq.setupStatus).toBe('applied');

		// Changes on a screen are its own: a reload keeps them, and Put Back restores the service's.
		a.acq.setup.schedule.full_level_s = 9;
		expect(a.acq.setupStatus).toBe('changed');
		await new Promise((r) => setTimeout(r, 10));
		unwatch();
		a.client.close();
		const again = await screen(server, 'Shell on bench-pc', null, localA);
		expect(again.acq.setup.schedule.full_level_s).toBe(9);
		expect(again.acq.setupStatus).toBe('changed');
		expect(again.acq.held?.config.title).toBe('Bench loopback, second');
		again.acq.revertSetup();
		expect(again.acq.setupStatus).toBe('applied');
		expect(again.acq.setup.schedule.full_level_s).toBe(2);
		again.client.close();
		b.client.close();
	});

	it('names a field that holds no number before anything is sent', async () => {
		const { acq, client } = await controller();
		acq.setup.breakpoints[1].slope_db_oct = NaN;
		expect(acq.fieldErrors['profile.breakpoints.1.value']).toEqual(['Enter a number.']);
		expect(await acq.checkSetup()).toBeNull();
		expect(acq.actions.apply.enabled).toBe(false);
		client.close();
	});

	it('blocks on feasibility, and arms past it only with the operator acknowledgement', async () => {
		const { acq, client } = await controller({ feasibilityExceeds: true });
		acq.setup.feasibility.enabled = true;
		acq.setup.feasibility.shaker_text = '{"model":"bench shaker"}';
		expect(await acq.apply()).toBe(true);
		expect(await acq.arm()).toBe(false);
		expect(acq.refusal?.reason).toBe('feasibility_exceeded');
		await until(() => acq.run?.state === 'blocked');
		expect(acq.primary).toBe('override');
		expect(await acq.arm('Reviewed the ratings with the test lead.')).toBe(true);
		expect(acq.run?.state).toBe('armed');
		await until(() => acq.log.some((e) => e.kind === 'override'));
		client.close();
	});
});

describe('running', () => {
	it('arms, starts, holds for review, proceeds, steps the level by hand, and stops ramped', async () => {
		const { acq, client } = await controller();
		expect(await acq.apply()).toBe(true);
		expect(await acq.arm()).toBe(true);
		expect(acq.run?.state).toBe('armed');
		expect(acq.run?.grid?.actual).toBe(true);
		expect(acq.actions.start.enabled).toBe(true);

		expect(await acq.start()).toBe(true);
		await until(() => acq.run?.state === 'review');
		expect(acq.primary).toBe('proceed');
		expect(await acq.proceed()).toBe(true);
		await until(() => acq.run?.phase === 'engaged' && acq.frame !== null);

		// Automatic ladder: a level step is refused by the bar before it is sent.
		expect(acq.actions.level.reason).toMatch(/manual/);
		expect(await acq.setMode('manual')).toBe(true);
		await until(() => acq.run?.mode === 'manual');
		expect(acq.actions.level.enabled).toBe(true);
		expect(await acq.goTo(0)).toBe(0);
		expect(await acq.level(-1)).toBe(-1);
		expect(await acq.goTo(-3)).toBeNull();
		expect(acq.refusal?.reason).toBe('not_a_rung');
		expect(acq.refusal?.facts).toEqual(['Schedule levels: -6, 0 dB']);

		await until(() => (acq.frame?.views[0].series ?? []).some((s) => s.name === 'Control'));
		const control = acq.frame!.views[0];
		expect(control.series.every((s) => s.shape === 'steps' && !!s.edges)).toBe(true);

		expect(await acq.stop()).toBe(true);
		expect(acq.run?.state).toBe('stopping');
		expect(acq.actions.stop.enabled).toBe(false);
		await until(() => acq.run?.state === 'finished');
		expect(acq.run?.end?.outcome).toBe('operator_stop');
		expect(acq.primary).toBe('reset');
		expect(await acq.reset()).toBe(true);
		expect(acq.run?.state).toBe('configured');
		expect(acq.setupStatus).toBe('applied');
		client.close();
	});

	it('aborts from an observer screen, at once, and says who', async () => {
		const server = createControlServer({ speed: 10 });
		const a = await screen(server, 'Shell on bench-pc');
		await a.acq.claim();
		a.acq.setup = quick();
		a.acq.setup.control.review_after_sysid = false;
		await a.acq.apply();
		await a.acq.start();
		const b = await screen(server, 'Shell on laptop');
		expect(b.acq.role).toBe('observer');
		expect(b.acq.actions.abort.enabled).toBe(true);
		expect(b.acq.actions.stop.enabled).toBe(true);
		expect(b.acq.actions.level.enabled).toBe(false);
		expect(await b.acq.abort()).toBe(true);
		await until(() => a.acq.run?.state === 'finished');
		expect(a.acq.run?.end?.outcome).toBe('operator_abort');
		expect(a.acq.run?.end?.message).toMatch(/observer/);
		a.client.close();
		b.client.close();
	});

	it('keeps one log per run, by its run id, and never repeats an entry', async () => {
		const server = createControlServer({ speed: 10 });
		const conns = connections(server);
		const s = await screen(server);
		await s.acq.claim();
		s.acq.setup = quick();
		s.acq.setup.control.review_after_sysid = false;
		await s.acq.apply();
		const first = s.acq.run!.run_id!;
		await s.acq.start();
		await until(() => s.acq.log.length >= 3);
		// An entry this screen already has, sent again: shown once, and nothing is lost.
		const n = s.acq.log.length;
		conns[0].emit('random.log', { run_id: first, entries: [s.acq.log[0]] });
		await new Promise((r) => setTimeout(r, 20));
		expect(s.acq.log.length).toBeGreaterThanOrEqual(n);
		expect(new Set(s.acq.log.map((e) => e.seq)).size).toBe(s.acq.log.length);

		await s.acq.abort();
		await until(() => s.acq.run?.state === 'finished');
		await s.acq.reset();
		const second = s.acq.run!.run_id!;
		expect(second).not.toBe(first);
		await until(() => s.acq.log.length > 0 && s.acq.log[0].seq === 0 && !s.acq.log.some((e) => /abort/i.test(e.text)));
		// A new run's log starts again, even when its first entry is not number 0.
		conns[0].emit('random.log', { run_id: 'r-9999', entries: [{ seq: 7, at_unix: 1, kind: 'note', text: 'another run' }] });
		await until(() => s.acq.log.length === 1 && s.acq.log[0].text === 'another run');
		s.client.close();
	});

	it('shows a protection trip with its facts', async () => {
		const { acq, client } = await controller({ abortDemoAtS: 0.5 });
		acq.setup.control.review_after_sysid = false;
		acq.setup.schedule.levels[0].dwell_s = 30;
		await acq.apply();
		await acq.start();
		await until(() => acq.run?.state === 'finished', 15000);
		expect(acq.run?.end?.outcome).toBe('aborted');
		expect(acq.run?.end?.trip?.kind).toBe('envelope');
		expect(acq.log.some((e) => e.kind === 'trip')).toBe(true);
		client.close();
	});

	it('ends held when the coherence gate is not met', async () => {
		const { acq, client } = await controller({ coherence: 0.3 });
		await acq.apply();
		await acq.start();
		await until(() => acq.run?.state === 'finished');
		expect(acq.run?.phase).toBe('coherence_hold');
		expect(acq.run?.end?.outcome).toBe('held');
		client.close();
	});
});

describe('reconnecting', () => {
	it('keeps unsent table edits across a drop and shows a table that moved meanwhile', async () => {
		const server = createControlServer({ speed: 10 });
		const storage = memoryStorage();
		const a = await screen(server, 'A', storage);
		await a.acq.claim();
		a.acq.rows[0].description = 'Table top, unsent';
		const b = await screen(server, 'B');
		// The drop: A's connection goes; B takes the free seat and writes revision 2.
		a.client.close();
		await until(() => b.acq.session?.controller?.present === false);
		await b.acq.claim();
		b.acq.rows[0].range_max = 50;
		expect(await b.acq.applyTable()).toBe(true);
		await b.acq.release();
		// A comes back on the same store: its edit is still there, and the change is shown.
		a.client.connect();
		await a.client.whenOpen();
		await a.acq.sync();
		expect(a.acq.rows[0].description).toBe('Table top, unsent');
		expect(a.acq.tableChange?.changes).toEqual(['Channel 1: range changed.']);
		a.client.close();
		b.client.close();
	});
});
