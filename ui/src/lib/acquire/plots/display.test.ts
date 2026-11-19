import { describe, expect, it } from 'vitest';
import { plotTitle, readChannels, readDisplay, readPanelSpec, saveDisplay, savePanelSpec, withType } from './display';
import { defaultDisplay, newPlotSpec, type PlotType } from './types';

const PSD: PlotType = { id: 'control', words: 'Control and Reference', help: 'The control spectrum.', group: 'frequency', channels: false, time: false, families: ['measured', 'reference', 'tolerance', 'abort'] };
const TIME: PlotType = { id: 'time', words: 'Time History', help: 'Samples.', group: 'time', channels: true, time: true };

describe('reading a stored display', () => {
	it('gives the defaults for anything that is not a display', () => {
		for (const raw of [null, undefined, 3, 'x', [], [1, 2]]) {
			const d = readDisplay(raw);
			expect({ ...d, cursorAt: undefined, holdEpoch: undefined }).toEqual({ ...defaultDisplay(), cursorAt: undefined, holdEpoch: undefined });
			expect(d.cursorAt).toEqual([null, null]);
		}
	});

	it('keeps each good field and replaces each bad one with its default', () => {
		const d = readDisplay({
			grid: 'minor',
			legend: 'sideways',
			legendCorner: 'bottom-left',
			readout: 'yes',
			cursor: 'peaks',
			count: 7,
			hold: 'average',
			average: 1,
			timeRender: 'envelope',
			lanes: 'split',
			window_s: -2,
			x: { scale: 'log', range: 'fixed', min: 10, max: 2000, label: '  Freq  ', ticks: 'dense' },
			y: { scale: 'cubic', range: 'held', min: 5, max: 1, unit: 42 }
		});
		expect(d.grid).toBe('minor');
		expect(d.legend).toBe('inside');
		expect(d.legendCorner).toBe('bottom-left');
		expect(d.readout).toBe(true);
		expect(d.cursor).toBe('peaks');
		expect(d.count).toBe(7);
		expect(d.hold).toBe('average');
		expect(d.average).toBe(8);
		expect(d.timeRender).toBe('envelope');
		expect(d.lanes).toBe('split');
		expect(d.window_s).toBeNull();
		expect(d.x).toEqual({ scale: 'log', range: 'fixed', min: 10, max: 2000, label: 'Freq', unit: null, ticks: 'dense' });
		// a range whose top is not above its bottom is dropped whole
		expect(d.y).toEqual({ scale: 'auto', range: 'held', min: null, max: null, label: null, unit: null, ticks: 'normal' });
	});

	it('refuses numbers that are not finite or not whole where they must be', () => {
		const d = readDisplay({ count: 2.5, average: Infinity, x: { min: Number.NaN, max: 3 }, cursorAt: [Infinity, 412.5], holdEpoch: -1 });
		expect(d.count).toBe(5);
		expect(d.average).toBe(8);
		expect(d.x.min).toBeNull();
		expect(d.x.max).toBe(3);
		expect(d.cursorAt).toEqual([null, 412.5]);
		expect(d.holdEpoch).toBe(0);
	});

	it('keeps only known line families, in their own order', () => {
		expect(readDisplay({ hidden: ['abort', 'bogus', 'measured', 3] }).hidden).toEqual(['measured', 'abort']);
		expect(readDisplay({ hidden: 'abort' }).hidden).toEqual([]);
	});

	it('reads trace styles, keeping only colours from the palette', () => {
		const d = readDisplay({
			traces: {
				Control: { visible: false, width: 2, dash: 'dotted', color: '#E69F00', marker: 'square' },
				Drive: { width: 99, dash: 'wavy', color: 'red', marker: 'star' },
				Bad: 'not a style'
			}
		});
		expect(d.traces.Control).toEqual({ visible: false, width: 2, dash: 'dotted', color: '#e69f00', marker: 'square' });
		expect(d.traces.Drive).toEqual({ visible: true, width: null, dash: null, color: null, marker: null });
		expect(d.traces.Bad).toBeUndefined();
	});

	it('reads notes, clamped to the plot and each id once', () => {
		const d = readDisplay({
			notes: [
				{ id: 'a', text: 'Resonance', fx: 1.4, fy: -1 },
				{ id: 'a', text: 'Again', fx: 0.2, fy: 0.2 },
				{ text: '   ', fx: 0.1, fy: 0.1 },
				{ text: 'No id', fx: 'left' }
			]
		});
		expect(d.notes).toEqual([
			{ id: 'a', text: 'Resonance', fx: 1, fy: 0 },
			{ id: 'n4', text: 'No id', fx: 0.5, fy: 0.2 }
		]);
	});

	it('caps what a stored layout may hold', () => {
		const notes = Array.from({ length: 80 }, (_, i) => ({ id: `n${i}`, text: 'x'.repeat(500), fx: 0, fy: 0 }));
		const d = readDisplay({ notes });
		expect(d.notes.length).toBe(50);
		expect(d.notes[0].text.length).toBe(200);
	});
});

describe('reading a stored panel', () => {
	it('needs a quantity, and one the test offers when the types are given', () => {
		expect(readPanelSpec({})).toBeNull();
		expect(readPanelSpec({ quantity: '' })).toBeNull();
		expect(readPanelSpec({ quantity: 'srs' }, [PSD, TIME])).toBeNull();
		expect(readPanelSpec({ quantity: 'time' }, [PSD, TIME])?.arrange).toBe('overlay');
		expect(readPanelSpec({ quantity: 'control' }, [PSD, TIME])?.arrange).toBe('split');
	});

	it('reads channels as a preset or as whole channel numbers, each once and in order', () => {
		expect(readChannels('control')).toBe('control');
		expect(readChannels([3, 1, 3, -1, 2.5, '4'])).toEqual([1, 3]);
		expect(readChannels('some')).toBe('all');
	});

	it('takes a point count only within bounds', () => {
		expect(readPanelSpec({ quantity: 'time', points: 4096 })?.points).toBe(4096);
		expect(readPanelSpec({ quantity: 'time', points: 10 })?.points).toBe(1024);
		expect(readPanelSpec({ quantity: 'time', points: 1e9 })?.points).toBe(1024);
	});
});

describe('saving', () => {
	it('round-trips through storage unchanged', () => {
		const spec = newPlotSpec(TIME, [2, 5]);
		spec.title = 'Responses';
		spec.display.traces = { B: { visible: false, width: 2, dash: 'dashed', color: '#0072b2', marker: 'cross' }, A: { visible: true, width: null, dash: null, color: null } };
		spec.display.notes = [{ id: 'n1', text: 'Peak', fx: 0.3, fy: 0.4 }];
		spec.display.x = { ...spec.display.x, range: 'fixed', min: 0, max: 0.5 };
		const saved = JSON.parse(JSON.stringify(savePanelSpec(spec)));
		const back = readPanelSpec(saved, [PSD, TIME]);
		expect(back).not.toBeNull();
		expect(savePanelSpec(back!)).toEqual(saved);
	});

	it('writes the same text for the same choice, whatever order its traces were set in', () => {
		const a = defaultDisplay();
		a.traces = { Zeta: { visible: true, width: 1, dash: null, color: null }, Alpha: { visible: false, width: null, dash: null, color: null } };
		const b = defaultDisplay();
		b.traces = { Alpha: { visible: false, width: null, dash: null, color: null }, Zeta: { visible: true, width: 1, dash: null, color: null } };
		b.hidden = [];
		expect(JSON.stringify(saveDisplay(a))).toBe(JSON.stringify(saveDisplay(b)));
		expect(Object.keys(saveDisplay(a).traces)).toEqual(['Alpha', 'Zeta']);
	});
});

describe('titles and types', () => {
	it('names a plot by its title, or its type', () => {
		expect(plotTitle({ title: null, quantity: 'control' }, PSD)).toBe('Control and Reference');
		expect(plotTitle({ title: 'Mine', quantity: 'control' }, PSD)).toBe('Mine');
		expect(plotTitle({ title: null, quantity: 'control' }, null)).toBe('control');
	});

	it('starts a changed type on its own axes, lines and arrangement', () => {
		const spec = newPlotSpec(PSD);
		spec.display.x = { ...spec.display.x, scale: 'linear', range: 'fixed', min: 1, max: 2 };
		spec.display.hidden = ['abort'];
		spec.display.hold = 'peak';
		const next = withType(spec, TIME);
		expect(next.quantity).toBe('time');
		expect(next.arrange).toBe('overlay');
		expect(next.display.x.range).toBe('auto');
		expect(next.display.hidden).toEqual([]);
		expect(next.display.hold).toBe('none');
	});
});
