/**
 * The MIMO Mixed Mode setup draft: the channels fitted to the table with every per-control list
 * following the controls, the floor's build request, the configuration on the wire with its five
 * laws, where a service finding goes, and what the editor says before anything is sent.
 */
import { describe, expect, it } from 'vitest';
import type { ChannelRow } from '$lib/protocol/types';
import type { MimoMixedBandBody, MimoMixedReportItem, MimoMixedToneBody } from '$lib/protocol/mimoMixed';
import { abortVpkOf, breakpointParamsOf, defaultMmixSetup, fitChannels, freeId, localErrors, mergeMmixSetup, mmixItemKey, mmixScheduleOf, mmixSectionOf, mmixTitle, newBand, newControl, newDrive, refusalWhere, setControl, toMmixConfig } from './mmixSetup';

function row(index: number, role: ChannelRow['role'], enabled = true): ChannelRow {
	return { index, enabled, description: '', role, units: 'g' } as unknown as ChannelRow;
}

const TABLE = [row(1, 'control'), row(2, 'control'), row(3, 'control'), row(4, 'drive'), row(5, 'drive'), row(6, 'monitor')];

function filled() {
	const d = defaultMmixSetup();
	fitChannels(d, TABLE);
	for (const x of d.drives) {
		x.ceiling_vrms = 2;
		x.peak_v = 6;
	}
	return d;
}

const item = (over: Partial<MimoMixedReportItem>): MimoMixedReportItem => ({ reason: 'bad_config', message: 'm', ...over }) as MimoMixedReportItem;

describe('the defaults', () => {
	it("give the default tone an exclusion of at least two of the default frame's lines, which the controller needs to arm", () => {
		const d = defaultMmixSetup();
		const df = d.estimation.fs_hz / d.estimation.n_pts;
		expect(d.tones.length).toBeGreaterThan(0);
		expect(d.exclusion_hz).toBeGreaterThanOrEqual(2 * df);
	});
});

describe('the channels', () => {
	it('takes every enabled control and drive row, and fits every per-control list to the controls', () => {
		const d = filled();
		expect(d.controls.map((c) => c.channel)).toEqual([1, 2, 3]);
		expect(d.drives.map((x) => x.channel)).toEqual([4, 5]);
		expect(d.tones[0].amp_g).toHaveLength(3);
		expect(d.tones[0].phase_deg).toHaveLength(3);
		d.bands.push(newBand('b1', 1, 3));
		setControl(d, TABLE, 2, false);
		expect(d.controls.map((c) => c.channel)).toEqual([1, 3]);
		expect(d.tones[0].amp_g).toHaveLength(2);
		expect(d.bands[0].statement.db).toHaveLength(2);
		expect(fitChannels(d, TABLE)).toBe(false);
	});

	it('keeps a drive its ceilings, and leaves new drives with none', () => {
		const d = filled();
		fitChannels(d, [...TABLE, row(7, 'drive')]);
		expect(d.drives[0]).toMatchObject({ channel: 4, ceiling_vrms: 2, peak_v: 6 });
		expect(Number.isNaN(d.drives[2].ceiling_vrms)).toBe(true);
	});

	it('gives each new part a free id, never the floor', () => {
		const d = filled();
		expect(freeId(d, 't')).toBe('t2');
		expect(freeId(d, 'b')).toBe('b1');
	});

	it('names itself from its size and its parts, and reads back a partial draft', () => {
		const d = filled();
		expect(mmixTitle(d)).toBe('MIMO Mixed 3 by 2, 1 tone on the floor');
		const back = mergeMmixSetup({ laws: { bands: 'nonsense' } } as never);
		expect(back.laws).toMatchObject({ bands: 'auto', ceiling_rule: 'common_scale', fill_rule: 'no_error' });
		expect(back.tones).toHaveLength(1);
	});
});

describe('to the protocol', () => {
	it("asks for the floor's reference with one coherence and phase for every pair", () => {
		const d = filled();
		d.reference.coherence = 0.5;
		d.reference.phase_deg = 30;
		const p = breakpointParamsOf(d, ['A', 'B', 'C']);
		expect(p.tables.map((t) => t.label)).toEqual(['A', 'B', 'C']);
		expect(p.pairs).toEqual([
			{ row: 0, col: 1, coherence: 0.5, phase_deg: 30 },
			{ row: 0, col: 2, coherence: 0.5, phase_deg: 30 },
			{ row: 1, col: 2, coherence: 0.5, phase_deg: 30 }
		]);
		d.reference.coherence = 0;
		expect(breakpointParamsOf(d, []).pairs).toEqual([]);
	});

	it('sends the floor first, every part, the five laws and the model held', () => {
		const d = filled();
		d.tones[0].amp_g = [1, 2, 1];
		d.tones[0].phases_stated = true;
		d.tones[0].phase_deg = [0, 90, 180];
		const b = newBand('b1', 1, 3);
		b.statement = { kind: 'added', db: [10, 10, 10], level_g2_hz: [0.02, 0.02, 0.02], coherence: 0.3 };
		d.bands.push(b);
		d.laws.bands = 'per_control';
		d.laws.ceiling_rule = 'in_turn';
		d.laws.fill_rule = 'model_fill';
		const c = toMmixConfig(d, null, { floor: null, tones: null });
		expect(c.parts.map((p) => p.body.kind)).toEqual(['floor', 'tone', 'narrowband']);
		expect(c.parts.map((p) => p.id)).toEqual(['floor', 't1', 'b1']);
		const tone = c.parts[1].body as MimoMixedToneBody;
		expect(tone.amplitude).toEqual([
			{ kind: 'constant', g_pk: 1 },
			{ kind: 'constant', g_pk: 2 },
			{ kind: 'constant', g_pk: 1 }
		]);
		expect(tone.phase_deg).toEqual([0, 90, 180]);
		expect(tone.schedule).toEqual({ kind: 'fixed', hz: 120 });
		const band = c.parts[2].body as MimoMixedBandBody;
		expect(band.statement).toEqual({ kind: 'added', level_g2_hz: 0.02, coherence: 0.3 });
		// no bands of its own: the members are left out (the wire refuses null), the service's 3 and 6 dB hold
		expect('alarm_db' in band || 'abort_db' in band).toBe(false);
		expect(c.law).toMatchObject({ bands: { id: 'per_control' }, ceiling_rule: { id: 'in_turn' }, fill_rule: { id: 'model_fill' } });
		expect(c.frf_mode).toBe('Hold');
		expect(c.controls).toEqual([1, 2, 3]);
		// no aborts of its own: the members are left out, and the service takes each ceiling plus 1 dB
		expect(c.drives[0]).toEqual({ channel_index: 4, ceiling_vrms: 2, peak_v: 6, pretest_max_vrms: 0.2 });
		expect(c.parts[0].window).toEqual({ on_s: 0 });
		expect('specification' in c).toBe(false);
	});

	it("gives each drive's own aborts, the limits' ceilings and the level schedule as the controller has them", () => {
		const d = filled();
		d.drives[1].abort_vrms = 2.4;
		d.drives[1].abort_vpk = 7;
		d.limits = [{ channel: 6, ceiling: 0.04, abort_margin_db: 3 }];
		d.record.document = 'Spec';
		const c = toMmixConfig(d, null, { floor: null, tones: null });
		expect(c.drives[1]).toEqual({ channel_index: 5, ceiling_vrms: 2, peak_v: 6, abort_vrms: 2.4, abort_vpk: 7, pretest_max_vrms: 0.2 });
		expect(c.limits).toEqual([{ channel_index: 6, ceiling_psd: [0.04], abort_margin_db: 3 }]);
		expect(c.schedule).toEqual({ mode: 'auto', start_db: -12, table_db: [-12, -6, 0], dwell_s: [10, 10] });
		expect(c.full_level_duration_s).toBe(120);
		expect(c.caps).toEqual({ up_db: 3.5, down_db: 6 });
		expect(c.identification).toEqual({ below_reference_db: 20, frames: 64, start_vrms: 0.01, tolerance_db: 1, max_steps: 8, noise_frames: 32 });
		expect(c.gates).toEqual({ coherence_threshold: 0.5, condition_threshold: 1000 });
		expect(c.specification).toEqual({ document: 'Spec', clause: '' });
		// the peak abort the run takes: its own, or the peak ceiling plus 1 dB
		expect(abortVpkOf(d.drives[1])).toBe(7);
		expect(abortVpkOf(d.drives[0])).toBeCloseTo(6.732, 3);
	});

	it('ends every sweep: by its sweeps, its duration or whichever comes first', () => {
		const d = filled();
		const s = { ...d.tones[0].schedule, kind: 'sweep' as const, band_hz: [50, 400] as [number, number], law: 'log' as const, oct_per_min: 2 };
		expect(mmixScheduleOf(s)).toEqual({ kind: 'sweep', lo_hz: 50, hi_hz: 400, start_hz: 50, direction: 'up', law: 'log_oct_per_min', rate: 2, target: { kind: 'sweeps', n: 4 } });
		expect(mmixScheduleOf({ ...s, end_by: 'duration' })).toMatchObject({ target: { kind: 'duration', s: 300 } });
		expect(mmixScheduleOf({ ...s, end_by: 'both' })).toMatchObject({ target: { kind: 'whichever_first', sweeps: 4, duration_s: 300 } });
		expect(mmixScheduleOf({ ...s, kind: 'fixed', dwell: { by: 'cycles', seconds: 1, cycles: 50 } })).toEqual({ kind: 'fixed', hz: 120, for_cycles: 50 });
		// a draft saved with a sweep that never ends reads back ending after its sweeps
		const back = mergeMmixSetup({ tones: [{ ...d.tones[0], schedule: { ...s, end_by: 'none' } }] } as never);
		expect(back.tones[0].schedule.end_by).toBe('sweeps');
	});

	it('leaves the phases to the tone law when none are stated', () => {
		const d = filled();
		d.tones[0].amp_g = [1, 1, 1];
		d.tones[0].phases_stated = false;
		const tone = toMmixConfig(d, null, { floor: null, tones: null }).parts[1].body as MimoMixedToneBody;
		expect(tone.amplitude).toEqual({ kind: 'constant', g_pk: 1 });
		expect(tone.phases_stated).toBe(false);
		expect('phase_deg' in tone).toBe(false);
	});
});

describe('where a finding goes', () => {
	it('puts a refusal by its field path or its name, and each key in its section', () => {
		const d = filled();
		d.bands.push(newBand('b1', 1, 3));
		const cases: [Partial<MimoMixedReportItem>, string, string][] = [
			[{ reason: 'law_refuses_tone', field: 'parts[1]' }, 'tones.0', 'parts'],
			[{ reason: 'law_refuses_tone', field: 'parts[2]' }, 'bands.0', 'parts'],
			[{ reason: 'band_not_additive', field: 'config' }, 'parts', 'parts'],
			[{ reason: 'budget_cannot_fit', field: 'config' }, 'forecast', 'forecast'],
			[{ reason: 'frf_update_not_built', field: 'config' }, 'control.frf', 'limits'],
			[{ reason: 'unknown_fill_rule', field: 'config' }, 'laws.fill_rule', 'laws'],
			[{ reason: 'ramp_too_short', field: 'config' }, 'control.part_ramp_s', 'limits'],
			[{ reason: 'channel_unknown', field: 'drives[1]' }, 'drives.1', 'channels'],
			[{ reason: 'duplicate_drive', field: 'drives[1]' }, 'drives.1', 'channels'],
			[{ reason: 'channel_unknown', field: 'limits[0]' }, 'limits.0', 'limits'],
			[{ reason: 'channel_unknown', field: 'controls[2]' }, 'channels.controls', 'channels'],
			[{ reason: 'law_unknown', field: 'law.tones.id' }, 'laws.tones.id', 'laws'],
			[{ reason: 'param_out_of_range', field: 'law.floor.params.step' }, 'laws.floor.params.step', 'laws'],
			[{ reason: 'out_of_range', field: 'estimation.n_pts' }, 'estimation.n_pts', 'schedule'],
			[{ reason: 'reference_invalid', field: 'reference' }, 'reference', 'reference'],
			[{ reason: 'drive_channel_present', channel: 9 }, 'channels', 'channels']
		];
		for (const [over, key, section] of cases) {
			expect(mmixItemKey(d, item(over)), over.reason).toBe(key);
			expect(mmixSectionOf(key)).toBe(section);
		}
	});

	it('names where a refusal is in words', () => {
		const names = { parts: ['Floor', 'Tone 1', 'Band 1'], controls: ['A', 'B', 'C'], drives: ['X', 'Y'] };
		expect(refusalWhere({ field: 'parts[2]' }, names)).toBe('part Band 1');
		expect(refusalWhere({ field: 'drives[1]' }, names)).toBe('drive Y');
		expect(refusalWhere({ field: 'law.tones.id' }, names)).toBe("the tones' law");
		expect(refusalWhere({ field: 'config' }, names)).toBe('');
	});
});

describe('what the editor says first', () => {
	it("takes MIMO Sine's 24 controls and 12 drives, and names one more of either", () => {
		const d = defaultMmixSetup();
		d.controls = Array.from({ length: 24 }, (_, i) => newControl(i + 1));
		d.drives = Array.from({ length: 12 }, (_, j) => newDrive(25 + j));
		expect(localErrors(d)['channels.controls']).toBeUndefined();
		expect(localErrors(d)['channels.drives']).toBeUndefined();
		d.controls.push(newControl(37));
		d.drives.push(newDrive(38));
		expect(localErrors(d)['channels.controls']).toEqual(['At most 24 controls.']);
		expect(localErrors(d)['channels.drives']).toEqual(['At most 12 drives.']);
	});

	it('asks for each drive its ceilings, with no default', () => {
		const d = defaultMmixSetup();
		fitChannels(d, TABLE);
		const e = localErrors(d);
		expect(e['drives.0.ceiling_vrms'][0]).toMatch(/no default/);
		expect(e['drives.1.peak_v']).toBeDefined();
		d.drives[0].ceiling_vrms = 2;
		d.drives[0].peak_v = 1;
		expect(localErrors(d)['drives.0.peak_v']).toEqual(['The peak ceiling is at least the rms ceiling.']);
	});

	it('is quiet on a filled draft, and refuses a part id taken or a follower with no leader', () => {
		const d = filled();
		expect(localErrors(d)).toEqual({});
		const b = newBand('t1', 1, 3);
		b.schedule = { kind: 'follow', of: 'nobody', ratio: 2 } as never;
		d.bands.push(b);
		const e = localErrors(d);
		expect(e['bands.0.id']).toEqual(['Another part has this id.']);
		expect(e['bands.0.schedule.of']).toEqual(['Choose the part it follows.']);
	});

	it("refuses a drive's abort at or under its ceiling, and an identification with no room", () => {
		const d = filled();
		d.drives[0].abort_vpk = 6;
		d.drives[0].abort_vrms = 1.5;
		d.drives[1].pretest_max_vrms = 0;
		const e = localErrors(d);
		expect(e['drives.0.abort_vpk']).toEqual(['The peak abort is above the peak ceiling.']);
		expect(e['drives.0.abort_vrms']).toEqual(['The rms abort is above the rms ceiling.']);
		expect(e['drives.1.pretest_max_vrms']).toBeDefined();
		d.estimation.n_pts = 1000;
		expect(localErrors(d)['estimation.n_pts']).toEqual(['A power of two from 64.']);
	});

	it('wants no controls or drives before anything is sent', () => {
		const e = localErrors(defaultMmixSetup());
		expect(e['channels.controls']).toBeDefined();
		expect(e['channels.drives']).toBeDefined();
	});
});
