/**
 * The MIMO Mixed Mode setup draft: the channels fitted to the table with every per-control list
 * following the controls, the floor's build request, the configuration on the wire with its five
 * laws, where a service finding goes, and what the editor says before anything is sent.
 */
import { describe, expect, it } from 'vitest';
import type { ChannelRow } from '$lib/protocol/types';
import type { MimoMixedBandPart, MimoMixedReportItem, MimoMixedTonePart } from '$lib/protocol/mimoMixed';
import { breakpointParamsOf, defaultMmixSetup, fitChannels, freeId, localErrors, mergeMmixSetup, mmixItemKey, mmixSectionOf, mmixTitle, newBand, refusalWhere, setControl, toMmixConfig } from './mmixSetup';

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
		expect(c.parts.map((p) => p.kind)).toEqual(['floor', 'tone', 'narrowband']);
		const tone = c.parts[1] as MimoMixedTonePart;
		expect(tone.amplitude).toEqual([
			{ kind: 'constant', g_pk: 1 },
			{ kind: 'constant', g_pk: 2 },
			{ kind: 'constant', g_pk: 1 }
		]);
		expect(tone.phase_deg).toEqual([0, 90, 180]);
		const band = c.parts[2] as MimoMixedBandPart;
		expect(band.statement).toEqual({ kind: 'added', level_g2_hz: 0.02, coherence: 0.3 });
		expect(c.law).toMatchObject({ bands: { id: 'per_control' }, ceiling_rule: { id: 'in_turn' }, fill_rule: { id: 'model_fill' } });
		expect(c.control?.frf).toBe('hold');
		expect(c.channels.drives[0]).toEqual({ channel: 4, ceiling_vrms: 2, peak_v: 6, abort_vrms: null });
	});

	it('leaves the phases to the tone law when none are stated', () => {
		const d = filled();
		d.tones[0].amp_g = [1, 1, 1];
		d.tones[0].phases_stated = false;
		const tone = toMmixConfig(d, null, { floor: null, tones: null }).parts[1] as MimoMixedTonePart;
		expect(tone.amplitude).toEqual({ kind: 'constant', g_pk: 1 });
		expect(tone.phases_stated).toBe(false);
		expect(tone.phase_deg).toEqual([]);
	});
});

describe('where a finding goes', () => {
	it('puts a refusal by its part, drive, law or field, and each key in its section', () => {
		const d = filled();
		d.bands.push(newBand('b1', 1, 3));
		const cases: [Partial<MimoMixedReportItem>, string, string][] = [
			[{ reason: 'band_not_additive', part: 'b1', control: 2 }, 'bands.0', 'parts'],
			[{ reason: 'part_out_of_band', part: 't1' }, 'tones.0', 'parts'],
			[{ reason: 'budget_cannot_fit', drive: 1 }, 'forecast', 'forecast'],
			[{ reason: 'frf_update_not_built' }, 'control.frf', 'limits'],
			[{ reason: 'unknown_fill_rule' }, 'laws.fill_rule', 'laws'],
			[{ reason: 'bad_config', drive: 1 }, 'drives.1', 'channels'],
			[{ reason: 'law_unknown', field: 'config.law.tones.id' }, 'laws.tones.id', 'laws'],
			[{ reason: 'bad_config', field: 'config.law.floor.params.step' }, 'laws.floor.params.step', 'laws'],
			[{ reason: 'out_of_range', field: 'config.schedule.levels[1].db' }, 'schedule.levels.1.db', 'schedule'],
			[{ reason: 'reference_invalid' }, 'reference', 'reference']
		];
		for (const [over, key, section] of cases) {
			expect(mmixItemKey(d, item(over))).toBe(key);
			expect(mmixSectionOf(key)).toBe(section);
		}
	});

	it('names where a refusal is in words', () => {
		const names = { parts: { b1: 'Band 1' }, controls: ['A', 'B', 'C'], drives: ['X', 'Y'] };
		expect(refusalWhere({ part: 'b1', control: 2 }, names)).toBe('part Band 1, control C');
		expect(refusalWhere({ drive: 1 }, names)).toBe('drive Y');
	});
});

describe('what the editor says first', () => {
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

	it('wants no controls or drives before anything is sent', () => {
		const e = localErrors(defaultMmixSetup());
		expect(e['channels.controls']).toBeDefined();
		expect(e['channels.drives']).toBeDefined();
	});
});
