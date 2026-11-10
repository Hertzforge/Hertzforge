import { describe, expect, it } from 'vitest';
import type { Series, SpectrumView, View } from '$lib/protocol/types';
import { DASH_STYLES } from '$lib/viz/roles';
import { axisRange, bandOf, cropWindow, envelopeOf, isShown, overlay, present, traceList, traceOverrides, withoutFamilies, withScales } from './present';
import { defaultAxis, newPlotSpec, type PlotType } from './types';

const RESPONSE: PlotType = { id: 'response', words: 'Response Spectra', help: '', group: 'frequency', channels: true, time: false };
const TIME: PlotType = { id: 'time', words: 'Time History', help: '', group: 'time', channels: true, time: true };

const s = (name: string, x: number[], y: number[], role: Series['role'] = 'measured'): Series => ({ name, role, x: Float64Array.from(x), y: Float64Array.from(y) });
const psd = (title: string, series: Series[]): SpectrumView => ({ view: 'spectrum', title, x: { label: 'Frequency', unit: 'Hz', scale: 'log' }, y: { label: 'PSD', unit: 'g²/Hz', scale: 'log' }, series, link: 'frequency' });
const wave = (title: string, series: Series[]): SpectrumView => ({ view: 'spectrum', title, x: { label: 'Time', unit: 's', scale: 'linear' }, y: { label: 'Amplitude', unit: 'g', scale: 'linear' }, series });

describe('scales and families', () => {
	it('keeps the service scale on auto and replaces it otherwise', () => {
		const v = psd('C', [s('Control', [1], [1])]);
		expect(withScales(v, { x: defaultAxis(), y: defaultAxis() })).toBe(v);
		const lin = withScales(v, { x: defaultAxis(), y: { ...defaultAxis(), scale: 'linear' } });
		expect(lin.y.scale).toBe('linear');
		expect(lin.x.scale).toBe('log');
	});

	it('leaves out the families switched off', () => {
		const v = psd('C', [s('Control', [1], [1]), s('Abort high', [1], [2], 'abort'), s('Alarm high', [1], [2], 'tolerance')]);
		expect(withoutFamilies(v, ['abort']).series.map((x) => x.name)).toEqual(['Control', 'Alarm high']);
		expect(withoutFamilies(v, [])).toBe(v);
	});
});

describe('time plots', () => {
	it('crops to the last seconds of the newest sample', () => {
		const v = wave('Ch', [s('Channel 1', [0, 0.25, 0.5, 0.75, 1], [1, 2, 3, 4, 5])]);
		const cropped = cropWindow(v, 0.5);
		expect(Array.from(cropped.series[0].x)).toEqual([0.5, 0.75, 1]);
		expect(Array.from(cropped.series[0].y)).toEqual([3, 4, 5]);
		expect(cropWindow(v, null)).toBe(v);
	});

	it('draws an envelope as each interval\'s smallest and largest sample', () => {
		const x = Array.from({ length: 8 }, (_, i) => i);
		const env = envelopeOf(s('Ch', x, [1, -1, 2, -2, 3, -3, 4, -4]), 2);
		expect(Array.from(env.y)).toEqual([2, 4]);
		expect(Array.from(env.y_min!)).toEqual([-2, -4]);
		expect(Array.from(env.x)).toEqual([1.75, 5.25]);
	});

	it('keeps a short series as it is, each sample its own minimum', () => {
		const env = envelopeOf(s('Ch', [0, 1], [3, 4]), 10);
		expect(Array.from(env.y)).toEqual([3, 4]);
		expect(Array.from(env.y_min!)).toEqual([3, 4]);
	});

	it('uses a decimated series\' own minimum', () => {
		const series = { ...s('Ch', [0, 1, 2, 3], [1, 1, 1, 1]), y_min: Float64Array.from([-1, -5, -1, -1]) };
		expect(Array.from(envelopeOf(series, 2).y_min!)).toEqual([-5, -1]);
	});
});

describe('several channels on one pane', () => {
	it('tells apart a name two views use by the view\'s title', () => {
		const a = psd('Channel 1', [s('Response', [1], [1]), s('Limit', [1], [2], 'limit')]);
		const b = psd('Channel 2', [s('Channel 2', [1], [3]), s('Limit', [1], [4], 'limit')]);
		const v = overlay([a, b], 'Responses');
		expect(v.title).toBe('Responses');
		expect(v.series.map((x) => x.name)).toEqual(['Response', 'Limit, Channel 1', 'Channel 2', 'Limit, Channel 2']);
	});

	it('tells them apart by channel when the channels are known', () => {
		const a = psd('Channel 1: Accel', [s('Response', [1], [1]), s('Limit', [1], [2], 'limit')]);
		const b = psd('Channel 4: Monitor', [s('Response', [1], [3])]);
		expect(overlay([a, b], 'Responses', [1, 4]).series.map((x) => x.name)).toEqual(['Response, channel 1', 'Limit', 'Response, channel 4']);
	});

	it('keeps a follower on its own measurement', () => {
		const a = psd('A', [s('M', [1], [1])]);
		const b = psd('B', [s('N', [1], [1]), { ...s('Fit', [1], [1]), follows: 0 }]);
		expect(overlay([a, b], 'T').series[2].follows).toBe(1);
	});
});

describe('the band and axis ranges', () => {
	it('reads the band from the reference, or the specification lines', () => {
		expect(bandOf([psd('C', [s('Control', [5, 3000], [1, 1]), s('Reference', [20, 2000], [1, 1], 'reference')])])).toEqual([20, 2000]);
		expect(bandOf([psd('C', [s('Limit', [50, 500], [1, 1], 'limit')])])).toEqual([50, 500]);
		expect(bandOf([psd('C', [s('Control', [5, 3000], [1, 1])])])).toBeNull();
	});

	it('reads a stepped reference to its outer bin edges', () => {
		const ref: Series = { ...s('Reference', [25, 35], [1, 1], 'reference'), shape: 'steps', edges: Float64Array.from([20, 30, 40]) };
		expect(bandOf([psd('C', [ref])])).toEqual([20, 40]);
	});

	it('holds a range only when it is whole and fits the scale', () => {
		const fixed = { ...defaultAxis(), range: 'fixed' as const, min: 0, max: 10 };
		expect(axisRange(fixed, false)).toEqual([0, 10]);
		expect(axisRange(fixed, true)).toBeNull();
		expect(axisRange({ ...fixed, max: null }, false)).toBeNull();
		expect(axisRange({ ...fixed, range: 'held' }, false)).toEqual([0, 10]);
		expect(axisRange({ ...fixed, range: 'auto' }, false)).toBeNull();
		expect(axisRange({ ...fixed, range: 'band' }, true, [20, 2000])).toEqual([20, 2000]);
	});
});

describe('trace looks', () => {
	it('applies the person\'s choices by name', () => {
		const v = psd('C', [s('Control', [1], [1]), s('Reference', [1], [1], 'reference')]);
		const o = traceOverrides(v, { Reference: { visible: false, width: 2.5, dash: 'dotted', color: '#e69f00' } });
		expect(o[0]).toBeUndefined();
		expect(o[1]).toEqual({ visible: false, width: 2.5, dash: DASH_STYLES.dotted.dash, dashName: 'dotted', color: '#e69f00', marker: null });
		expect(isShown(o, 0)).toBe(true);
		expect(isShown(o, 1)).toBe(false);
	});

	it('gives the second and later measurements on a pane a marker each, unless one is chosen', () => {
		const v = psd('C', [s('A', [1], [1]), s('B', [1], [1]), s('Ref', [1], [1], 'reference'), s('C', [1], [1])]);
		expect(traceOverrides(v, {}).map((o) => o?.marker ?? null)).toEqual([null, 'circle', null, 'square']);
		expect(traceOverrides(v, { B: { visible: true, width: null, dash: null, color: null, marker: 'none' } })[1]?.marker).toBeNull();
		expect(traceOverrides(psd('C', [s('A', [1], [1])]), {})).toEqual([undefined]);
	});
});

describe('a panel presented', () => {
	const channels: View[] = [
		psd('Channel 1', [s('Channel 1', [10, 100], [1, 2]), s('Limit', [10, 100], [3, 3], 'limit')]),
		psd('Channel 2', [s('Channel 2', [10, 100], [2, 1])]),
		{ view: 'reading', title: 'Run', rows: [] }
	];

	it('draws one pane per channel, or all on one', () => {
		const spec = newPlotSpec(RESPONSE, [1, 2]);
		const split = present(channels, spec, RESPONSE);
		expect(split.panes.map((p) => p.view.title)).toEqual(['Channel 1', 'Channel 2']);
		expect(split.layout).toBe('grid');
		expect(split.overlaid).toBe(false);
		expect(split.readings.length).toBe(1);
		const one = present(channels, { ...spec, arrange: 'overlay' }, RESPONSE);
		expect(one.panes.length).toBe(1);
		expect(one.panes[0].view.title).toBe('Response Spectra');
		expect(traceList(one).map((t) => t.name)).toEqual(['Channel 1', 'Limit', 'Channel 2']);
	});

	it('keys and names split panes by channel, when there is a channel for every view', () => {
		const spec = newPlotSpec(RESPONSE, [3, 7]);
		const untitled = [psd('', [s('A', [1], [1])]), psd('Channel 7', [s('B', [1], [1])])];
		const p = present(untitled, spec, RESPONSE, { channels: [3, 7] });
		expect(p.panes.map((x) => [x.key, x.view.title])).toEqual([
			['channel 3', 'Channel 3'],
			['channel 7', 'Channel 7']
		]);
		expect(present(untitled, spec, RESPONSE, { channels: [3] }).panes.map((x) => x.key)).toEqual(['0:', '1:Channel 7']);
	});

	it('stacks a time plot\'s channels as lanes when asked, and overlays them by default', () => {
		const views = [wave('Ch 1', [s('Channel 1', [0, 1], [0, 1])]), wave('Ch 2', [s('Channel 2', [0, 1], [1, 0])])];
		const spec = newPlotSpec(TIME, 'all');
		expect(present(views, spec, TIME).panes.length).toBe(1);
		const lanes = present(views, { ...spec, display: { ...spec.display, lanes: 'split' } }, TIME);
		expect(lanes.panes.length).toBe(2);
		expect(lanes.layout).toBe('lanes');
	});

	it('holds x to the band on a frequency plot', () => {
		const spec = newPlotSpec(RESPONSE, [1]);
		spec.display.x = { ...spec.display.x, range: 'band' };
		const p = present([psd('C', [s('Control', [5, 3000], [1, 1])])], spec, RESPONSE, { band: [20, 2000] });
		expect(p.panes[0].xFixed).toEqual([20, 2000]);
	});
});
