/**
 * The classical shock setup's pure rules: every field of a configuration survives the draft and
 * back, and a reload; the shell's defaults; what the editor names before anything is sent; where
 * the service's findings land; what the preview is asked for.
 */
import { describe, expect, it } from 'vitest';
import type { LawEntry } from '$lib/protocol/mimo';
import type { ChannelRow } from '$lib/protocol/types';
import type { ShockConfig } from '$lib/protocol/shock';
import { loadShockSetup, saveShockSetup } from './shockStore.svelte';
import {
	defaultShockSetup,
	fromShockConfig,
	localErrors,
	mergeShockSetup,
	previewParams,
	runChannels,
	shockByKey,
	shockItemKey,
	shockSectionOf,
	toShockConfig
} from './shockSetup';

const LAW: LawEntry = {
	id: 'fd_correct',
	world: 'shock_classical',
	builtin: true,
	name: 'frequency-domain correction',
	version: '0.1.0',
	author: 'Hertzforge',
	summary: '',
	sha256: null,
	check: { status: 'built_in', date: null, tool_version: null, cases: [], hardware: true, message: '' },
	params: [
		{ key: 'beta', label: 'Correction gain', help: '', kind: 'number', unit: '', min: 0.05, max: 1, step: 0.05, default: 0.5 },
		{ key: 'max_db_per_pulse', label: 'Largest change', help: '', kind: 'number', unit: 'dB', min: 1, max: 9, step: 0.5, default: 6 }
	],
	diagnostics: [],
	shapes: null,
	needs: null
};

/** Every field of the configuration, none at its default. */
const FULL: ShockConfig = {
	title: 'Trapezoid 30 g 6 ms',
	pulse: {
		shape: 'trapezoid',
		amplitude_g: 30,
		duration_ms: 6,
		polarity: 'negative',
		rise_frac: 0.15,
		fall_frac: 0.2,
		compensation: { shape: 'half_sine', pre_frac: 1.5, post_frac: 2.5, symmetric: false }
	},
	standard: { name: 'iec_60068_2_27', tolerance_abort: 'every_level', overshoot_abort: 0.35 },
	acquisition: { fs_hz: 25600, srs: { f_lo_hz: 10, f_hi_hz: 5000, bands_per_octave: 6, band_tol_db: 6 }, pvss: true },
	schedule: { mode: 'manual', levels_db: [-10, -5, -2, 0], pulses: [2, 1, 1, 4], start_db: -5, advance: 'count', max_pulses_per_level: 7 },
	spacing: { cooling_s: 3, settling_s: 1.5, compute_s: 0.75 },
	identification: { drive_vrms: 0.1, duration_s: 6, nfft: 4096, band_hz: [10, 4000], min_mean_coherence: 0.85, review: false },
	control: {
		law: { id: 'fd_correct', params: { beta: 0.25, max_db_per_pulse: 3 } },
		band_hz: [8, 4500],
		coherence_floor: 0.8,
		tikhonov_floor: 0.1,
		ref_floor_db: 40,
		min_band_energy: 0.85,
		law_budget: 0.3,
		law_misses: 4,
		max_pulses: 60,
		max_peak_rise_db: 8,
		max_bin_rise_db: 9,
		first_drive_ratio: 1.5,
		divergence_pulses: 4
	},
	limits: {
		drive_vpk_ceiling: 1.2,
		drive_vpk_abort: 1.6,
		peak_accel_g: 45,
		monitors: [
			{ channel: 3, unit: 'A', peak_ceiling: 40, kind: 'between_pulse' },
			{ channel: 4, unit: 'V', peak_ceiling: 120, kind: 'predictive' }
		]
	},
	feasibility: {
		shaker: { model: 'test shaker', rated_force_lbf: 10000, stroke_in: 2 },
		moving_mass_lb: { armature: 45, slip_table: 10, drive_adapter: 3, fixturing: 10, test_article: 12 },
		residual_velocity_ips: 0.2,
		residual_disp_in: 0.002
	},
	train: { count: 4, spacing_s: 8 },
	gunfire: { rounds_per_min: 900, rounds: 12 },
	capture: { auto_start: true, run_name: 'trap_run' },
	metadata: [
		{ key: 'run_number', label: 'Run Number', value: '17' },
		{ key: 'operator', label: 'Operator', value: 'A. Tester' }
	]
};

describe('a configuration through the draft and back', () => {
	it('keeps every field', () => {
		expect(toShockConfig(fromShockConfig(FULL), LAW)).toEqual(FULL);
	});

	it('keeps every field through a reload of the draft, an emptied field included', () => {
		const d = fromShockConfig(FULL);
		expect(toShockConfig(loadShockSetup(saveShockSetup(d)), LAW)).toEqual(FULL);
		d.pulse.amplitude_g = NaN;
		expect(Number.isNaN(loadShockSetup(saveShockSetup(d)).pulse.amplitude_g)).toBe(true);
	});

	it('sends no rise or fall for a shape that has none, and no feasibility or gunfire while off', () => {
		const d = fromShockConfig(FULL);
		d.pulse.shape = 'half_sine';
		d.feasibility.enabled = false;
		d.gunfire.enabled = false;
		const c = toShockConfig(d, LAW);
		expect([c.pulse.rise_frac, c.pulse.fall_frac, c.feasibility, c.gunfire]).toEqual([null, null, null, null]);
		// the draft still holds them for when they come back on
		expect([d.pulse.rise_frac, d.gunfire.rounds]).toEqual([0.15, 12]);
	});

	it('spreads one pulse count over every level, and takes the wire defaults for what is left out', () => {
		const lean: ShockConfig = {
			title: 'Lean',
			pulse: { shape: 'half_sine', amplitude_g: 20, duration_ms: 11 },
			schedule: { mode: 'auto', levels_db: [-6, 0], pulses: [2] },
			limits: { drive_vpk_ceiling: 0.5, drive_vpk_abort: 0.8 }
		};
		const d = fromShockConfig(lean);
		expect(d.schedule.levels).toEqual([
			{ db: -6, pulses: 2 },
			{ db: 0, pulses: 2 }
		]);
		expect(d.schedule.advance).toBe('count');
		expect(d.pulse.compensation).toEqual({ shape: 'rectangular', pre_frac: 4, post_frac: 4, symmetric: true });
		expect(d.standard).toEqual({ name: 'mil_std_810_516', tolerance_abort: 'full_level', overshoot_abort: null });
		expect(d.train.count).toBe(1);
	});

	it('sends only the parameters the law declares', () => {
		const d = fromShockConfig(FULL);
		d.law.params.stale = 1;
		expect(toShockConfig(d, LAW).control?.law?.params).toEqual({ beta: 0.25, max_db_per_pulse: 3 });
	});
});

describe("the shell's defaults", () => {
	it('rectangular shoulders four times the duration, advance on a pass, five pulses a level, no drive ceiling', () => {
		const d = defaultShockSetup();
		expect(d.pulse.compensation).toEqual({ shape: 'rectangular', pre_frac: 4, post_frac: 4, symmetric: true });
		expect([d.schedule.advance, d.schedule.max_pulses_per_level]).toEqual(['on_pass', 5]);
		expect(Number.isNaN(d.limits.drive_vpk_ceiling) && Number.isNaN(d.limits.drive_vpk_abort)).toBe(true);
		expect(d.law.id).toBe('fd_correct');
	});

	it('a saved draft missing sections is filled from the defaults', () => {
		const d = mergeShockSetup({ title: 'Old', pulse: { amplitude_g: 50 } as never, schedule: { levels: [] } as never });
		expect(d.title).toBe('Old');
		expect(d.pulse.amplitude_g).toBe(50);
		expect(d.pulse.shape).toBe('half_sine');
		expect(d.schedule.levels.length).toBe(5);
		expect(loadShockSetup('not json').title).toBe(defaultShockSetup().title);
	});
});

describe('what the editor names before anything is sent', () => {
	const rows = [{ index: 1, enabled: true, role: 'control' }, { index: 2, enabled: true, role: 'drive' }, { index: 3, enabled: false, role: 'monitor' }] as unknown as ChannelRow[];

	it('asks for the drive ceiling and the drive abort, above it', () => {
		const d = defaultShockSetup();
		const e = localErrors(d, rows);
		expect(e['limits.drive_vpk_ceiling']?.[0]).toMatch(/drive ceiling/);
		expect(e['limits.drive_vpk_abort']?.[0]).toMatch(/drive abort/);
		d.limits.drive_vpk_ceiling = 0.8;
		d.limits.drive_vpk_abort = 0.5;
		expect(localErrors(d, rows)['limits.drive_vpk_abort']).toEqual(['The drive abort is above the drive ceiling.']);
		d.limits.drive_vpk_abort = 1;
		expect(Object.keys(localErrors(d, rows))).toEqual([]);
	});

	it('names empty numbers, a monitor on a row that is not enabled, and a shaker record that does not read', () => {
		const d = fromShockConfig(FULL);
		d.pulse.duration_ms = NaN;
		d.schedule.levels[1].pulses = 1.5;
		d.limits.monitors[0].channel = 3;
		d.feasibility.shaker_text = '{ not json';
		d.title = ' ';
		const e = localErrors(d, rows);
		expect(Object.keys(e).sort()).toEqual(['feasibility.shaker', 'limits.monitors.0.channel', 'limits.monitors.1.channel', 'pulse.duration_ms', 'schedule.levels.1.pulses', 'title']);
	});

	it('the trapezoid edges count only for a trapezoid, the post shoulder only when not symmetric', () => {
		const d = defaultShockSetup();
		d.limits.drive_vpk_ceiling = 0.5;
		d.limits.drive_vpk_abort = 0.8;
		d.pulse.rise_frac = NaN;
		d.pulse.compensation.post_frac = NaN;
		expect(localErrors(d)).toEqual({});
		d.pulse.shape = 'trapezoid';
		d.pulse.compensation.symmetric = false;
		expect(Object.keys(localErrors(d)).sort()).toEqual(['pulse.compensation.post_frac', 'pulse.rise_frac']);
	});
});

describe("where the service's findings land", () => {
	it('on the level table, the law panel, the station, or the field as named', () => {
		expect(shockItemKey({ reason: 'levels_not_ascending', field: 'config.schedule.levels_db[2]' })).toBe('schedule.levels.2.db');
		expect(shockItemKey({ reason: 'out_of_range', field: 'config.schedule.pulses[1]' })).toBe('schedule.levels.1.pulses');
		expect(shockItemKey({ reason: 'pulses_length_mismatch', field: 'config.schedule.pulses' })).toBe('schedule.pulses');
		expect(shockItemKey({ reason: 'law_param_invalid', field: 'config.control.law.params.beta' })).toBe('law.params.beta');
		expect(shockItemKey({ reason: 'law_unknown', field: 'config.control.law.id' })).toBe('law.id');
		expect(shockItemKey({ reason: 'law_wrong_world', field: 'config.control.law' })).toBe('law.id');
		expect(shockItemKey({ reason: 'no_drive_channel' })).toBe('station.channels');
		expect(shockItemKey({ reason: 'out_of_range', field: 'config.limits.monitors[0].peak_ceiling' })).toBe('limits.monitors.0.peak_ceiling');
		expect(shockByKey([{ reason: 'quiet_window_exceeded', field: 'config.pulse.compensation', message: 'm' }])).toEqual({ 'pulse.compensation': ['m'] });
	});

	it('and each key has its section', () => {
		const sections = ['title', 'pulse.compensation', 'acquisition.srs.f_lo_hz', 'standard.overshoot_abort', 'spacing.cooling_s', 'schedule.levels.2.db', 'identification.nfft', 'law.params.beta', 'control.band_hz', 'limits.monitors.0.channel', 'feasibility.shaker', 'gunfire.rounds', 'metadata', 'station.channels'].map(shockSectionOf);
		expect(sections).toEqual(['pulse', 'pulse', 'standard', 'standard', 'schedule', 'schedule', 'identification', 'control', 'control', 'limits', 'limits', 'trains', 'trains', 'station']);
	});
});

describe('the preview request', () => {
	it('is the pulse, the standard and the rate, or nothing while a number is missing', () => {
		const d = defaultShockSetup();
		expect(previewParams(d)).toEqual({
			pulse: { shape: 'half_sine', amplitude_g: 20, duration_ms: 11, polarity: 'positive', rise_frac: null, fall_frac: null, compensation: { shape: 'rectangular', pre_frac: 4, post_frac: 4, symmetric: true } },
			standard: { name: 'mil_std_810_516', tolerance_abort: 'full_level', overshoot_abort: null },
			fs_hz: 51200
		});
		d.pulse.amplitude_g = NaN;
		expect(previewParams(d)).toBeNull();
	});

	it('the run uses the first enabled control row and the first enabled drive row', () => {
		const rows = [
			{ index: 4, enabled: false, role: 'control' },
			{ index: 5, enabled: true, role: 'control' },
			{ index: 6, enabled: true, role: 'drive' }
		] as unknown as ChannelRow[];
		const c = runChannels(rows);
		expect([c.control?.index, c.drive?.index]).toEqual([5, 6]);
	});
});
