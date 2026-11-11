/**
 * The parts table's words against rows shaped as the control service sends them on a combined run:
 * a follower whose row carries no direction and its leader's rate under the wrong unit, a
 * narrowband sized as a share of its center whose rate field is 0, the floor early in a run, and
 * parts that are off or have ended.
 */

import { describe, expect, it } from 'vitest';
import type { MixedConfig, MixedPartRow } from '$lib/protocol/mixed';
import { partDetail, partDrive, partFreq, partMotion, partStateWord } from './mixedWords';

const base: MixedPartRow = { id: 'x', kind: 'tone', name: 'x', enabled: true, on: true, scale: 1, freq_hz: null, direction: null, sweeps: null, rate_oct_per_min: null, rate_hz_per_s: null, width_hz: null, unit: 'g', target: 0.0846, measured: 0.083, error_db: null, alarm: false, drive_vpk: 0.0679, drive_vrms: null, gain_db: null, settled: false, limiting: false, dof: null, lines: null, law: { misses: 0, capped: 0 } };
const row = (over: Partial<MixedPartRow>): MixedPartRow => ({ ...base, ...over });

// The rows of the real combined run at parts time 6.7 s (view.rs: a follower's direction is null and
// its rate is written under its root's unit; a narrowband's rate stays 0; the floor's dof is the
// fewest of any clean line).
const floor = row({ id: 'floor', kind: 'floor', name: 'Floor', freq_hz: null, unit: 'g', target: 0.125, measured: 0.125, error_db: 0.01, drive_vpk: null, drive_vrms: 0.163, dof: 2, settled: true, lines: { in_band: 3961, clean: 3928, alarm: 0, abort: 0 } });
const rotor = row({ id: 't2', name: 'Rotor', freq_hz: 409.5, direction: 'up', sweeps: { done: 0, of: null }, rate_oct_per_min: 0.3, drive_vpk: 0.062 });
const second = row({ id: 't3', name: 'Rotor 2nd', freq_hz: 818.9, rate_oct_per_min: 2.84, drive_vpk: 0.0193, target: 0.0282 });
const third = row({ id: 't4', name: 'Rotor 3rd', freq_hz: 1228.4, rate_oct_per_min: 4.26, drive_vpk: 0.0356, target: 0.0282 });
const gear = row({ id: 'b1', kind: 'narrowband', name: 'Gear mesh', freq_hz: 250, unit: 'g²/Hz', target: 0.0000397, measured: null, drive_vpk: null, drive_vrms: 0, width_hz: 20, dof: 0, gain_db: 0, law: null });
const band = row({ id: 'b2', kind: 'narrowband', name: 'Rotor band', freq_hz: 1317.5, direction: 'up', sweeps: { done: 0, of: 2 }, rate_hz_per_s: 0, unit: 'g²/Hz', target: 0.0000397, measured: null, drive_vpk: null, drive_vrms: 0, width_hz: 65.9, dof: 0, gain_db: 0, law: null });
const rows = [floor, rotor, second, third, gear, band];

const config = {
	parts: [
		{ id: 'floor', kind: 'floor', name: 'Floor', breakpoints: [] },
		{ id: 't2', kind: 'tone', name: 'Rotor', schedule: { kind: 'sweep', band_hz: [400, 600], rate: { law: 'log', oct_per_min: 0.3 }, start_hz: 400, direction: 'up', end: null }, amplitude_g: 0.3 },
		{ id: 't3', kind: 'tone', name: 'Rotor 2nd', schedule: { kind: 'follow', of: 't2', ratio: 2 }, amplitude_g: 0.1 },
		{ id: 't4', kind: 'tone', name: 'Rotor 3rd', schedule: { kind: 'follow', of: 't3', ratio: 1.5 }, amplitude_g: 0.1 },
		{ id: 'b1', kind: 'narrowband', name: 'Gear mesh', schedule: { kind: 'fixed', freq_hz: 250, end: null }, level_g2_hz: 0.0005, width_hz: 20, window: { on_s: 0, off_s: 30 } },
		{ id: 'b2', kind: 'narrowband', name: 'Rotor band', schedule: { kind: 'sweep', band_hz: [1300, 1600], rate: { law: 'linear', hz_per_s: 5 }, start_hz: 1300, direction: 'up', end: { sweeps: 2 } }, level_g2_hz: 0.0005, width_share: 0.05 },
		{ id: 'b3', kind: 'narrowband', name: 'Late band', schedule: { kind: 'follow', of: 'b2', ratio: 0.5 }, level_g2_hz: 0.0005, width_hz: 10, window: { on_s: 40, off_s: null } }
	],
	acquisition: { fs_hz: 12800, n_pts: 16384, n_avg: 4 },
	schedule: { mode: 'auto', rungs: [] },
	limits: {}
} as unknown as MixedConfig;
const ctx = { rows, config, parts_s: 6.7 };

describe('a follower', () => {
	it('says what it follows and at what ratio, from the setup when the row says nothing', () => {
		expect(partMotion(second, false, ctx)).toBe('Follows Rotor at 2×, sweeping up at 0.3 oct/min');
	});

	it('takes a chain of followers to the sweep at its root, the rate in the root sweep\'s unit', () => {
		expect(partMotion(third, false, ctx)).toBe('Follows Rotor 2nd at 1.5×, sweeping up at 0.3 oct/min');
	});

	it("uses the row's own leader and ratio when the service sends them", () => {
		const r = { ...second, follows: { of: 't2', ratio: 2.5 } } as MixedPartRow;
		expect(partMotion(r, false, ctx)).toBe('Follows Rotor at 2.5×, sweeping up at 0.3 oct/min');
	});

	it('of a part standing still, says only what it follows', () => {
		const cfg = { ...config, parts: config.parts.map((p) => (p.id === 't2' ? { ...p, schedule: { kind: 'fixed', freq_hz: 400 } } : p)) } as MixedConfig;
		expect(partMotion(second, false, { rows: rows.map((r) => (r.id === 't2' ? { ...rotor, direction: null, sweeps: null, rate_oct_per_min: 0 } : r)), config: cfg, parts_s: 6.7 })).toBe('Follows Rotor at 2×');
	});

	it('of a linear sweep, moves at its ratio times the root rate in Hz/s', () => {
		const follower = row({ id: 'b3', kind: 'narrowband', name: 'Late band', freq_hz: 660, rate_hz_per_s: 0, law: null });
		expect(partMotion(follower, false, { rows: [...rows, follower], config, parts_s: 50 })).toBe('Follows Rotor band at 0.5×, sweeping up at 2.5 Hz/s');
	});

	it('shows its own frequency from its leader when the row has none', () => {
		expect(partFreq(row({ ...second, freq_hz: null }), ctx)).toBeCloseTo(819, 6);
		expect(partFreq(second, ctx)).toBe(818.9);
		expect(partFreq(floor, ctx)).toBeNull();
	});
});

describe('a narrowband sized by a share of its center', () => {
	it("never reads a zero rate from the field the service left empty: the schedule's own rate and unit", () => {
		expect(partMotion(band, false, ctx)).toBe('Sweeping up at 5 Hz/s, sweep 1 of 2');
		expect(partMotion(band, false, ctx)).not.toContain('0 Hz/s');
	});

	it("uses the row's rate when it carries one in the schedule's unit", () => {
		expect(partMotion({ ...band, rate_hz_per_s: -5 }, false, ctx)).toBe('Sweeping up at 5 Hz/s, sweep 1 of 2');
	});

	it('converts a rate the row gives only in the other unit, at the frequency now', () => {
		// 0.31 oct/min at 409.5 Hz is 409.5 ln 2 * 0.31 / 60 Hz/s.
		expect(partMotion({ ...rotor, rate_oct_per_min: null, rate_hz_per_s: (409.5 * Math.LN2 * 0.31) / 60 }, false, ctx)).toBe('Sweeping up at 0.31 oct/min, sweep 1');
	});

	it('without the setup and with a zero rate, names no rate at all', () => {
		expect(partMotion(band, false)).toBe('Sweeping up, sweep 1 of 2');
	});

	it('says its width as the share of its center as well as in Hz', () => {
		expect(partDetail(band, ctx)).toContain('65.9 Hz wide, 5% of its center');
	});
});

describe('the floor early in a run', () => {
	it('says what the degrees of freedom are, once', () => {
		expect(partDetail(floor, ctx)).toBe('3928 of 3961 lines clean, 0 in alarm, 0 over abort; at least 2 degrees of freedom on every clean line');
		expect(partDetail(floor, ctx)).not.toMatch(/\b2 dof\b/);
	});

	it('shows no figure the row does not carry', () => {
		expect(partDetail({ ...floor, dof: null }, ctx)).toBe('3928 of 3961 lines clean, 0 in alarm, 0 over abort');
		expect(partDetail({ ...floor, dof: null, lines: null }, ctx)).toBe('');
	});

	it('before a clean frame is averaged, says so instead of 0 degrees of freedom', () => {
		expect(partDetail({ ...floor, dof: 0 }, ctx)).toBe('3928 of 3961 lines clean, 0 in alarm, 0 over abort; no clean frame averaged yet');
	});
});

describe('a narrowband before its first average', () => {
	it('says no average yet rather than 0 dof, and shows no drive figure it does not have', () => {
		expect(partDetail(gear, ctx)).toBe('20 Hz wide, no frame averaged yet, gain +0.00 dB');
		expect(partDrive(gear)).toBe('-');
		expect(partDrive({ ...gear, drive_vrms: 0.012 })).toBe('0.012 V rms');
		expect(partDrive({ ...gear, on: false, scale: 0, drive_vrms: 0 })).toBe('0 V rms');
	});
});

describe('a part that is off or has ended', () => {
	it('waits for its window only before the window opens', () => {
		const late = row({ id: 'b3', kind: 'narrowband', name: 'Late band', on: false, scale: 0, law: null });
		expect(partMotion(late, false, ctx)).toBe('Waiting for its window, from 40 s');
		expect(partStateWord(late, ctx)).toBe('Waiting');
	});

	it('says its window has closed once parts time passes the window', () => {
		const closed = { ...gear, on: false, scale: 0 };
		expect(partMotion(closed, false, { ...ctx, parts_s: 31 })).toBe('Its window closed at 30 s');
		expect(partStateWord(closed, { ...ctx, parts_s: 31 })).toBe('Ended');
	});

	it('says its schedule has ended once its sweeps are done', () => {
		const done = { ...band, on: false, scale: 0, sweeps: { done: 2, of: 2 } };
		expect(partMotion(done, false, ctx)).toBe('Its schedule ended');
		expect(partStateWord(done, ctx)).toBe('Ended');
	});

	it("uses the row's own done flag when the service sends it", () => {
		const done = { ...rotor, on: false, scale: 0, done: true } as MixedPartRow;
		expect(partMotion(done, false, ctx)).toBe('Its schedule ended');
	});

	it('switched off reads Off', () => {
		expect(partMotion({ ...second, on: false, enabled: false, scale: 0 }, false, ctx)).toBe('Off');
		expect(partStateWord({ ...second, on: false, enabled: false, scale: 0 }, ctx)).toBe('Off');
	});
});
