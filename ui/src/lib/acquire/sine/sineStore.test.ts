/**
 * The Sine store over the real client and codec, against the mock control service: the law
 * catalog of the Sine world, the service's reading of the profile and of pasted or file rows, the
 * dry run of the draft, the setup applied, a run from identification through the review hold to the
 * end of the schedule with its ramp, the sweep verbs, the manual ladder, a live law parameter and
 * each reason it is refused, the run slot shared with the other test types, and a window's
 * subscription. The mock refuses as the service does; the last part holds it to that.
 */
import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { AcquireState } from '../store.svelte';
import { SrsStore } from '../srs/srsStore.svelte';
import { SineStore } from './sineStore.svelte';
import { SineViewsState } from './sineViews.svelte';
import { errorWords, sineLevelStatus, sweepWords } from './sineWords';

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
	const sine = new SineStore(acq, { local: null, checkDelayMs: 0 });
	await sine.sync();
	/** A second screen on the same service, observing. */
	const observer = async () => {
		const c = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		c.connect();
		await c.whenOpen();
		return c;
	};
	return { client, acq, sine, observer };
}

function ready(sine: SineStore) {
	sine.setup.limits.drive_vpk_ceiling = 0.5;
	sine.setup.limits.drive_vpk_abort = 0.8;
}

/** A short sweep for a run that ends in a test: 50 to 800 Hz at 32 oct/min, two levels of one sweep each. */
function short(sine: SineStore) {
	ready(sine);
	const t = sine.setup.test;
	t.band_hz = [50, 800];
	t.sweep.oct_per_min = 32;
	t.sweep.sweeps = 2;
	sine.setup.profile.rows = [
		{ freq_hz: 20, quantity: 'accel_g', value: 1, own_bands: false, alarm_db: [1.5, 1.5], abort_db: [3, 3] },
		{ freq_hz: 2000, quantity: 'accel_g', value: 1, own_bands: false, alarm_db: [1.5, 1.5], abort_db: [3, 3] }
	];
	sine.setup.schedule.levels = [-6, 0];
	sine.setup.schedule.start_db = -6;
	sine.setup.schedule.full_level = { by: 'sweeps', n: 1 };
}

const refusal = async (p: Promise<unknown>) => (await p.catch((e: unknown) => e)) as RequestError;

async function running(sine: SineStore) {
	if (sine.setupStatus !== 'applied') expect(await sine.apply()).toBe(true);
	expect(await sine.start()).toBe(true);
	await until(() => sine.status?.state === 'review');
	expect(await sine.proceed()).toBe(true);
	await until(() => sine.now?.phase === 'engaged' && sine.live?.frequency_hz !== null && sine.live?.frequency_hz !== undefined);
}

describe('the Sine store', () => {
	it("reads the Sine world's laws only, and starts the law at its defaults", async () => {
		const { sine } = await screen();
		expect(sine.sineLaws.map((l) => l.id)).toEqual(['servo_blend']);
		expect(sine.law?.name).toBe('Servo blend');
		expect(sine.law?.live).toEqual(['blend_w']);
		expect(sine.setup.law.params).toEqual({ blend_w: 0.5, compression: 'slow', ki: 2, max_rate_db_per_s: 20, kp: 0.3 });
	});

	it('imports a Sine law file and shows its parameters from its manifest; another world is not chosen', async () => {
		const { sine } = await screen();
		expect(await sine.importLaw('/laws/sine_template.wasm')).toBe(true);
		expect(sine.sineLaws.map((l) => l.name)).toEqual(['Servo blend', 'Sine PI']);
		expect(sine.law?.name).toBe('Sine PI');
		expect(sine.setup.law.params).toEqual({ kp: 0.5, ki: 0.1, start_offset_db: 0 });
		expect(sine.wire.control?.law?.params).toEqual({ kp: 0.5, ki: 0.1, start_offset_db: 0 });
		expect(await sine.importLaw('/laws/shock_template.wasm')).toBe(true);
		expect(sine.note?.text).toMatch(/not a Sine law/);
		expect(sine.sineLaws).toHaveLength(2);
		expect(await sine.importLaw('laws/relative.wasm')).toBe(false);
		expect(sine.refusal?.reason).toBe('bad_params');
	});

	it('asks for the drive ceiling and abort before the setup can be applied', async () => {
		const { sine } = await screen();
		expect(sine.actions.apply.reason).toBe('Fix the 2 errors in the setup first.');
		expect(sine.counts.limits.errors).toBe(2);
		ready(sine);
		expect(sine.actions.apply.enabled).toBe(true);
		sine.setup.limits.drive_vpk_abort = 0.4;
		expect(sine.fieldErrors['limits.drive_vpk_abort']).toEqual(['The drive abort is at least the drive ceiling.']);
	});

	it("draws the profile from the service's reading, with its crossovers and kinematics, and names a profile short of the band", async () => {
		const { sine } = await screen();
		ready(sine);
		await sine.refreshValidate();
		const v = sine.profileCheck;
		expect(v?.views.map((x) => x.title)).toEqual(['Profile', 'Velocity', 'Displacement']);
		expect(v?.crossovers).toEqual([{ freq_hz: 44.9, from: 'disp_pp_in', to: 'accel_g' }]);
		expect(v?.kinematics?.verdict).toBe('not_evaluated');
		sine.setup.profile.rows[2].freq_hz = 1500;
		expect(sine.profileCheck).toBeNull();
		await sine.checkSetup();
		expect(sine.fieldErrors['profile.rows']?.[0]).toMatch(/covers 20 to 1500 Hz/);
		expect(sine.counts.profile.errors).toBe(1);
	});

	it('reads rows pasted as text or from a file, keeps the bands, and names a row it cannot read', async () => {
		const { sine } = await screen();
		sine.setup.profile.alarm_db = [1, 1];
		expect(await sine.importProfile({ text: '# a profile\nfreq_hz, accel_g, units\n20, 0.5, g\n2000, 0.5, g\n' })).toBe(true);
		expect(sine.setup.profile.rows.map((r) => [r.freq_hz, r.quantity, r.value])).toEqual([
			[20, 'accel_g', 0.5],
			[2000, 'accel_g', 0.5]
		]);
		expect(sine.setup.profile.alarm_db).toEqual([1, 1]);
		expect(sine.imported?.warnings[0].reason).toBe('units_column_ignored');
		expect(await sine.importProfile({ text: 'freq_hz, accel_g, velocity_ips\n20, 1, 2\n' })).toBe(false);
		expect([sine.imported?.errors[0].reason, sine.imported?.errors[0].row]).toEqual(['quantity_ambiguous', 0]);
		expect(sine.setup.profile.rows).toHaveLength(2);
		expect(await sine.importProfile({ path: '/data/profile.csv' })).toBe(true);
		expect(sine.setup.profile.rows.map((r) => r.quantity)).toEqual(['disp_pp_in', 'accel_g', 'accel_g']);
		expect(await sine.importProfile({ path: '/data/missing.csv' })).toBe(false);
		expect(sine.imported?.errors[0].field).toBe('path');
		expect(await sine.importProfile({ path: 'data/profile.csv' })).toBe(false);
		expect([sine.refusal?.reason, sine.refusal?.detail?.param]).toEqual(['bad_params', 'path']);
	});

	it('reads the run\'s numbers from its own data frames once it has subscribed to plots, as the service sends them', async () => {
		const { sine, client } = await screen();
		short(sine);
		sine.setup.identification.review = false;
		expect(await sine.apply()).toBe(true);
		// A screen drawing its pages subscribes to plots: from then on the service sends it `sine.data`, not `sine.live`.
		const lives: number[] = [];
		client.on('sine.live', () => lives.push(1));
		await client.request('sine.subscribe', { rate_hz: 5, plots: [{ id: 'p1', quantity: 'control' }] });
		expect(await sine.start()).toBe(true);
		await until(() => (sine.frameOfRun?.t_s ?? 0) > 0.5);
		const t = sine.frameOfRun!.t_s!;
		await until(() => (sine.frameOfRun?.t_s ?? 0) > t + 0.5);
		expect(typeof sine.frameOfRun?.drive_vpk).toBe('number');
		expect(lives).toHaveLength(0);
		await sine.abort();
	}, 15000);

	it("latches a control row's overload once it reaches 95 % of the abort band's top, and only a control row's", async () => {
		const { sine } = await screen();
		short(sine);
		sine.setup.identification.review = false;
		sine.setup.profile.abort_db = [0.3, 0.3];
		sine.setup.profile.alarm_db = [0.2, 0.2];
		expect(await sine.apply()).toBe(true);
		expect(await sine.start()).toBe(true);
		await until(() => !!sine.live?.channels.find((c) => c.role === 'control')?.overload);
		const live = sine.live!;
		expect(live.channels.filter((c) => c.role !== 'control').every((c) => !c.overload)).toBe(true);
		const control = live.channels.find((c) => c.role === 'control')!;
		expect(live.alarms).toContainEqual({ kind: 'overload', channel: control.index });
		// latched: it stays to the end of the run
		const t = live.t_s ?? 0;
		await until(() => (sine.live?.t_s ?? 0) > t + 0.5);
		expect(sine.live?.channels.find((c) => c.index === control.index)?.overload).toBe(true);
		await sine.abort();
	}, 15000);

	it('names the service findings on their sections: the reference output on the simulator, a level short of the sweeps, a limit channel on the control row', async () => {
		const { sine } = await screen();
		ready(sine);
		sine.setup.cola.enabled = true;
		sine.setup.test.sweep.sweeps = 2;
		sine.setup.limits.channels.push({ channel: 1, shared_name: '', profile: { label: '', rows: [{ freq_hz: 10, quantity: 'accel_g', value: 3, own_bands: false, alarm_db: [1.5, 1.5], abort_db: [3, 3] }, { freq_hz: 3000, quantity: 'accel_g', value: 3, own_bands: false, alarm_db: [1.5, 1.5], abort_db: [3, 3] }], alarm_db: [1.5, 1.5], abort_db: [3, 3] } });
		expect(sine.fieldErrors['limits.channels.0.channel']?.[0]).toMatch(/is a control channel/);
		sine.setup.limits.channels = [];
		await sine.checkSetup();
		expect(sine.fieldErrors['cola.enabled']?.[0]).toMatch(/simulator has one/);
		expect(sine.fieldErrors['test.end.sweeps']?.[0]).toMatch(/needs 4 to hold full level/);
		expect([sine.counts.limits.errors, sine.counts.test.errors]).toEqual([1, 1]);
	});

	it('runs a sweep from identification through the review hold to the end of the schedule, ramping down', async () => {
		const { sine } = await screen({ speed: 5 });
		short(sine);
		await sine.checkSetup();
		expect(sine.check?.derived?.grid).toEqual({ fs_hz: 16384, tick_frames: 164, tick_s: 0.0100098, actual: false });
		expect(sine.check?.derived?.stop_ramp_db_per_s).toBe(120);
		expect(await sine.apply()).toBe(true);
		expect(sine.setupStatus).toBe('applied');
		expect(await sine.start()).toBe(true);
		expect(sine.step).toBe('run');
		await until(() => sine.status?.state === 'review');
		expect(sineLevelStatus(sine.now, sine.live).text).toBe('Holding for review');
		expect(sine.status?.identification?.passed).toBe(true);
		expect(sine.primary).toBe('proceed');
		// the law is built at proceed: before it there is nothing to change
		expect(sine.liveParams[0]).toMatchObject({ key: 'blend_w', enabled: false, reason: 'Once the tone is driving at the schedule.' });
		expect(await sine.adjust('blend_w', 0.3)).toBeNull();
		expect(sine.refusal?.reason).toBe('wrong_state');
		expect(await sine.proceed()).toBe(true);
		await until(() => sine.live?.phase === 'engaged' && (sine.live?.frequency_hz ?? 0) > 60);
		const f0 = sine.live!.frequency_hz!;
		await until(() => (sine.live?.frequency_hz ?? 0) > f0 + 5);
		expect(sine.now?.direction).toBe('up');
		expect(sineLevelStatus(sine.now, sine.live, sine.runConfig).text).toBe('Sweeping up at 32 oct/min at -6.0 dB');
		expect(sweepWords(sine.now)).toBe('Sweep 1 of 1, up');
		expect(errorWords(sine.live)).toMatch(/^[\d.]+ g against [\d.]+ g, [+-][\d.]+ dB$/);
		await until(() => sine.now?.phase === 'full_level');
		await until(() => sine.status?.state === 'finished', 15000);
		expect(sine.status?.end?.outcome).toBe('completed');
		expect(sine.status?.end?.ramp_s).toBeGreaterThan(0.5);
		expect(sine.log.some((e) => e.kind === 'level')).toBe(true);
		expect(sine.primary).toBe('reset');
	}, 20000);

	it('holds, turns, jogs and goes to a frequency on a sweep; refuses a frequency outside the band', async () => {
		const { sine } = await screen();
		short(sine);
		sine.setup.test.sweep.oct_per_min = 2;
		sine.setup.test.sweep.sweeps = 4;
		sine.setup.identification.review = false;
		expect(await sine.apply()).toBe(true);
		expect(await sine.start()).toBe(true);
		await until(() => sine.now?.phase === 'engaged' && sine.live?.frequency_hz !== null);
		expect(sine.verbs.hold).toMatchObject({ visible: true, enabled: true });
		expect(sine.verbs.resume.visible).toBe(false);
		expect(await sine.hold()).toBe(true);
		expect([sine.verbs.hold.visible, sine.verbs.resume.visible]).toEqual([false, true]);
		await until(() => sine.live?.held === true);
		const held = sine.live!.frequency_hz!;
		await new Promise((r) => setTimeout(r, 300));
		expect(Math.abs((sine.live?.frequency_hz ?? 0) - held)).toBeLessThan(0.01);
		expect(sine.live?.sweep_rate_oct_per_min).toBe(0);
		expect(sineLevelStatus(sine.now, sine.live).text).toBe(`Holding at ${held.toFixed(1)} Hz`);
		expect(await sine.gotoHz(300)).toBe(300);
		expect(await sine.jog(10)).toBe(310);
		expect(await sine.jog(-1000)).toBe(50);
		expect(await sine.resume()).toBe(true);
		expect(await sine.reverse()).toBe(true);
		await until(() => sine.live?.direction === 'down');
		expect(await sine.gotoHz(5000)).toBeNull();
		expect([sine.refusal?.reason, sine.refusal?.detail?.band_hz]).toEqual(['out_of_band', [50, 800]]);
		expect(sine.refusal?.title).toBe('Outside the Band');
	}, 15000);

	it("refuses the sweep verbs on a dwell, and offers them only on a sweep", async () => {
		const { sine, client } = await screen();
		short(sine);
		sine.setup.test.kind = 'dwell';
		sine.setup.test.dwell = { freq_hz: 250, by: 'seconds', seconds: 300, cycles: 1000 };
		sine.setup.schedule.per_level = { by: 'seconds', n: 30 };
		sine.setup.schedule.full_level = { by: 'seconds', n: 30 };
		sine.setup.identification.review = false;
		expect(await sine.apply()).toBe(true);
		expect(await sine.start()).toBe(true);
		await until(() => sine.now?.phase === 'engaged');
		expect([sine.verbs.reverse.visible, sine.verbs.jog.visible, sine.verbs.goto.visible, sine.verbs.hold.visible]).toEqual([false, false, false, true]);
		const r = await refusal(client.request('sine.reverse'));
		expect([r.reason, r.detail?.test]).toEqual(['not_a_sweep', 'dwell']);
		expect((await refusal(client.request('sine.goto', { hz: 300 }))).reason).toBe('not_a_sweep');
		await until(() => sine.live?.frequency_hz === 250 && sine.live.level_db === -6);
		expect(sineLevelStatus(sine.now, sine.live).text).toBe('Dwelling at 250.0 Hz at -6.0 dB');
		expect(sweepWords(sine.now)).toBeNull();
	}, 15000);

	it('steps the level on the manual ladder only, to a level of the schedule or by a step held to the schedule', async () => {
		const { sine } = await screen();
		short(sine);
		sine.setup.test.sweep.oct_per_min = 2;
		sine.setup.test.sweep.sweeps = 4;
		sine.setup.schedule.levels = [-12, -6, 0];
		sine.setup.schedule.start_db = -12;
		sine.setup.identification.review = false;
		expect(await sine.apply()).toBe(true);
		expect(await sine.start()).toBe(true);
		await until(() => sine.now?.phase === 'engaged');
		expect(sine.actions.level.reason).toBe('Switch the ladder to manual to step the level.');
		expect(await sine.level(1)).toBeNull();
		expect(sine.refusal?.reason).toBe('manual_mode_required');
		expect(await sine.setMode('manual')).toBe(true);
		await until(() => sine.now?.mode === 'manual');
		expect(sine.actions.level.enabled).toBe(true);
		expect(await sine.level(-1)).toBe(-12);
		expect(await sine.level(1)).toBe(-11);
		expect(await sine.goTo(-6)).toBe(-6);
		expect(await sine.goTo(-7)).toBeNull();
		expect([sine.refusal?.reason, sine.refusal?.detail?.levels_db]).toEqual(['not_a_rung', [-12, -6, 0]]);
		await until(() => sine.live?.commanded_db === -6 && Math.abs((sine.live?.level_db ?? 0) + 6) < 0.1);
	}, 15000);

	it('changes a live parameter of the run, and refuses one that is unknown, fixed or out of its range', async () => {
		const { sine, client } = await screen();
		short(sine);
		sine.setup.test.sweep.oct_per_min = 2;
		sine.setup.test.sweep.sweeps = 4;
		expect(await sine.apply()).toBe(true);
		const before = sine.liveParams;
		expect(before.map((p) => [p.key, p.live, p.enabled, p.reason])).toEqual([
			['blend_w', true, false, 'Changed during a run.'],
			['ki', false, false, 'Fixed for the run.'],
			['max_rate_db_per_s', false, false, 'Fixed for the run.'],
			['kp', false, false, 'Fixed for the run.']
		]);
		await running(sine);
		expect(sine.liveParams[0]).toMatchObject({ key: 'blend_w', enabled: true, value: 0.5 });
		expect(sine.liveParams.slice(1).every((p) => !p.enabled)).toBe(true);
		expect(await sine.adjust('blend_w', 0.2)).toEqual({ key: 'blend_w', value: 0.2 });
		await until(() => sine.status?.law?.live.blend_w === 0.2);
		expect(sine.liveParams[0].value).toBe(0.2);
		expect(sine.log.some((e) => e.kind === 'law' && /blend_w 0.5 to 0.2/.test(e.text))).toBe(true);
		const why = async (key: string, value: number) => (await refusal(client.request('sine.adjust', { key, value }))).detail?.why;
		expect(await why('nope', 1)).toBe('unknown_key');
		expect(await why('kp', 0.4)).toBe('not_live');
		expect(await why('blend_w', 2)).toBe('out_of_range');
		expect(await sine.adjust('kp', 0.4)).toBeNull();
		expect([sine.refusal?.reason, sine.refusal?.title, sine.refusal?.facts]).toEqual(['adjust_refused', 'Parameter Not Changed', ['Parameter: kp', 'That parameter is fixed for the run.']]);
	}, 15000);

	it("passes an imported law's own refusal of a value through as the law's", async () => {
		const { sine, client } = await screen();
		short(sine);
		sine.setup.test.sweep.oct_per_min = 2;
		sine.setup.test.sweep.sweeps = 4;
		expect(await sine.importLaw('/laws/sine_template.wasm')).toBe(true);
		await running(sine);
		expect(sine.liveParams.map((p) => [p.key, p.live])).toEqual([
			['kp', true],
			['ki', false],
			['start_offset_db', false]
		]);
		expect((await refusal(client.request('sine.adjust', { key: 'kp', value: 1.8 }))).detail?.why).toBe('law_refused');
		expect(await sine.adjust('kp', 1.2)).toEqual({ key: 'kp', value: 1.2 });
	}, 15000);

	it('keeps a finished run finished when a stale reply arrives after it', async () => {
		// The stop's reply carries the state at the stop, "stopping". When the run finishes before
		// that reply lands (the mock ends a stop within one tick; a loaded runner reorders the two),
		// the reply must not take the screen back to "stopping", where no later event would correct it.
		const { sine } = await screen();
		short(sine);
		await running(sine);
		const live = sine.status!;
		const finished = { ...live, state: 'finished' } as typeof live;
		const stale = { ...live, state: 'stopping' } as typeof live;
		(sine as unknown as { onStatus(s: typeof live): void }).onStatus(finished);
		(sine as unknown as { onStatus(s: typeof live): void }).onStatus(stale);
		expect(sine.status?.state).toBe('finished');
		await sine.abort();
	});

	it('stops with the ramp from the level in force; an observer may stop and abort but not hold', async () => {
		const { sine, observer } = await screen();
		short(sine);
		sine.setup.test.sweep.oct_per_min = 2;
		sine.setup.test.sweep.sweeps = 4;
		await running(sine);
		const obs = await observer();
		expect((await refusal(obs.request('sine.hold'))).reason).toBe('not_controller');
		expect((await refusal(obs.request('sine.adjust', { key: 'blend_w', value: 0.3 }))).reason).toBe('not_controller');
		expect(await sine.stop()).toBe(true);
		await until(() => sine.status?.state === 'stopping' || sine.status?.state === 'finished');
		expect(sineLevelStatus(sine.now, sine.live).text).toMatch(/^Stopping, ramping down from|^Stopped by/);
		await until(() => sine.status?.state === 'finished');
		expect(sine.status?.end?.outcome).toBe('operator_stop');
		expect(sine.status?.end?.level_db).toBe(-6);
		expect(sine.status?.end?.ramp_s).toBeGreaterThan(0.5);
		expect(await sine.reset()).toBe(true);
		await running(sine);
		await obs.request('sine.abort');
		await until(() => sine.status?.state === 'finished');
		expect(sine.status?.end?.outcome).toBe('operator_abort');
		expect(sine.status?.end?.ramp_s).toBeNull();
	}, 15000);

	it('ends a law file that answers late three times in a row with the controlled stop', async () => {
		const { sine } = await screen();
		short(sine);
		sine.setup.test.sweep.oct_per_min = 2;
		sine.setup.test.sweep.sweeps = 4;
		expect(await sine.importLaw('/laws/sine_misbehave.wasm')).toBe(true);
		sine.setup.law.params.after_s = 1;
		await running(sine);
		await until(() => sine.status?.state === 'finished', 10000);
		const end = sine.status?.end;
		expect([end?.outcome, end?.trip?.kind, end?.trip?.kind === 'law_fault' && end.trip.reason, end?.trip?.kind === 'law_fault' && end.trip.consecutive]).toEqual(['law_fault', 'law_fault', 'over_budget', 3]);
		expect(end?.ramp_s).toBeGreaterThan(0);
	}, 15000);

	it('shares the run slot: a live Sine run holds off SRS shock, and is refused while another run is live', async () => {
		const { acq, sine } = await screen();
		short(sine);
		sine.setup.test.sweep.oct_per_min = 2;
		sine.setup.test.sweep.sweeps = 4;
		const srs = new SrsStore(acq, { local: null, checkDelayMs: 0 });
		await srs.sync();
		srs.setup.limits.drive_vpk_ceiling = 0.5;
		srs.setup.limits.drive_vpk_abort = 0.8;
		expect(await sine.apply()).toBe(true);
		expect(await srs.apply()).toBe(true);
		expect(await sine.start()).toBe(true);
		expect(await srs.start()).toBe(false);
		expect([srs.refusal?.reason, srs.refusal?.detail?.run_id]).toEqual(['run_active', sine.status?.run_id]);
		expect(await sine.abort()).toBe(true);
		await until(() => sine.status?.state === 'finished');
		expect(sine.status?.end?.outcome).toBe('operator_abort');
		expect(await srs.start()).toBe(true);
		expect(await sine.reset()).toBe(true);
		expect(await sine.start()).toBe(false);
		expect(sine.refusal?.reason).toBe('run_active');
		await srs.abort();
	}, 15000);

	it("subscribes a window's pages: channels on the plots that take them, refused where they do not", async () => {
		const { sine, client } = await screen();
		short(sine);
		sine.setup.test.sweep.oct_per_min = 2;
		sine.setup.test.sweep.sweeps = 4;
		const views = new SineViewsState(client, { setupTitle: () => sine.setup.title, showing: () => true, storage: null, session: null, events: null, subscribeDelayMs: 0 });
		const stop = views.watch();
		await until(() => views.accepted !== null);
		expect(views.accepted?.plots?.map((p) => [p.quantity, p.channels ?? null, p.points ?? null])).toEqual([
			['control', null, null],
			['drive', null, null]
		]);
		views.selectPage(views.pages.find((p) => p.name === 'Limits')!.id);
		await until(() => views.accepted?.plots?.[0]?.quantity === 'response');
		expect(views.accepted?.plots?.[0].channels).toBe('limited');
		views.selectPage(views.pages.find((p) => p.name === 'Responses')!.id);
		await until(() => views.accepted?.plots?.length === 2 && views.accepted.plots[1].quantity === 'phase');
		const response = views.page.panels.find((p) => p.kind === 'plot' && p.plot.quantity === 'response')!.id;
		// a time history added to the page is asked for with its point count
		const time = views.addPlot('time') as string;
		await until(() => views.accepted?.plots?.length === 3);
		expect(views.accepted?.plots?.map((p) => [p.id, p.quantity, p.points ?? null])[2]).toEqual([time, 'time', 1024]);
		await running(sine);
		await until(() => views.views(response).length > 0);
		expect(views.views(response)[0].view).toBe('spectrum');
		expect(views.frame(response)?.channels?.length).toBeGreaterThan(0);
		const bad = async (plot: Record<string, unknown>) => (await refusal(client.request('sine.subscribe', { plots: [{ id: 'p', ...plot }] }))).detail?.param;
		expect(await bad({ quantity: 'control', channels: 'all' })).toBe('plots[0].channels');
		expect(await bad({ quantity: 'response', points: 512 })).toBe('plots[0].points');
		expect(await bad({ quantity: 'time', points: 10 })).toBe('plots[0].points');
		expect(await bad({ quantity: 'spectrum' })).toBe('plots[0].quantity');
		stop();
		await sine.abort();
	}, 15000);
});
