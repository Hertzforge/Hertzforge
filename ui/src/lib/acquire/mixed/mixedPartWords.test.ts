/**
 * The parts table's words against rows shaped as the control service sends them on a combined run
 * (a follower with its leader, ratio, direction and rate; a narrowband sized by a share of its
 * center; the floor early in a run; parts off or ended), and against rows that leave a fact out
 * (no `follows`, a zero rate), which the words fill from the setup rather than print wrongly.
 */

import { describe, expect, it } from 'vitest';
import type { MixedConfig, MixedPartRow } from '$lib/protocol/mixed';
import { judgedTolerance } from './mixedJudged';
import type { SpectrumView } from '$lib/protocol/types';
import { limitingWords, mixedTripFacts, partErrorCell, toneJudgingWords, judgingSummary, partDetail, partDrive, partFreq, partMotion, partStateWord } from './mixedWords';
import type { MixedTrip } from '$lib/protocol/mixed';

const base: MixedPartRow = { id: 'x', kind: 'tone', name: 'x', enabled: true, on: true, scale: 1, freq_hz: null, direction: null, sweeps: null, rate_oct_per_min: null, rate_hz_per_s: null, width_hz: null, unit: 'g', target: 0.0846, measured: 0.083, error_db: null, alarm: false, drive_vpk: 0.0679, drive_vrms: null, gain_db: null, settled: false, limiting: false, dof: null, lines: null, law: { misses: 0, capped: 0 }, follows: null, judge_s: null, judged_error_db: null, judged_alarm_db: null, judged_abort_db: null, tolerance_widened: null };
const row = (over: Partial<MixedPartRow>): MixedPartRow => ({ ...base, ...over });

// The rows of the combined run at parts time 6.7 s as the service sends them.
const floor = row({ id: 'floor', kind: 'floor', name: 'Floor', freq_hz: null, unit: 'g', target: 0.125, measured: 0.125, error_db: 0.01, drive_vpk: null, drive_vrms: 0.163, dof: 2, settled: true, lines: { in_band: 3961, clean: 3928, alarm: 0, abort: 0, rms_judged: 2100 } });
const rotor = row({ id: 't2', name: 'Rotor', freq_hz: 409.5, direction: 'up', sweeps: { done: 0, of: null }, rate_oct_per_min: 0.3, drive_vpk: 0.062 });
const second = row({ id: 't3', name: 'Rotor 2nd', freq_hz: 818.9, direction: 'up', follows: { of: 't2', ratio: 2 }, rate_oct_per_min: 0.3, drive_vpk: 0.0193, target: 0.0282 });
const third = row({ id: 't4', name: 'Rotor 3rd', freq_hz: 1228.4, direction: 'up', follows: { of: 't3', ratio: 1.5 }, rate_oct_per_min: 0.3, drive_vpk: 0.0356, target: 0.0282 });
const gear = row({ id: 'b1', kind: 'narrowband', name: 'Gear mesh', freq_hz: 250, unit: 'g²/Hz', target: 0.0000397, measured: null, drive_vpk: null, drive_vrms: null, width_hz: 20, dof: 0, gain_db: 0, law: null });
const band = row({ id: 'b2', kind: 'narrowband', name: 'Rotor band', freq_hz: 1317.5, direction: 'up', sweeps: { done: 0, of: 2 }, rate_hz_per_s: 5, unit: 'g²/Hz', target: 0.0000397, measured: null, drive_vpk: null, drive_vrms: null, width_hz: 65.9, dof: 0, gain_db: 0, law: null });
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
	it('says what it follows and at what ratio', () => {
		expect(partMotion(second, false, ctx)).toBe('Follows Rotor at 2×, sweeping up at 0.3 oct/min');
	});

	it('reads its leader and ratio from the setup when the row leaves them out', () => {
		expect(partMotion({ ...second, follows: null, direction: null, rate_oct_per_min: null }, false, ctx)).toBe('Follows Rotor at 2×, sweeping up at 0.3 oct/min');
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
		const empty = { ...band, rate_hz_per_s: 0 };
		expect(partMotion(empty, false, ctx)).toBe('Sweeping up at 5 Hz/s, sweep 1 of 2');
		expect(partMotion(empty, false, ctx)).not.toContain('0 Hz/s');
	});

	it("uses the row's rate when it carries one in the schedule's unit", () => {
		expect(partMotion({ ...band, rate_hz_per_s: -5 }, false, ctx)).toBe('Sweeping up at 5 Hz/s, sweep 1 of 2');
	});

	it('converts a rate the row gives only in the other unit, at the frequency now', () => {
		// 0.31 oct/min at 409.5 Hz is 409.5 ln 2 * 0.31 / 60 Hz/s.
		expect(partMotion({ ...rotor, rate_oct_per_min: null, rate_hz_per_s: (409.5 * Math.LN2 * 0.31) / 60 }, false, ctx)).toBe('Sweeping up at 0.31 oct/min, sweep 1');
	});

	it('without the setup and with a zero rate, names no rate at all', () => {
		expect(partMotion({ ...band, rate_hz_per_s: 0 }, false)).toBe('Sweeping up, sweep 1 of 2');
	});

	it('says its width as the share of its center as well as in Hz', () => {
		expect(partDetail(band, ctx)).toContain('65.9 Hz wide, 5% of its center');
	});
});

describe('the floor early in a run', () => {
	it('says what the degrees of freedom are and whether the rms abort is armed yet', () => {
		expect(partDetail(floor, ctx)).toBe('3928 of 3961 lines clean, 0 in alarm, 0 over abort; about 2 degrees of freedom on a typical line; rms abort not armed yet, 2100 lines qualify');
		expect(partDetail(floor, ctx)).not.toMatch(/\b2 dof\b/);
		expect(partDetail({ ...floor, dof: 120, lines: { ...floor.lines!, rms_judged: 3928 } }, ctx)).toBe('3928 of 3961 lines clean, 0 in alarm, 0 over abort; about 120 degrees of freedom on a typical line; rms abort judged on 3928 lines');
	});

	it('shows no figure the row does not carry', () => {
		expect(partDetail({ ...floor, dof: null }, ctx)).toBe('3928 of 3961 lines clean, 0 in alarm, 0 over abort; rms abort not armed yet, 2100 lines qualify');
		expect(partDetail({ ...floor, dof: null, lines: null }, ctx)).toBe('');
	});

	it('before a clean frame is averaged, says so instead of 0 degrees of freedom', () => {
		expect(partDetail({ ...floor, dof: 0 }, ctx)).toBe('3928 of 3961 lines clean, 0 in alarm, 0 over abort; no clean frame averaged yet; rms abort not armed yet, 2100 lines qualify');
	});
});

describe('a narrowband before its first average', () => {
	it('says no average yet rather than 0 dof, and shows no drive of its own', () => {
		expect(partDetail(gear, ctx)).toBe('20 Hz wide, no frame averaged yet, gain +0.00 dB');
		expect(partDrive(gear)).toBe('-');
		expect(partDrive({ ...gear, drive_vrms: 0.012 })).toBe('0.012 V rms');
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

	it('switched off reads Off', () => {
		expect(partMotion({ ...second, on: false, enabled: false, scale: 0 }, false, ctx)).toBe('Off');
		expect(partStateWord({ ...second, on: false, enabled: false, scale: 0 }, ctx)).toBe('Off');
	});
});

describe('limiting, for the run header', () => {
	it('names the parts the ceiling holds, or the clamp when no part says so, and nothing otherwise', () => {
		expect(limitingWords({ limiting: true, parts: [floor, { ...rotor, limiting: true }, { ...second, limiting: true }] })).toBe('Rotor, Rotor 2nd');
		expect(limitingWords({ limiting: true, parts: rows })).toBe('At the drive ceiling');
		expect(limitingWords({ limiting: false, parts: rows })).toBeNull();
	});
});

describe('trip facts', () => {
	const at = { part: 't2', at_hz: 433.2, at_level_db: -6, at_s: 40, at_loop: 8, at_tick: 4000 };
	it('shows a tone abort measured against its limit once the service fills them', () => {
		const f = mixedTripFacts({ ...at, kind: 'abort_band', side: 'high', error_db: 3.1, measured_g: 0.1205, limit_g: 0.1195 } as MixedTrip, (id) => (id === 't2' ? 'Rotor' : id));
		expect(f).toContainEqual({ label: 'Measured against limit', value: '0.1205 g against 0.1195 g' });
		expect(f).toContainEqual({ label: 'Part', value: 'Rotor' });
	});
	it('never prints a null', () => {
		const f = mixedTripFacts({ ...at, part: null, at_hz: null, kind: 'envelope', abort_lines: 14, worst_db: null } as MixedTrip, (id) => id);
		expect(f.map((x) => x.label)).toEqual(['Level', 'Lines out']);
		const b = mixedTripFacts({ ...at, kind: 'band_abort', side: 'low', error_db: null, measured_g2_hz: null, limit_g2_hz: 0.0002, dof: null } as MixedTrip, (id) => id);
		expect(b.map((x) => x.label)).toEqual(['Part', 'At', 'Level', 'Side', 'Lower abort']);
		for (const x of [...f, ...b]) expect(x.value).not.toMatch(/null|undefined|NaN/);
	});
});

describe('how a tone is judged', () => {
	const small = row({ ...second, judge_s: 8, error_db: -0.35, judged_error_db: -0.25, judged_alarm_db: [1.75, 1.75], judged_abort_db: [3.5, 3.5], tolerance_widened: true });
	const quick = row({ ...rotor, judge_s: 0.5, error_db: 0.15, judged_error_db: 0.13, judged_alarm_db: [1.5, 1.5], judged_abort_db: [3, 3], tolerance_widened: false });

	it('puts the judged error first and the single reading beside it, never the single reading alone as the error', () => {
		expect(partErrorCell(small)).toEqual({ main: '-0.25 dB', single: '-0.35 dB' });
		expect(partErrorCell({ ...small, judged_error_db: null })).toEqual({ main: '-', single: '-0.35 dB' });
		expect(partErrorCell(floor)).toEqual({ main: '+0.01 dB', single: null });
	});

	it('says how long a tone is judged over, and the widened tolerances when they are', () => {
		expect(toneJudgingWords(quick)).toBe('judged over 0.5 s');
		expect(toneJudgingWords(small)).toBe('judged over 8 s, tolerances widened to alarm ±1.75 dB, abort ±3.5 dB');
		expect(toneJudgingWords({ ...small, judge_s: null })).toBe('');
		expect(partDetail(small, ctx)).toBe('judged over 8 s, tolerances widened to alarm ±1.75 dB, abort ±3.5 dB');
	});

	it("redraws a tone's amplitude bands at the judged tolerance, from its Target line", () => {
		const line = (name: string, role: string, y: number[]) => ({ name, role, x: Float64Array.from([1, 2]), y: Float64Array.from(y), shape: 'line' });
		const v = { view: 'spectrum', title: 'Rotor 2nd', x: { label: 'Time', unit: 's', scale: 'linear' }, y: { label: 'Amplitude', unit: 'g', scale: 'log' }, series: [line('Measured', 'measured', [0.1, 0.1]), line('Target', 'reference', [0.1, 0.1]), line('Alarm high', 'tolerance', [0.119, 0.119]), line('Alarm low', 'tolerance', [0.084, 0.084]), line('Abort high', 'abort', [0.141, 0.141]), line('Abort low', 'abort', [0.071, 0.071])] } as unknown as SpectrumView;
		const [out] = judgedTolerance([v], ['t3'], [small]) as SpectrumView[];
		const y = (n: string) => out.series.find((s) => s.name === n)!.y[0];
		expect(y('Alarm high')).toBeCloseTo(0.1 * 10 ** (1.75 / 20), 9);
		expect(y('Alarm low')).toBeCloseTo(0.1 * 10 ** (-1.75 / 20), 9);
		expect(y('Abort high')).toBeCloseTo(0.1 * 10 ** (3.5 / 20), 9);
		expect(y('Measured')).toBe(0.1);
		// a part without judged bands (a narrowband) keeps the view as it came
		expect(judgedTolerance([v], ['b1'], [gear])[0]).toBe(v);
	});
});

describe('the setup summary\'s judging line', () => {
	it('lists the tones judged slowly or against widened tolerances, before the run', () => {
		const j = [
			{ part: 't1', judge_s: 0.5, sigma_db: 0.12, alarm_db: [1.5, 1.5] as [number, number], abort_db: [3, 3] as [number, number], widened: false },
			{ part: 't2', judge_s: 3.01, sigma_db: 0.5, alarm_db: [1.5, 1.5] as [number, number], abort_db: [3, 3] as [number, number], widened: false },
			{ part: 't3', judge_s: 8, sigma_db: 0.94, alarm_db: [2.82, 2.82] as [number, number], abort_db: [3, 3] as [number, number], widened: true }
		];
		const name = (id: string) => ({ t1: 'Blade pass', t2: 'Rotor', t3: 'Rotor 2nd' })[id] ?? id;
		expect(judgingSummary(j, name)).toBe('Rotor: over 3.01 s; Rotor 2nd: over 8 s, alarm widened to ±2.82 dB, abort ±3 dB');
		expect(judgingSummary(j.slice(0, 1), name)).toBe('Every tone is judged at once');
	});
});
