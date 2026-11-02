/** The pure parts of Acquire & Control: setup conversion, run bar rules, table helpers, words. */
import { describe, expect, it } from 'vitest';
import { RequestError } from '$lib/protocol/client.svelte';
import type { ChannelRow, NiDevice, RandomState, RefusalReason } from '$lib/protocol/types';
import { primaryAction, runActions, type RunContext } from './actions';
import { byField, defaultSetup, fromConfig, itemKey, localErrors, toConfig } from './setup';
import { bindUnbound, newRow, rowFindings, tableChanges } from './table';
import { clock, db, outcomeTone, overloadNote, RANGE_MEANING, refusalTitle, toRefusal, tripFacts } from './words';

function state(over: Partial<RandomState>): RandomState {
	return {
		run_id: 'r-0001',
		state: 'configured',
		phase: 'idle',
		title: 'T',
		source: 'SIM',
		station: { kind: 'sim', class: 'sim', devices: [] },
		grid: null,
		clock: null,
		mode: 'auto',
		levels_db: [-6, 0],
		level_db: null,
		full_level_s: 10,
		elapsed_s: null,
		at_level_s: null,
		remaining_at_level_s: null,
		alarm: false,
		feasibility: null,
		end: null,
		...over
	};
}

const ctx = (over: Partial<RunContext>): RunContext => ({
	connected: true,
	role: 'controller',
	run: null,
	setup: 'none',
	setupErrors: 0,
	busy: null,
	pending: null,
	...over
});

describe('setup', () => {
	it('sends anchors and slopes as the protocol has them, with row bands only where set', () => {
		const d = defaultSetup();
		d.breakpoints[2].alarm_db = [2, 2];
		const c = toConfig(d);
		expect(c.profile.breakpoints[0]).toEqual({ freq_hz: 20, psd_g2_hz: 0.01 });
		expect(c.profile.breakpoints[1]).toEqual({ freq_hz: 80, slope_db_oct: 3 });
		expect(c.profile.breakpoints[2]).toEqual({ freq_hz: 350, psd_g2_hz: 0.04, alarm_db: [2, 2] });
		// The last level holds for full_level_s and carries no dwell.
		expect(c.schedule.levels.at(-1)).toEqual({ db: 0 });
		expect(c.schedule.levels[0]).toEqual({ db: -12, dwell_s: 30 });
		expect(c.feasibility).toBeNull();
		expect(c.control?.shutdown_ramp_s).toBe(3);
		expect(c.control?.level_ramp_s).toBe(3);
		expect(toConfig(fromConfig(c))).toEqual(c);
	});

	it('locates every finding the way the editor keys its fields', () => {
		expect(itemKey({ reason: 'frequency_not_ascending', message: '', row: 2 })).toBe('profile.breakpoints.2.freq_hz');
		expect(itemKey({ reason: 'first_row_not_anchor', message: '', row: 0 })).toBe('profile.breakpoints.0.kind');
		expect(itemKey({ reason: 'out_of_range', message: '', field: 'control.damping' })).toBe('control.damping');
		expect(itemKey({ reason: 'zero_sensitivity', message: '', channel: 3 })).toBeNull();
		expect(byField([{ reason: 'dwell_missing', message: 'm', field: 'schedule.levels.0' }])).toEqual({ 'schedule.levels.0': ['m'] });
	});

	it('names a field holding no number, and an unreadable shaker record', () => {
		const d = defaultSetup();
		d.acquisition.n_avg = 2.5;
		d.feasibility.enabled = true;
		d.feasibility.shaker_text = 'not json';
		const e = localErrors(d);
		expect(e['acquisition.n_avg']).toEqual(['Enter a whole number.']);
		expect(e['feasibility.shaker']?.[0]).toMatch(/does not read/);
		expect(localErrors(defaultSetup())).toEqual({});
	});
});

describe('run bar', () => {
	it('offers the next step and says why anything else waits', () => {
		expect(primaryAction(ctx({}))).toBe('apply');
		expect(primaryAction(ctx({ run: state({}), setup: 'applied' }))).toBe('start');
		expect(primaryAction(ctx({ run: state({ state: 'blocked', phase: 'feasibility_blocked' }), setup: 'applied' }))).toBe('override');
		expect(primaryAction(ctx({ run: state({ state: 'review', phase: 'awaiting_review' }) }))).toBe('proceed');
		expect(primaryAction(ctx({ run: state({ state: 'running', phase: 'engaged' }) }))).toBeNull();
		expect(primaryAction(ctx({ run: state({ state: 'finished', phase: 'stopped' }), setup: 'applied' }))).toBe('reset');
		expect(primaryAction(ctx({ run: state({ state: 'finished', phase: 'stopped' }), setup: 'changed' }))).toBe('apply');

		const a = runActions(ctx({ setupErrors: 2 }));
		expect(a.apply).toEqual({ visible: true, enabled: false, reason: 'Fix the 2 errors in the setup first.' });
		expect(runActions(ctx({ pending: 'Apply the channel table edits, or discard them, first.' })).apply.reason).toMatch(/channel table/);
		expect(runActions(ctx({ run: state({}), setup: 'changed' })).start.reason).toMatch(/changed since it was applied/);
		expect(runActions(ctx({ connected: false, run: state({}) })).start.reason).toBe('The control service is not connected.');
	});

	it('lets an observer stop and abort, and nothing else', () => {
		const running = runActions(ctx({ role: 'observer', run: state({ state: 'running', phase: 'engaged', mode: 'manual' }), busy: 'Level Up' }));
		expect(running.stop.enabled).toBe(true);
		expect(running.abort.enabled).toBe(true);
		expect(running.level.reason).toBe('Take control to change the level.');
		expect(running.mode.enabled).toBe(false);
		expect(running.apply.visible).toBe(false);
		const stopping = runActions(ctx({ run: state({ state: 'stopping', phase: 'ramping_down' }) }));
		expect(stopping.stop.enabled).toBe(false);
		expect(stopping.abort.enabled).toBe(true);
		const idle = runActions(ctx({ run: state({}) }));
		expect(idle.abort.visible).toBe(false);
	});

	it('steps the level only on the manual ladder while driving', () => {
		const auto = runActions(ctx({ run: state({ state: 'running', phase: 'engaged', mode: 'auto' }) }));
		expect(auto.level.reason).toMatch(/manual/);
		const sysid = runActions(ctx({ run: state({ state: 'running', phase: 'sysid', mode: 'manual' }) }));
		expect(sysid.level.reason).toMatch(/once the run is driving/);
		const manual = runActions(ctx({ run: state({ state: 'running', phase: 'full_level', mode: 'manual' }) }));
		expect(manual.level.enabled).toBe(true);
	});
});

describe('channel table', () => {
	const devices: NiDevice[] = [
		{ id: 'c1', product: 'chassis', serial: '', simulated: true, parent: null, slot: null, timing_family: 'c1', inputs: [], outputs: [] },
		{ id: 'c1m1', product: 'in', serial: '', simulated: true, parent: 'c1', slot: 1, timing_family: 'c1', inputs: [{ id: 'ai0', label: 'ai0' }, { id: 'ai1', label: 'ai1' }], outputs: [] },
		{ id: 'c1m2', product: 'out', serial: '', simulated: true, parent: 'c1', slot: 2, timing_family: 'c1', inputs: [], outputs: [{ id: 'ao0', label: 'ao0' }] }
	];

	it('makes new rows with IEPE off and binds unbound rows to free channels by direction', () => {
		const rows: ChannelRow[] = [newRow(1, 'control'), newRow(2, 'drive'), newRow(3, 'monitor'), newRow(4, 'monitor')];
		rows[2].physical = { device: 'c1m1', channel: 'ai0' };
		expect(rows.every((r) => r.interface === 'single_ended' && r.iepe_bias_v === null)).toBe(true);
		const { rows: out, bound } = bindUnbound(rows, devices, ['c1m1', 'c1m2']);
		expect(bound).toBe(2);
		expect(out[0].physical).toEqual({ device: 'c1m1', channel: 'ai1' });
		expect(out[1].physical).toEqual({ device: 'c1m2', channel: 'ao0' });
		expect(out[3].physical).toBeNull();
	});

	it('puts each finding on its row and cell, and says what changed between revisions', () => {
		const f = rowFindings({
			errors: [{ reason: 'channel_unbound', message: 'unbound', channel: 1 }, { reason: 'no_channel_enabled', message: 'none', channel: null }],
			warnings: [{ reason: 'iepe_without_bias', message: 'bias', channel: 1 }]
		});
		expect(f.rows.get(1)?.map((x) => [x.level, x.cell])).toEqual([
			['error', 'physical'],
			['warning', 'iepe_bias_v']
		]);
		expect(f.table.map((x) => x.reason)).toEqual(['no_channel_enabled']);

		const before = [newRow(1, 'control'), newRow(2, 'drive')];
		const after = [{ ...newRow(1, 'control'), sensitivity_mv_per_eu: 10, abort: { accel_g: 30, velocity_ips: null, displacement_mil: null } }, newRow(3, 'monitor')];
		expect(tableChanges(before, after)).toEqual(['Channel 1: sensitivity, abort limits changed.', 'Channel 3 added (Monitor).', 'Channel 2 removed (Drive).']);
	});
});

describe('words', () => {
	it('says Overload only while an input is in it; otherwise the word is only defined', () => {
		expect(overloadNote([])).toBeNull();
		expect(overloadNote([3])).toBe('Overload: channel 3 reached the top of its range during this run (90 % of it in the run, or 98 % at the station).');
		expect(overloadNote([1, 3])).toMatch(/^Overload: channels 1, 3 reached the top of their range/);
		expect(RANGE_MEANING).toMatch(/^OK, or Overload once the input reaches the top of its range/);
	});

	it('gives every refusal reason a heading, and keeps the service message verbatim', () => {
		const reasons: RefusalReason[] = [
			'bad_params', 'not_controller', 'controller_held', 'run_active', 'not_configured', 'run_finished', 'wrong_state',
			'manual_mode_required', 'not_a_rung', 'feasibility_exceeded', 'override_not_needed', 'arm_refused',
			'ni_driver_unavailable', 'no_ni_devices', 'scan_failed', 'devices_absent', 'mixed_simulated_and_real',
			'mixed_timing_families', 'not_enough_channels', 'hardware_busy', 'stale_revision', 'table_invalid',
			'config_invalid', 'shaker_record_invalid'
		];
		for (const r of reasons) expect(refusalTitle(r)).not.toBe('Not Done');
		const r = toRefusal('Use Station', new RequestError('refused', 'Selected device not present: cDAQ2.', 'station.select', 'devices_absent', { missing: ['cDAQ2'], found: ['cDAQ1'] }));
		expect(r.title).toBe('Devices Not Found');
		expect(r.message).toBe('Selected device not present: cDAQ2.');
		expect(r.facts).toEqual(['Missing: cDAQ2', 'Found: cDAQ1']);
		expect(toRefusal('Arm', new RequestError('timeout', 'random.arm got no response within 10 s.', 'random.arm')).title).toBe('No Answer in Time');
	});

	it('shows a trip by its axis and facts, and every outcome but two as an abort', () => {
		const facts = tripFacts({ kind: 'kinematic', channel: 2, axis: 'displacement', value: 41.2, limit: 40, at_level_db: 0, at_loop: 99 });
		expect(facts.slice(0, 4)).toEqual([
			{ label: 'Axis', value: 'Displacement' },
			{ label: 'Channel', value: '2' },
			{ label: 'Value', value: '41.2 mil' },
			{ label: 'Limit', value: '40 mil' }
		]);
		expect(outcomeTone('completed')).toBe('pass');
		expect(outcomeTone('operator_stop')).toBe('info');
		for (const o of ['operator_abort', 'safe_stop', 'aborted', 'held', 'fault'] as const) expect(outcomeTone(o)).toBe('fail');
		expect(db(-6)).toBe('-6.0 dB');
		expect(db(0)).toBe('0.0 dB');
		expect(db(3.04)).toBe('+3.0 dB');
		expect(clock(42)).toBe('42.0 s');
		expect(clock(725)).toBe('12:05');
	});
});
