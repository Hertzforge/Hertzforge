/**
 * The Sine setup draft: the configuration it sends, every field of it through the draft and back,
 * each test kind's wire form, the findings the editor names before anything is sent, and where a
 * service finding lands.
 */
import { describe, expect, it } from 'vitest';
import type { ChannelRow } from '$lib/protocol/types';
import type { LawEntry } from '$lib/protocol/mimo';
import type { SineConfig } from '$lib/protocol/sine';
import {
	bandOf,
	defaultSineSetup,
	fromSineConfig,
	localErrors,
	mergeSineSetup,
	newRow,
	sineByKey,
	sineItemKey,
	sineSectionOf,
	toSineConfig,
	validateParams,
	type SineDraft
} from './sineSetup';

const row = (index: number, role: ChannelRow['role'], enabled = true): ChannelRow => ({
	index,
	enabled,
	description: `ch ${index}`,
	role,
	physical: null,
	interface: 'single_ended',
	iepe_bias_v: null,
	units: role === 'drive' ? 'V' : 'g',
	sensitivity_mv_per_eu: 100,
	range_max: 10,
	polarity: 'positive',
	control_group: null,
	abort: { accel_g: null, velocity_ips: null, displacement_mil: null }
});
const ROWS = [row(1, 'control'), row(2, 'drive'), row(3, 'monitor'), row(4, 'control'), row(5, 'monitor', false)];

function ready(): SineDraft {
	const d = defaultSineSetup();
	d.limits.drive_vpk_ceiling = 0.5;
	d.limits.drive_vpk_abort = 0.8;
	return d;
}

const num = (key: string, max = 1) => ({ key, label: key, help: '', kind: 'number' as const, unit: '', min: 0, max, step: 0.1, default: 0.5 });
const LAW: LawEntry = {
	id: 'servo_blend',
	world: 'sine',
	builtin: true,
	name: 'Servo blend',
	version: '0.1.0',
	author: '',
	summary: '',
	sha256: null,
	check: { status: 'built_in', date: null, tool_version: null, cases: [], hardware: true, message: '' },
	params: [num('blend_w'), { key: 'compression', label: 'Compression', help: '', kind: 'choice', options: ['slow', 'fast', 'custom'], default: 'slow' }, num('ki', 20), num('max_rate_db_per_s', 240), num('kp', 2)],
	diagnostics: [],
	shapes: null,
	needs: null,
	live: ['blend_w']
};

/**
 * A draft with every field the wire carries moved off its default, so a field the conversion drops
 * shows. What the wire does not carry stays at the editor's default: the rate of the law not chosen,
 * and the bands a row without bands of its own keeps hidden (the profile's).
 */
function everything(): SineDraft {
	const d = ready();
	d.title = 'Bracket survey';
	d.profile.label = 'Survey';
	d.profile.alarm_db = [1, 2];
	d.profile.abort_db = [2.5, 3.5];
	const plain = (f: number, q: 'velocity_ips' | 'accel_g', v: number) => ({ ...newRow(f, q, v), alarm_db: [1, 2] as [number, number], abort_db: [2.5, 3.5] as [number, number] });
	d.profile.rows = [{ ...newRow(10, 'disp_pp_in', 0.05), own_bands: true, alarm_db: [2, 1], abort_db: [4, 3] }, plain(30, 'velocity_ips', 2), plain(2500, 'accel_g', 2.5)];
	d.test.band_hz = [10, 2400];
	d.test.sweep = { law: 'linear', oct_per_min: 2, hz_per_s: 7, start_hz: 100, direction: 'down', end_by: 'both', sweeps: 6, duration_s: 1200 };
	d.acquisition = { fs_hz: 20480, tick_ms: 12 };
	d.schedule = { mode: 'manual', levels: [-18, -9, 0], start_db: -9, per_level: { by: 'seconds', n: 40 }, full_level: { by: 'sweeps', n: 3 } };
	d.identification = { level_db: -24, gate_coherence: 0.6, trust_coherence: 0.7, trust_h_floor_frac: 0.02, review: false };
	d.law = { id: 'servo_blend', params: { blend_w: 0.3, compression: 'custom', ki: 4, max_rate_db_per_s: 30, kp: 0.4 } };
	d.control = { composite: 'max', weights: [{ channel: 1, weight: 2 }, { channel: 4, weight: 0.5 }], tracking: { kind: 'fixed', hz: 5, pct: 10, max_hz: 100 }, law_budget: 0.3, law_misses: 5, max_step_db: 2, max_rise_db_per_s: 120, divergence_s: 2, fault_debounce_ticks: 80 };
	d.limits = {
		drive_vpk_ceiling: 0.4,
		drive_vpk_abort: 0.6,
		kinematic_monitor: false,
		channels: [{ channel: 3, shared_name: 'fixture', profile: { label: 'Fixture bound', rows: [newRow(10, 'accel_g', 3), newRow(2500, 'accel_g', 4)], alarm_db: [1.5, 1.5], abort_db: [3, 3] } }],
		notch: { drive_floor: 0.02, smoothing_ticks: 5, release_ramp_db_per_s: 10 }
	};
	d.cola = { enabled: true, volts_pk: 0.2, waveform: 'square' };
	d.feasibility = { enabled: true, shaker_text: '{"name":"Bench shaker"}', moving_mass_lb: { armature: 45, slip_table: 5, drive_adapter: 3, fixturing: 10, test_article: 12 } };
	d.capture = { rule: 'first_seconds', first_s: 15, level_settle_s: 2, periodic_save_s: 60, run_name: 'survey' };
	d.metadata = [
		{ key: 'run_number', label: 'Run Number', value: '7' },
		{ key: 'operator', label: '', value: 'K' }
	];
	return d;
}

describe('the Sine setup draft', () => {
	it('asks for the drive ceiling and abort, and starts from a setup the wire takes', () => {
		const d = defaultSineSetup();
		expect(Object.keys(localErrors(d))).toEqual(['limits.drive_vpk_ceiling', 'limits.drive_vpk_abort']);
		const c = toSineConfig(ready(), null);
		expect(c.test).toEqual({ kind: 'sweep', band_hz: [20, 2000], rate: { law: 'log', oct_per_min: 2 }, start_hz: 20, direction: 'up', end: { sweeps: 4, duration_s: null } });
		expect(c.schedule).toEqual({ mode: 'auto', levels_db: [-12, -6, 0], start_db: -12, per_level: { sweeps: 1 }, full_level: { sweeps: 2 } });
		expect(c.acquisition).toEqual({ fs_hz: null, tick_ms: 10 });
		expect(c.capture).toEqual({ rule: 'off', first_s: null, level_settle_s: 1, periodic_save_s: null, run_name: 'sine' });
		expect(c.feasibility).toBeNull();
		expect(localErrors(ready())).toEqual({});
	});

	it('keeps every field through the configuration and back', () => {
		const d = everything();
		const c = toSineConfig(d, LAW);
		expect(fromSineConfig(c)).toEqual({ ...d, feasibility: { ...d.feasibility, shaker_text: JSON.stringify({ name: 'Bench shaker' }, null, 2) } });
		expect(toSineConfig(fromSineConfig(c), LAW)).toEqual(c);
		// a law's parameters go only with the law that declares them
		expect(toSineConfig(d, null).control?.law).toEqual({ id: 'servo_blend', params: {} });
	});

	it('sends a row its own bands only when it has them, and a limit channel its bound in the same form', () => {
		const c = toSineConfig(everything(), null);
		expect(c.profile.rows[0]).toEqual({ freq_hz: 10, quantity: 'disp_pp_in', value: 0.05, alarm_db: [2, 1], abort_db: [4, 3] });
		expect(c.profile.rows[1]).toEqual({ freq_hz: 30, quantity: 'velocity_ips', value: 2 });
		expect(c.limits.channels?.[0]).toEqual({ channel: 3, shared_name: 'fixture', profile: { label: 'Fixture bound', rows: [{ freq_hz: 10, quantity: 'accel_g', value: 3 }, { freq_hz: 2500, quantity: 'accel_g', value: 4 }], alarm_db: [1.5, 1.5], abort_db: [3, 3] } });
		const plain = ready();
		plain.profile.label = '';
		expect('label' in toSineConfig(plain, null).profile).toBe(false);
	});

	it("sends each test kind's own settings, and keeps the others for a change back", () => {
		const d = ready();
		d.test.sweep.end_by = 'duration';
		d.test.sweep.start_hz = null;
		expect(toSineConfig(d, null).test).toMatchObject({ start_hz: 20, end: { sweeps: null, duration_s: 900 } });
		d.test.kind = 'dwell';
		d.test.dwell = { freq_hz: 250, by: 'cycles', seconds: 60, cycles: 12000 };
		expect(toSineConfig(d, null).test).toEqual({ kind: 'dwell', freq_hz: 250, end: { cycles: 12000 } });
		expect(bandOf(d)).toEqual([250, 250]);
		d.test.kind = 'step';
		expect(toSineConfig(d, null).test).toEqual({ kind: 'step', band_hz: [20, 2000], steps_per_octave: 3, spacing: 'log', dwell: null });
		d.test.step.by = 'seconds';
		expect(toSineConfig(d, null).test).toMatchObject({ dwell: { seconds: 10 } });
		d.test.kind = 'sweep';
		expect(toSineConfig(d, null).test.kind).toBe('sweep');
		expect(d.test.dwell.cycles).toBe(12000);
	});

	it('keeps a resonance search another screen sent, as it came', () => {
		const base = ready();
		base.schedule.per_level = { by: 'seconds', n: 20 };
		base.schedule.full_level = { by: 'seconds', n: 60 };
		const c: SineConfig = { ...toSineConfig(base, null), test: { kind: 'rstd', band_hz: [100, 800], level_db: -20, rate: { law: 'log', oct_per_min: 1 }, tracking_bw_hz: 2, settle_k: 5, dwell_mode: 'phase_tracked', dwell: { seconds: 30 } } };
		const d = fromSineConfig(c);
		expect(d.test.kind).toBe('rstd');
		expect(toSineConfig(d, null).test).toEqual(c.test);
		expect(localErrors(d)).toEqual({});
	});

	it("sends a weight only for one of the table's enabled control rows", () => {
		const d = everything();
		d.control.weights.push({ channel: 3, weight: 9 }, { channel: 5, weight: 9 });
		expect(toSineConfig(d, null, ROWS).control?.weights).toEqual([
			{ channel: 1, weight: 2 },
			{ channel: 4, weight: 0.5 }
		]);
		expect(toSineConfig(d, null).control?.weights).toHaveLength(4);
	});

	it('names what it can before anything is sent', () => {
		const d = ready();
		d.title = ' ';
		d.profile.rows = [newRow(20, 'accel_g', NaN)];
		d.test.kind = 'dwell';
		d.limits.channels = [
			{ channel: 1, shared_name: '', profile: { label: '', rows: [newRow(10, 'accel_g', 1), newRow(2000, 'accel_g', 1)], alarm_db: [1.5, 1.5], abort_db: [3, 3] } },
			{ channel: 5, shared_name: '', profile: { label: '', rows: [newRow(10, 'accel_g', 1), newRow(2000, 'accel_g', 1)], alarm_db: [1.5, 1.5], abort_db: [3, 3] } }
		];
		d.capture.rule = 'entire_test';
		d.capture.run_name = '';
		d.limits.drive_vpk_abort = 0.4;
		const e = localErrors(d, ROWS);
		expect(e.title).toEqual(['Give the test a title.']);
		expect(e['profile.rows']).toEqual(['Give at least two rows.']);
		expect(e['profile.rows.0.value']).toEqual(['Enter a number.']);
		expect(e['schedule.per_level']).toEqual(['Sweeps count in a sweep test only; give seconds.']);
		expect(e['limits.channels.0.channel']).toEqual(['Channel 1 is a control channel; a limit channel is another input.']);
		expect(e['limits.channels.1.channel']).toEqual(['Channel 5 is not an enabled input of the channel table.']);
		expect(e['limits.drive_vpk_abort']).toEqual(['The drive abort is at least the drive ceiling.']);
		expect(e['capture.run_name']).toEqual(['Name the run folder, or turn capture off.']);
	});

	it('lands each service finding on its field and section', () => {
		const cases: [Parameters<typeof sineItemKey>[0], string, string][] = [
			[{ reason: 'value_not_positive', field: 'config.profile.rows[2].value', row: 2 }, 'profile.rows', 'profile'],
			[{ reason: 'quantity_ambiguous', row: 1 }, 'profile.rows', 'profile'],
			[{ reason: 'profile_not_covering', field: 'profile' }, 'profile.rows', 'profile'],
			[{ reason: 'end_before_full_level', field: 'test.end.sweeps' }, 'test.end.sweeps', 'test'],
			[{ reason: 'band_above_grid', field: 'config.test.band_hz[1]' }, 'test.band_hz.1', 'test'],
			[{ reason: 'tracking_lag', field: 'control.tracking' }, 'control.tracking', 'control'],
			[{ reason: 'tick_out_of_range', field: 'acquisition.tick_ms' }, 'acquisition.tick_ms', 'control'],
			[{ reason: 'out_of_range', field: 'acquisition.fs_hz' }, 'acquisition.fs_hz', 'test'],
			[{ reason: 'levels_not_ascending', field: 'schedule.levels_db[2]' }, 'schedule.levels.2', 'schedule'],
			[{ reason: 'out_of_range', field: 'identification.gate_coherence' }, 'identification.gate_coherence', 'schedule'],
			[{ reason: 'out_of_range', field: 'identification.trust_coherence' }, 'identification.trust_coherence', 'control'],
			[{ reason: 'law_param_invalid', field: 'control.law.params.blend_w' }, 'law.params.blend_w', 'control'],
			[{ reason: 'law_unknown', field: 'control.law.id' }, 'law.id', 'control'],
			[{ reason: 'weight_unknown_channel', field: 'control.weights[0].channel' }, 'control.weights', 'control'],
			[{ reason: 'limit_on_control_channel', field: 'limits.channels[0].channel' }, 'limits.channels.0.channel', 'limits'],
			[{ reason: 'value_not_positive', field: 'limits.channels[1].profile.rows[0].value' }, 'limits.channels.1.profile', 'limits'],
			[{ reason: 'cola_needs_two_outputs', field: 'cola.enabled' }, 'cola.enabled', 'limits'],
			[{ reason: 'out_of_range', field: 'feasibility.shaker' }, 'feasibility.shaker', 'limits'],
			[{ reason: 'out_of_range', field: 'capture.run_name' }, 'capture.run_name', 'capture'],
			[{ reason: 'no_control_channel' }, 'station.channels', 'station']
		];
		for (const [it, key, section] of cases) {
			expect([sineItemKey(it), sineSectionOf(sineItemKey(it))]).toEqual([key, section]);
		}
		expect(sineByKey([{ reason: 'out_of_range', field: 'test.rate.oct_per_min', message: 'a' }, { reason: 'tracking_lag', field: 'test.rate', message: 'a' }])).toEqual({ 'test.rate': ['a'] });
	});

	it("asks the service to read the profile only when it holds numbers, over the test's band", () => {
		const d = ready();
		expect(validateParams(d)).toMatchObject({ band_hz: [20, 2000], feasibility: null });
		d.feasibility = { ...d.feasibility, enabled: true, shaker_text: '{"name":"x"}' };
		expect(validateParams(d)?.feasibility).toEqual({ shaker: { name: 'x' }, moving_mass_lb: d.feasibility.moving_mass_lb });
		d.test.kind = 'dwell';
		expect(validateParams(d)?.band_hz).toBeNull();
		d.profile.rows[1].value = NaN;
		expect(validateParams(d)).toBeNull();
	});

	it('fills a saved draft from the defaults where it is missing, and keeps what it holds', () => {
		const saved = { title: 'Mine', test: { kind: 'dwell', dwell: { freq_hz: 80 } }, limits: { drive_vpk_ceiling: 1, channels: [{ channel: 3 }] } } as unknown as Partial<SineDraft>;
		const d = mergeSineSetup(saved);
		expect([d.title, d.test.kind, d.test.dwell.freq_hz, d.test.dwell.by, d.limits.drive_vpk_ceiling]).toEqual(['Mine', 'dwell', 80, 'seconds', 1]);
		expect(d.limits.channels[0].profile.rows).toHaveLength(2);
		expect(d.schedule).toEqual(defaultSineSetup().schedule);
		expect(mergeSineSetup(null)).toEqual(defaultSineSetup());
	});
});
