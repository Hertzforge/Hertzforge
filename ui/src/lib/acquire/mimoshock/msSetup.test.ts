/**
 * The MIMO Shock setup draft: fitted to the channel table (controls, drives and limits from its
 * enabled rows, each control's target following it), what the editor names before anything is
 * sent, where a service finding lands, and the wire's units.
 */
import { describe, expect, it } from 'vitest';
import type { ChannelRow } from '$lib/protocol/types';
import { defaultMsSetup, fitChannels, limitRows, localErrors, msItemKey, msSectionOf, parseLevels, sameTargetForAll, setControl, toMsConfig, type MsDraft } from './msSetup';

const row = (index: number, role: ChannelRow['role'], enabled = true): ChannelRow =>
	({ index, enabled, description: '', role, physical: null, interface: 'single_ended', iepe_bias_v: null, units: role === 'drive' ? 'V' : 'g', sensitivity_mv_per_eu: 100, range_max: 10, polarity: 'positive', control_group: null, abort: {} }) as unknown as ChannelRow;

const TABLE = [row(1, 'control'), row(2, 'control'), row(3, 'control'), row(4, 'monitor'), row(5, 'drive'), row(6, 'drive')];

function ready(): MsDraft {
	const d = defaultMsSetup();
	fitChannels(d, TABLE);
	d.law.id = 'a-law';
	for (const x of d.drives) {
		x.ceiling_vpk = 4;
		x.abort_vpk = 6;
	}
	return d;
}

describe('the MIMO Shock channels', () => {
	it('takes every enabled control row as a control and every drive row as a drive, in the table order', () => {
		const d = defaultMsSetup();
		expect(fitChannels(d, TABLE)).toBe(true);
		expect(d.controls).toEqual([1, 2, 3]);
		expect(d.drives.map((x) => x.channel)).toEqual([5, 6]);
		// one target and one held delay per control
		expect([d.pulses.length, d.srs.controls.length, d.timing.delays_ms.length]).toEqual([3, 3, 3]);
		// nothing changes when the table has not
		expect(fitChannels(d, TABLE)).toBe(false);
	});

	it('keeps the whole draft while the table has not been read', () => {
		const d = ready();
		expect(fitChannels(d, [])).toBe(false);
		expect(d.drives.map((x) => x.ceiling_vpk)).toEqual([4, 4]);
		expect(d.controls).toEqual([1, 2, 3]);
	});

	it("keeps each control's own target with it as rows go off and on", () => {
		const d = defaultMsSetup();
		fitChannels(d, TABLE);
		d.pulses[2].amplitude_g = 55;
		const fewer = TABLE.map((r) => (r.index === 2 ? { ...r, enabled: false } : r));
		fitChannels(d, fewer);
		expect(d.controls).toEqual([1, 3]);
		expect(d.pulses[1].amplitude_g).toBe(55);
		fitChannels(d, TABLE);
		expect(d.controls).toEqual([1, 2, 3]);
		expect(d.pulses[2].amplitude_g).toBe(55);
	});

	it('keeps a drive ceiling across a refit, and asks for a new drive row\'s', () => {
		const d = ready();
		fitChannels(d, [...TABLE, row(7, 'drive')]);
		expect(d.drives.map((x) => [x.channel, x.ceiling_vpk])).toEqual([[5, 4], [6, 4], [7, NaN]]);
		expect(localErrors(d)['drives.2.ceiling_vpk']?.[0]).toMatch(/no default/);
	});

	it('lets a control row become a limit, and a monitor row be one', () => {
		const d = ready();
		expect(limitRows(d, TABLE).map((r) => r.index)).toEqual([4]);
		setControl(d, TABLE, 3, false);
		expect(d.controls).toEqual([1, 2]);
		expect(limitRows(d, TABLE).map((r) => r.index)).toEqual([3, 4]);
		d.limits = [{ channel: 3, abort_peak: 50 }];
		setControl(d, TABLE, 3, true);
		expect(d.controls).toEqual([1, 2, 3]);
		expect(d.limits).toEqual([]);
	});

	it("copies the first control's target to every control", () => {
		const d = ready();
		d.pulses[0].amplitude_g = 33;
		sameTargetForAll(d);
		expect(d.pulses.map((p) => p.amplitude_g)).toEqual([33, 33, 33]);
		d.pulses[1].amplitude_g = 1;
		expect(d.pulses[0].amplitude_g).toBe(33);
	});
});

describe('what the editor names before sending', () => {
	it('a setup with its drives given checks clean', () => {
		expect(localErrors(ready())).toEqual({});
	});

	it('asks for every drive ceiling and abort, the abort above the ceiling', () => {
		const d = defaultMsSetup();
		fitChannels(d, TABLE);
		const e = localErrors(d);
		expect(Object.keys(e).filter((k) => k.startsWith('drives.'))).toEqual(['drives.0.ceiling_vpk', 'drives.0.abort_vpk', 'drives.1.ceiling_vpk', 'drives.1.abort_vpk']);
		const r = ready();
		r.drives[1].abort_vpk = 3;
		expect(localErrors(r)['drives.1.abort_vpk']).toEqual(['The abort is above the ceiling.']);
	});

	it('names an empty table, levels that do not rise to full level, and a hold without its delays', () => {
		expect(localErrors(defaultMsSetup())['channels.controls']).toBeDefined();
		const d = ready();
		d.schedule.levels_db = [-6, -9, 0];
		expect(localErrors(d)['schedule.levels_db']).toEqual(['The levels rise from one to the next.']);
		d.schedule.levels_db = [-12, -6];
		expect(localErrors(d)['schedule.levels_db']).toEqual(['The last level is full level, 0 dB.']);
		d.schedule.levels_db = [-12, 0];
		d.timing.mode = 'hold';
		d.timing.delays_ms = [0.1];
		expect(localErrors(d)['timing.delays_ms']).toEqual(['Give one delay for every control.']);
	});

	it('keeps the global verdict to an SRS target', () => {
		const d = ready();
		d.verdict = 'global';
		expect(localErrors(d).verdict?.[0]).toMatch(/SRS target/);
		d.kind = 'srs';
		expect(localErrors(d).verdict).toBeUndefined();
	});

	it('asks SRS breakpoints to rise in frequency', () => {
		const d = ready();
		d.kind = 'srs';
		d.srs.controls[1].points[2].freq_hz = 100;
		expect(localErrors(d)['srs.controls.1.points.2.freq_hz']).toEqual(['Frequencies rise from point to point.']);
	});
});

describe('the wire', () => {
	it('sends the timing in seconds, the held delays only when held, and the scaled aborts switch', () => {
		const d = ready();
		d.timing = { mode: 'hold', window_ms: 1.5, delays_ms: [0, 0.25, 1] };
		d.control.scaled_aborts = false;
		const c = toMsConfig(d, null);
		expect(c.timing).toEqual({ mode: 'hold', window_s: 0.0015, delays_s: [0, 0.00025, 0.001] });
		expect(c.control?.scaled_aborts).toBe(false);
		expect(c.channels).toEqual({ controls: [{ channel: 1 }, { channel: 2 }, { channel: 3 }], drives: [{ channel: 5, ceiling_vpk: 4, abort_vpk: 6 }, { channel: 6, ceiling_vpk: 4, abort_vpk: 6 }], limits: [] });
		d.timing.mode = 'free';
		expect(toMsConfig(d, null).timing?.delays_s).toBeNull();
	});

	it('sends one target per control of the kind chosen', () => {
		const d = ready();
		expect(toMsConfig(d, null).target).toMatchObject({ kind: 'waveform', standard: 'mil_std_810_516' });
		d.kind = 'srs';
		const t = toMsConfig(d, null).target;
		expect(t.kind === 'srs' && t.controls.length).toBe(3);
		expect(t.kind === 'srs' && t.controls[0].target).toEqual({ kind: 'breakpoints', points: [[50, 4], [300, 24], [2500, 24]] });
	});

	it("puts the service's findings on the setup's own fields and sections", () => {
		const k = (field: string, reason = 'out_of_range') => msItemKey({ field, reason });
		expect(k('config.target.pulses[2].amplitude_g')).toBe('pulses.2.amplitude_g');
		expect(k('config.target.controls[1].target')).toBe('srs.controls.1.points');
		expect(k('config.target.controls[1].synthesis.tol_db')).toBe('srs.controls.1.tol_db');
		expect(k('config.timing.delays_s[3]')).toBe('timing.delays_ms.3');
		expect(k('config.channels.drives[1].abort_vpk')).toBe('drives.1.abort_vpk');
		expect(k('config.control.law.params.gain')).toBe('law.params.gain');
		expect(k('config.control.law.id', 'law_wrong_target')).toBe('law.id');
		expect(k('config.schedule.levels_db[2]')).toBe('schedule.levels_db');
		expect(msItemKey({ reason: 'x', channel: 3 })).toBe('channels');
		expect(['pulses.0.amplitude_g', 'drives.1.abort_vpk', 'timing.window_ms', 'schedule.levels_db', 'law.params.gain', 'control.scaled_aborts'].map(msSectionOf)).toEqual(['target', 'channels', 'timing', 'ladder', 'law', 'limits']);
	});

	it('reads a ladder as the operator types it', () => {
		expect(parseLevels('-12, -9 -6;-3,  -1.5, 0')).toEqual([-12, -9, -6, -3, -1.5, 0]);
		expect(parseLevels('-12, x')[1]).toBeNaN();
	});
});
