/**
 * The mock control service's hardware and Random methods, through the real client and codec:
 * what the shell's Acquire & Control workspace will be built against.
 */
import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient } from '../client.svelte';
import type {
	ChannelRow,
	ChannelTable,
	ChannelsSetResult,
	LogEntry,
	RandomConfig,
	RandomConfigureResult,
	RandomDryRunResult,
	RandomLive,
	RandomState,
	SessionChanged,
	SessionClaimResult,
	SessionState,
	StationList
} from '../types';
import { createControlServer, type ControlServerOptions } from './control';
import { mockSocketFactory, type MockServer } from './socket';

const until = async (cond: () => boolean, ms = 5000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

function connect(server: MockServer) {
	const c = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
	c.connect();
	return c;
}

async function refusal(p: Promise<unknown>): Promise<RequestError> {
	const err = await p.then(
		() => null,
		(e: unknown) => e
	);
	expect(err).toBeInstanceOf(RequestError);
	return err as RequestError;
}

/** A short schedule so a run finishes in a second or two at the test speed. */
function config(overrides: Partial<RandomConfig> = {}): RandomConfig {
	return {
		title: 'Qualification Random',
		profile: {
			breakpoints: [
				{ freq_hz: 20, psd_g2_hz: 0.01 },
				{ freq_hz: 80, psd_g2_hz: 0.04 },
				{ freq_hz: 350, psd_g2_hz: 0.04 },
				{ freq_hz: 2000, psd_g2_hz: 0.007 }
			],
			alarm_db: [3, 3],
			abort_db: [6, 6]
		},
		acquisition: { fs_hz: 8192, n_pts: 1024, n_avg: 2 },
		schedule: { mode: 'auto', levels: [{ db: -6, dwell_s: 1 }, { db: 0 }], full_level_s: 2 },
		...overrides
	};
}

/** A server, a controlling client, and everything it is sent. */
async function setup(options: ControlServerOptions = {}) {
	const server = createControlServer({ speed: 10, ...options });
	const c = connect(server);
	const states: RandomState[] = [];
	const lives: RandomLive[] = [];
	const logs: LogEntry[] = [];
	c.on('random.state', (d) => states.push(d as unknown as RandomState));
	c.on('random.live', (d) => lives.push(d as unknown as RandomLive));
	c.on('random.log', (d) => logs.push(...(d as unknown as { entries: LogEntry[] }).entries));
	await c.whenOpen();
	const claim = await c.request<SessionClaimResult>('session.claim', { name: 'Shell on bench-pc' });
	return { server, c, states, lives, logs, claim };
}

describe('controller and observers', () => {
	it('one client controls, the others observe and are refused by name', async () => {
		const { server, c, claim } = await setup();
		expect(claim).toMatchObject({ role: 'controller', resumed: false, heartbeat_s: 5, timeout_s: 30 });
		const other = connect(server);
		const changes: SessionChanged[] = [];
		other.on('session.changed', (d) => changes.push(d as unknown as SessionChanged));
		await other.whenOpen();
		expect(await other.request<SessionState>('session.state')).toMatchObject({ role: 'observer', controller: { name: 'Shell on bench-pc', present: true } });
		const held = await refusal(other.request('session.claim'));
		expect([held.code, held.reason, held.detail]).toEqual(['refused', 'controller_held', { holder: 'Shell on bench-pc' }]);
		const e = await refusal(other.request('station.select', { kind: 'sim' }));
		expect([e.reason, e.detail?.holder]).toEqual(['not_controller', 'Shell on bench-pc']);
		// reading needs no control
		expect((await other.request<ChannelTable>('channels.get')).revision).toBe(1);
		await c.request('session.release');
		await until(() => changes.some((s) => s.controller === null));
		expect((await other.request<SessionClaimResult>('session.claim')).role).toBe('controller');
		c.close();
		other.close();
	});

	it('observers may stop and abort, nothing else', async () => {
		const { server, c, states, logs } = await setup();
		const long = config({ schedule: { mode: 'auto', levels: [{ db: 0 }], full_level_s: 600 }, control: { review_after_sysid: false } });
		await c.request('random.configure', { config: long });
		await c.request('random.start');
		await until(() => states.at(-1)?.phase === 'full_level');
		const observer = connect(server);
		await observer.whenOpen();
		const table = await observer.request<ChannelTable>('channels.get');
		for (const [method, params] of [
			['station.select', { kind: 'sim' }],
			['channels.set', { revision: table.revision, channels: table.channels }],
			['random.configure', { config: long }],
			['random.arm', {}],
			['random.start', {}],
			['random.proceed', {}],
			['random.level', { delta_db: 1 }],
			['random.mode', { mode: 'manual' }],
			['random.reset', {}],
			['random.clear', {}]
		] as const) {
			const e = await refusal(observer.request(method, params));
			expect([method, e.reason, e.detail?.holder]).toEqual([method, 'not_controller', 'Shell on bench-pc']);
		}
		expect(states.at(-1)?.phase).toBe('full_level');

		// the gentler way out: the controlled stop, named in the end message
		const stopping = await observer.request<RandomState>('random.stop');
		expect([stopping.state, stopping.phase]).toEqual(['stopping', 'ramping_down']);
		await until(() => states.at(-1)?.state === 'finished');
		expect(states.at(-1)?.end?.outcome).toBe('operator_stop');
		expect(states.at(-1)?.end?.message).toMatch(/\(observer\)/);

		// and the abort, at once
		await c.request('random.reset');
		await c.request('random.start');
		await until(() => states.at(-1)?.phase === 'full_level');
		const aborted = await observer.request<RandomState>('random.abort');
		expect([aborted.state, aborted.end?.outcome]).toEqual(['finished', 'operator_abort']);
		expect(logs.some((x) => x.text.startsWith('abort requested by') && x.text.endsWith('(observer)'))).toBe(true);
		expect((await observer.request<RandomState>('random.abort')).end?.outcome).toBe('operator_abort');
		c.close();
		observer.close();
	});
});

describe('stations', () => {
	it('lists the simulator and the NI chassis, and never tags driver-simulated devices HARDWARE', async () => {
		const { c } = await setup();
		const list = await c.request<StationList>('station.list');
		expect(list.stations.map((s) => s.kind)).toEqual(['sim', 'ni']);
		expect(list.selected).toMatchObject({ kind: 'sim', class: 'sim', source: 'SIM' });
		const [, ni] = list.stations;
		expect(ni.devices.map((d) => [d.id, d.parent, d.timing_family, d.simulated])).toEqual([
			['cDAQ1', null, 'cDAQ1', true],
			['cDAQ1Mod1', 'cDAQ1', 'cDAQ1', true],
			['cDAQ1Mod2', 'cDAQ1', 'cDAQ1', true]
		]);
		const { selected } = await c.request<{ selected: StationList['selected'] }>('station.select', { kind: 'ni', devices: ['cDAQ1Mod1', 'cDAQ1Mod2'] });
		expect(selected).toMatchObject({ class: 'ni_simulated', source: 'SIM', inputs: 4, outputs: 4, timing_family: 'cDAQ1' });
		expect((await refusal(c.request('station.select', { kind: 'ni' }))).reason).toBe('bad_params');
		const absent = await refusal(c.request('station.select', { kind: 'ni', devices: ['cDAQ2Mod1'] }));
		expect([absent.reason, absent.detail?.missing]).toEqual(['devices_absent', ['cDAQ2Mod1']]);
		c.close();
	});

	it('names a missing driver, and tags physical devices HARDWARE', async () => {
		const none = await setup({ niInstalled: false });
		const list = await none.c.request<StationList>('station.list');
		expect(list.stations[1]).toMatchObject({ available: false, unavailable: { reason: 'ni_driver_unavailable' }, devices: [] });
		expect((await refusal(none.c.request('station.select', { kind: 'ni', devices: ['cDAQ1Mod1'] }))).reason).toBe('ni_driver_unavailable');
		none.c.close();

		const real = await setup({ niReal: true });
		const { selected } = await real.c.request<{ selected: StationList['selected'] }>('station.select', { kind: 'ni', devices: ['cDAQ1Mod1', 'cDAQ1Mod2'] });
		expect([selected.class, selected.source]).toEqual(['ni_real', 'HARDWARE']);
		real.c.close();
	});
});

describe('channel table', () => {
	it('starts single-ended, refuses a stale revision and an invalid table by name, and checks a dry run', async () => {
		const { c } = await setup();
		const table = await c.request<ChannelTable>('channels.get');
		expect(table.channels.map((r) => [r.index, r.role, r.interface])).toEqual([
			[1, 'control', 'single_ended'],
			[2, 'drive', 'single_ended']
		]);
		const rows: ChannelRow[] = table.channels.map((r) => ({ ...r }));
		const stale = await refusal(c.request('channels.set', { revision: 0, channels: rows }));
		expect([stale.reason, stale.detail]).toEqual(['stale_revision', { current: 1, provided: 0 }]);

		const broken = rows.map((r) => (r.index === 1 ? { ...r, sensitivity_mv_per_eu: 0 } : r));
		const invalid = await refusal(c.request('channels.set', { revision: 1, channels: broken }));
		expect(invalid.reason).toBe('table_invalid');
		expect((invalid.detail?.errors as { reason: string; channel: number }[]).map((e) => [e.reason, e.channel])).toEqual([['zero_sensitivity', 1]]);

		const dry = await c.request<ChannelsSetResult>('channels.set', { revision: 1, channels: broken, dry_run: true });
		expect([dry.revision, dry.report.errors.length]).toEqual([1, 1]);

		const monitor = { ...rows[0], index: 3, role: 'monitor' as const, description: 'Fixture corner' };
		const written = await c.request<ChannelsSetResult>('channels.set', { revision: 1, channels: [...rows, monitor] });
		expect(written.revision).toBe(2);
		c.close();
	});

	it('on NI hardware, every enabled row is bound to a channel of the selected devices', async () => {
		const { c } = await setup();
		await c.request('station.select', { kind: 'ni', devices: ['cDAQ1Mod1', 'cDAQ1Mod2'] });
		const unbound = await c.request<ChannelTable>('channels.get');
		expect(unbound.report.errors.map((e) => e.reason)).toEqual(['channel_unbound', 'channel_unbound']);
		const rows = unbound.channels;
		const bind = (row: ChannelRow, device: string, channel: string) => ({ ...row, physical: { device, channel } });
		const wrong = await c.request<ChannelsSetResult>('channels.set', {
			revision: unbound.revision,
			channels: [bind(rows[0], 'cDAQ1Mod2', 'ao0'), bind(rows[1], 'cDAQ1Mod2', 'ao0')],
			dry_run: true
		});
		expect(wrong.report.errors.map((e) => e.reason)).toEqual(['physical_channel_wrong_direction', 'physical_channel_shared']);
		const ok = await c.request<ChannelsSetResult>('channels.set', {
			revision: unbound.revision,
			channels: [bind(rows[0], 'cDAQ1Mod1', 'ai0'), bind(rows[1], 'cDAQ1Mod2', 'ao0')]
		});
		expect(ok.report.errors).toEqual([]);
		c.close();
	});
});

describe('Random configuration', () => {
	it('names every error, and derives the grid and the reference rms', async () => {
		const { c } = await setup();
		const bad = config({
			profile: { breakpoints: [{ freq_hz: 20, slope_db_oct: 3 }, { freq_hz: 10, psd_g2_hz: 0.04 }] },
			schedule: { mode: 'auto', levels: [{ db: 0, dwell_s: 1 }, { db: -6 }], full_level_s: 2 },
			control: { damping: 12 }
		});
		const e = await refusal(c.request('random.configure', { config: bad }));
		expect(e.reason).toBe('config_invalid');
		expect((e.detail?.errors as { reason: string }[]).map((x) => x.reason)).toEqual([
			'first_row_not_anchor',
			'frequency_not_ascending',
			'levels_not_ascending',
			'out_of_range'
		]);
		const dry = await c.request<RandomDryRunResult>('random.configure', { config: bad, dry_run: true });
		expect([dry.errors.length, dry.derived]).toEqual([4, null]);

		const ok = await c.request<RandomConfigureResult>('random.configure', { config: config() });
		expect(ok.state).toMatchObject({ state: 'configured', phase: 'idle', source: 'SIM', station: { kind: 'sim', class: 'sim' }, level_db: null, end: null });
		expect(ok.derived.grms_g).toBeCloseTo(6.06, 1);
		expect(ok.derived.grid).toEqual({ fs_hz: 8192, n_pts: 1024, df_hz: 8, lines: 248, loop_s: 0.25, actual: false });
		expect(ok.derived.control_channels).toEqual([1]);
		c.close();
	});
});

describe('a Random run', () => {
	it('identifies the system, holds for review, climbs the ladder and completes, live and stepped', async () => {
		const { c, states, lives, logs } = await setup();
		await c.request('random.configure', { config: config({ schedule: { mode: 'auto', levels: [{ db: -6, dwell_s: 1 }, { db: 0 }], full_level_s: 8 } }) });
		const armed = await c.request<RandomState>('random.arm');
		expect(armed).toMatchObject({ state: 'armed', phase: 'armed', clock: 'shared', grid: { actual: true }, feasibility: { verdict: 'not_evaluated' } });
		const started = await c.request<RandomState>('random.start');
		expect([started.state, started.phase]).toEqual(['running', 'sysid']);
		await until(() => states.at(-1)?.state === 'review');
		expect((await refusal(c.request('random.level', { delta_db: 1 }))).reason).toBe('manual_mode_required');
		await c.request('random.proceed');
		await until(() => states.at(-1)?.state === 'finished');

		const phases = [...new Set(states.map((s) => s.phase))];
		expect(phases).toEqual(['idle', 'armed', 'sysid', 'coherence_gate', 'awaiting_review', 'engaged', 'full_level', 'stopped']);
		const end = states.at(-1)?.end;
		expect(end).toMatchObject({ outcome: 'completed', trip: null, level_db: 0 });

		const live = lives.filter((l) => l.phase === 'full_level' && l.level_db === 0).at(-1) as RandomLive;
		expect([live.source, live.class]).toEqual(['SIM', 'sim']);
		const [control, drive, run] = live.views;
		expect(control.series.map((s) => s.name)).toEqual(['Control', 'Reference', 'Alarm high', 'Alarm low', 'Abort high', 'Abort low']);
		for (const s of [...control.series, ...drive.series]) {
			expect(s.shape).toBe('steps');
			expect(s.x.length).toBe(248);
			expect(s.edges?.length).toBe(249);
		}
		expect(run.view).toBe('reading');
		expect(live.lines.in_band).toBe(248);
		expect(Math.abs(20 * Math.log10((live.control_rms_g as number) / (live.reference_rms_g as number)))).toBeLessThan(1);
		expect(live.reference_rms_g).toBeCloseTo(6.06, 1);
		expect(live.channels).toEqual([expect.objectContaining({ index: 1, role: 'control', unit: 'g', overload: false })]);
		expect(lives.at(-1)?.state).toBe('finished');

		const log = await c.request<{ entries: LogEntry[]; next: number }>('random.log');
		expect(log.entries).toEqual(logs);
		expect(log.entries.map((e) => e.kind)).toContain('level');
		expect((await refusal(c.request('random.start'))).reason).toBe('run_finished');
		expect((await c.request<RandomState>('random.reset')).state).toBe('configured');
		c.close();
	});

	it('steps the level by hand, and stops with the shutdown ramp', async () => {
		const { c, states } = await setup();
		const manual = config({
			schedule: { mode: 'manual', levels: [{ db: -6, dwell_s: 1 }, { db: -3, dwell_s: 1 }, { db: 0 }], full_level_s: 2 },
			control: { review_after_sysid: false }
		});
		await c.request('random.configure', { config: manual });
		await c.request('random.start');
		await until(() => states.at(-1)?.phase === 'engaged');
		expect((await c.request<{ level_db: number }>('random.level', { delta_db: 1 })).level_db).toBe(-5);
		expect((await c.request<{ level_db: number }>('random.level', { delta_db: 9 })).level_db).toBe(0);
		await until(() => states.at(-1)?.phase === 'full_level');
		const rung = await refusal(c.request('random.level', { to_db: -4 }));
		expect([rung.reason, rung.detail?.levels_db]).toEqual(['not_a_rung', [-6, -3, 0]]);
		expect((await c.request<{ level_db: number }>('random.level', { to_db: -3 })).level_db).toBe(-3);
		const stopping = await c.request<RandomState>('random.stop');
		expect([stopping.state, stopping.phase]).toEqual(['stopping', 'ramping_down']);
		await until(() => states.at(-1)?.state === 'finished');
		expect(states.at(-1)?.end?.outcome).toBe('operator_stop');
		c.close();
	});

	it('aborts on the operator at once, and on a protection with its trip', async () => {
		const op = await setup();
		await op.c.request('random.configure', { config: config({ control: { review_after_sysid: false } }) });
		await op.c.request('random.start');
		await until(() => op.states.at(-1)?.phase === 'engaged');
		const aborted = await op.c.request<RandomState>('random.abort');
		expect([aborted.state, aborted.end?.outcome, aborted.end?.trip]).toEqual(['finished', 'operator_abort', null]);
		expect((await op.c.request<RandomState>('random.abort')).end?.outcome).toBe('operator_abort');
		op.c.close();

		const trip = await setup({ abortDemoAtS: 1 });
		await trip.c.request('random.configure', {
			config: config({ schedule: { mode: 'auto', levels: [{ db: 0 }], full_level_s: 120 }, control: { review_after_sysid: false } })
		});
		await trip.c.request('random.start');
		await until(() => trip.states.at(-1)?.state === 'finished', 8000);
		const end = trip.states.at(-1)?.end;
		expect(end?.outcome).toBe('aborted');
		expect(end?.trip).toMatchObject({ kind: 'envelope' });
		expect((end?.trip as { abort_lines: number }).abort_lines).toBeGreaterThan(10);
		expect(trip.lives.some((l) => l.alarms.some((a) => a.kind === 'envelope'))).toBe(true);
		trip.c.close();
	});

	it('is blocked by an exceeded feasibility until armed with an acknowledgement', async () => {
		const { c, logs } = await setup({ feasibilityExceeds: true });
		const shaker = { shaker: { name: 'Test shaker' }, moving_mass_lb: { armature: 20, slip_table: 0, drive_adapter: 2, fixturing: 15, test_article: 10 } };
		await c.request('random.configure', { config: config({ feasibility: shaker }) });
		expect((await refusal(c.request('random.arm', { override_ack: 'x' }))).reason).toBe('override_not_needed');
		const blocked = await refusal(c.request('random.arm'));
		expect(blocked.reason).toBe('feasibility_exceeded');
		expect((await c.request<RandomState>('random.state')).state).toBe('blocked');
		expect((await refusal(c.request('random.arm', { override_ack: '  ' }))).reason).toBe('bad_params');
		const armed = await c.request<RandomState>('random.arm', { override_ack: 'Fixture mass checked by hand; proceed.' });
		expect([armed.state, armed.feasibility?.verdict]).toEqual(['armed', 'exceeded']);
		expect(logs.some((e) => e.kind === 'override' && e.text.includes('Fixture mass checked by hand; proceed.'))).toBe(true);
		c.close();
	});

	it('runs on NI with the rate the hardware programs, and tags it HARDWARE only on physical devices', async () => {
		const { c } = await setup({ niReal: true });
		await c.request('station.select', { kind: 'ni', devices: ['cDAQ1Mod1', 'cDAQ1Mod2'] });
		const t = await c.request<ChannelTable>('channels.get');
		await c.request('channels.set', {
			revision: t.revision,
			channels: [
				{ ...t.channels[0], physical: { device: 'cDAQ1Mod1', channel: 'ai0' } },
				{ ...t.channels[1], physical: { device: 'cDAQ1Mod2', channel: 'ao0' } }
			]
		});
		const configured = await c.request<RandomConfigureResult>('random.configure', { config: config() });
		expect(configured.state.source).toBe('HARDWARE');
		const armed = await c.request<RandomState>('random.arm');
		expect(armed.grid).toMatchObject({ fs_hz: 8533.3333, actual: true });
		expect(armed.station).toEqual({
			kind: 'ni',
			class: 'ni_real',
			devices: [
				{ id: 'cDAQ1Mod1', serial: '01A2B3C4' },
				{ id: 'cDAQ1Mod2', serial: '01A2B3C5' }
			]
		});
		await c.request('random.abort');
		c.close();
	});

	it('safe-stops a run whose controller went away and nobody claimed, and lets the holder resume', async () => {
		const { server, c, claim, states } = await setup({ timeoutS: 0.3 });
		await c.request('random.configure', { config: config({ schedule: { mode: 'auto', levels: [{ db: 0 }], full_level_s: 600 }, control: { review_after_sysid: false } }) });
		await c.request('random.start');
		await until(() => states.at(-1)?.phase === 'full_level');

		const watcher = connect(server);
		const seen: RandomState[] = [];
		const seats: SessionChanged[] = [];
		watcher.on('random.state', (d) => seen.push(d as unknown as RandomState));
		watcher.on('session.changed', (d) => seats.push(d as unknown as SessionChanged));
		await watcher.whenOpen();

		c.close(); // the controlling client goes
		await until(() => seats.some((s) => s.controller?.present === false && s.orphan_s !== null));
		// the holder comes back in time with its lease
		const back = connect(server);
		await back.whenOpen();
		expect(await back.request<SessionClaimResult>('session.claim', { lease: claim.lease })).toMatchObject({ resumed: true, lease: claim.lease });

		back.close(); // and goes again, for good
		await until(() => seen.at(-1)?.state === 'finished', 6000);
		expect(seen.some((s) => s.phase === 'ramping_down')).toBe(true);
		expect(seen.at(-1)?.end?.outcome).toBe('safe_stop');
		watcher.close();
	});

	it('shares one run slot with sim.*', async () => {
		const { c } = await setup();
		await c.request('random.configure', { config: config() });
		await c.request('random.arm');
		const e = await refusal(c.request('sim.start', { profile: 'qual', level_db: 0, speed: 1 }));
		expect([e.code, e.reason]).toEqual(['refused', 'run_active']);
		await c.request('random.abort');
		const { run_id } = await c.request<{ run_id: string }>('sim.start', { profile: 'qual', level_db: 0, speed: 1 });
		await c.request('random.reset');
		expect((await refusal(c.request('random.start'))).reason).toBe('run_active');
		await c.request('sim.stop');
		expect(await c.request('sim.state')).toEqual({ state: 'stopped', run_id });
		c.close();
	});
});
