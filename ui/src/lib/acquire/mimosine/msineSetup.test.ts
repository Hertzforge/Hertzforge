/**
 * The MIMO Sine setup draft: its conversion to the protocol's configuration and back, the channels
 * fitted to the table with each control keeping its reference, the laws' refusals in plain words,
 * where a service finding goes, and what the editor says before anything is sent.
 */
import { describe, expect, it } from 'vitest';
import type { ChannelRow } from '$lib/protocol/types';
import type { LawEntry } from '$lib/protocol/mimo';
import { canonical } from '../mimo/mimoSetup';
import { defaultMsineSetup, fitChannels, fromMsineConfig, lawRefusal, localErrors, mergeMsineSetup, msineItemKey, msineSectionOf, msineTitle, setControl, shapeOf, toMsineConfig } from './msineSetup';

function row(index: number, role: ChannelRow['role'], enabled = true): ChannelRow {
	return { index, enabled, description: '', role, units: 'g' } as unknown as ChannelRow;
}

const TABLE = [row(1, 'control'), row(2, 'control'), row(3, 'control'), row(4, 'drive'), row(5, 'drive'), row(6, 'monitor')];

function filled() {
	const d = defaultMsineSetup();
	fitChannels(d, TABLE);
	for (const x of d.drives) {
		x.ceiling_vpk = 2;
		x.abort_vpk = 3;
	}
	return d;
}

const law = (id: string, test_types: string[], amplitude_only: boolean, shapes: string[] | null = null) => ({ id, name: id, shapes, test_types, amplitude_only }) as unknown as LawEntry;

describe('the MIMO Sine setup draft', () => {
	it('fits the controls and drives to the table, each control keeping its reference', () => {
		const d = filled();
		expect(d.controls.map((c) => c.channel)).toEqual([1, 2, 3]);
		expect(d.drives.map((x) => x.channel)).toEqual([4, 5]);
		expect(d.feasibility.shakers.map((s) => s.drive)).toEqual([4, 5]);
		d.controls[1].profile.rows[0].value = 2.5;
		d.controls[1].phase[0].phase_deg = 90;
		setControl(d, TABLE, 1, false);
		expect(d.controls.map((c) => c.channel)).toEqual([2, 3]);
		expect(d.controls[0].profile.rows[0].value).toBe(2.5);
		expect(d.controls[0].phase[0].phase_deg).toBe(90);
		setControl(d, TABLE, 1, true);
		// a control coming back starts from the first control's reference
		expect(d.controls.map((c) => c.channel)).toEqual([1, 2, 3]);
		expect(d.controls[0].profile.rows[0].value).toBe(2.5);
		// a limit channel on a row that becomes a control goes
		setControl(d, TABLE, 3, false);
		d.limits.channels.push({ channel: 3, profile: d.controls[0].profile });
		setControl(d, TABLE, 3, true);
		expect(d.limits.channels).toEqual([]);
	});

	it('sends every field the protocol names and reads it back unchanged', () => {
		const d = filled();
		d.test.kind = 'stepped';
		d.test.stepped = { ...d.test.stepped, on: 'list', frequencies_hz: [40, 80, 160], by: 'cycles', cycles: 500 };
		d.control.rotation_cap_deg = 20;
		d.requests.honour = true;
		d.simulator.structure = 'sine-tall-6x3';
		d.law = { id: 'matrix', params: { carry: 0.5 } };
		const wire = toMsineConfig(d, null);
		expect(wire.test).toEqual({ kind: 'stepped', frequencies_hz: [40, 80, 160], dwell: { cycles: 500 } });
		expect(wire.control?.rotation_cap_deg).toBe(20);
		expect(wire.channels.controls[0].phase).toEqual([{ freq_hz: 20, phase_deg: 0 }]);
		expect(Object.keys(wire).sort()).toEqual(['acquisition', 'article', 'channels', 'control', 'feasibility', 'identification', 'limits', 'metadata', 'notes', 'operator', 'phase_specified', 'record', 'requests', 'schedule', 'simulator', 'specification', 'test', 'title']);
		for (const kind of ['sweep', 'dwell', 'stepped'] as const) {
			d.test.kind = kind;
			const w = toMsineConfig(d, null);
			expect(canonical(toMsineConfig(fromMsineConfig(w), null)), kind).toBe(canonical(w));
		}
	});

	it('sends no phase with amplitudes only', () => {
		const d = filled();
		d.phase_specified = false;
		const wire = toMsineConfig(d, null);
		expect(wire.phase_specified).toBe(false);
		expect(wire.channels.controls.every((c) => c.phase === undefined)).toBe(true);
		expect(localErrors(d)['controls.0.phase']).toBeUndefined();
	});

	it('names what is missing before anything is sent', () => {
		const d = defaultMsineSetup();
		expect(localErrors(d)['channels.controls']).toEqual(['Switch on at least one control row in the channel table.']);
		fitChannels(d, TABLE);
		const e = localErrors(d);
		expect(e['drives.0.ceiling_vpk']?.[0]).toMatch(/no default/);
		d.drives.forEach((x) => ((x.ceiling_vpk = 2), (x.abort_vpk = 2)));
		expect(localErrors(d)['drives.0.abort_vpk']).toEqual(['The abort is above the ceiling.']);
		d.drives.forEach((x) => (x.abort_vpk = 3));
		d.controls[0].phase[0].phase_deg = -180;
		expect(localErrors(d)['controls.0.phase']).toEqual(['Row 1: a phase above -180 and at most 180 degrees.']);
		d.controls[0].phase[0].phase_deg = 180;
		d.test.kind = 'stepped';
		d.test.stepped.on = 'list';
		expect(localErrors(d)['test.dwell']).toEqual(['A list of frequencies needs a dwell in seconds or cycles at each.']);
		d.test.stepped.by = 'seconds';
		expect(Object.keys(localErrors(d))).toEqual(['schedule.per_level', 'schedule.full_level']);
	});

	it("says why a law cannot run the setup, in plain words", () => {
		const ctx = { kind: 'sweep' as const, phase_specified: true, controls: 4, drives: 4 };
		expect(lawRefusal(law('matrix', ['sweep', 'dwell', 'stepped'], true), ctx)).toBeNull();
		expect(lawRefusal(law('secant', ['dwell'], false), ctx)).toBe('Not for a sweep: secant accepts a dwell only.');
		expect(lawRefusal(law('ratio', ['dwell', 'stepped'], true), { ...ctx, kind: 'sweep' })).toBe('Not for a sweep: ratio accepts a dwell or a stepped sine.');
		expect(lawRefusal(law('secant', ['dwell'], false), { ...ctx, kind: 'dwell', phase_specified: false })).toBe('Needs a phase on every reference: secant does not choose the phases itself.');
		expect(lawRefusal(law('sq', ['dwell'], true, ['square']), { ...ctx, kind: 'dwell', controls: 6, drives: 3 })).toBe('Not for more controls than drives: sq does not accept that shape.');
		expect(shapeOf(3, 6)).toBe('fewer_controls_than_drives');
	});

	it('puts each service finding under the field it names', () => {
		const d = filled();
		d.limits.channels.push({ channel: 6, profile: d.controls[0].profile });
		const k = (field: string | null, reason = 'out_of_range', extra: Record<string, unknown> = {}) => msineItemKey({ field, reason, ...extra } as never, d);
		expect(k('config.channels.controls[1].profile.rows[0].value')).toBe('controls.1.profile.rows');
		expect(k('channels.controls[2].phase')).toBe('controls.2.phase');
		expect(k('channels.drives[1].abort_vpk')).toBe('drives.1.abort_vpk');
		expect(k('limits.channels[0].profile')).toBe('limits.channels.0.profile.rows');
		expect(k('control.law.params.carry')).toBe('law.params.carry');
		expect(k('control.law.id')).toBe('law.id');
		expect(k('test.frequencies_hz[2]')).toBe('test.frequencies_hz');
		expect(k(null, 'profile_not_covering', { row: 0, channel: 6 })).toBe('limits.channels.0.profile.rows');
		expect(k(null, 'structure_wrong_size')).toBe('simulator.structure');
		expect(msineSectionOf('controls.1.phase')).toBe('references');
		expect(msineSectionOf('simulator.structure')).toBe('channels');
		expect(msineSectionOf('requests.max_hold_s')).toBe('law');
		expect(msineSectionOf('feasibility.shakers.0')).toBe('limits');
		expect(msineSectionOf('identification.level_db')).toBe('schedule');
	});

	it('reads a stored draft back, filling what is missing, and titles it from the references and the test', () => {
		const d = filled();
		const back = mergeMsineSetup(JSON.parse(JSON.stringify({ ...d, test: { kind: 'dwell' }, control: undefined })));
		expect(back.test.kind).toBe('dwell');
		expect(back.test.sweep.oct_per_min).toBe(2);
		expect(back.control.abort_ticks).toBe(3);
		expect(back.controls.map((c) => c.channel)).toEqual([1, 2, 3]);
		expect(msineTitle(back)).toBe('MIMO Sine 1 g, dwell at 100 Hz');
		expect(msineTitle(d)).toBe('MIMO Sine 1 g, 20 to 2000 Hz');
	});
});
