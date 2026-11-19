import { describe, expect, it } from 'vitest';
import type { ParamDescriptor, ProjectObject } from '$lib/protocol/types';
import { defaultFor, defaultValues, hasObjectsFor, objectKinds, objectVerb, withSettings, parseNumber, rangeHint, reconcile, rescaled, rescaledQuantities, validate, validateAll, withDisplayLengths, withDisplayQuantities, withObjects } from './params';

const tol: ParamDescriptor = { name: 'tolerance_db', label: 'Tolerance', type: 'number', unit: 'dB', default: 3, min: 0.5, max: 6, step: 0.5 };
const avg: ParamDescriptor = { name: 'averages', label: 'Averages', type: 'integer', default: 64, min: 8, max: 512 };
const mode: ParamDescriptor = {
	name: 'mode',
	label: 'Mode',
	type: 'enum',
	options: [
		{ value: 'a', label: 'A' },
		{ value: 'b', label: 'B' }
	]
};
const flag: ParamDescriptor = { name: 'unity', label: 'Unity', type: 'boolean', default: true };
const label: ParamDescriptor = { name: 'label', label: 'Label', type: 'string' };

describe('defaults', () => {
	it('uses valid defaults as given', () => {
		expect(defaultValues([tol, avg, mode, flag])).toEqual({ tolerance_db: 3, averages: 64, mode: 'a', unity: true });
	});

	it('starts a second object param of the same objects on another object', () => {
		const sets: ParamDescriptor[] = [
			{ name: 'first', label: 'Basis', type: 'enum', options_from: 'objects:shape_set', options: [{ value: 'A', label: 'A' }, { value: 'B', label: 'B' }] },
			{ name: 'second', label: 'Compared', type: 'enum', options_from: 'objects:shape_set', options: [{ value: 'A', label: 'A' }, { value: 'B', label: 'B' }] },
			{ name: 'geometry', label: 'Geometry', type: 'enum', options_from: 'objects:geometry', options: [{ value: 'G', label: 'G' }] }
		];
		expect(defaultValues(sets)).toEqual({ first: 'A', second: 'B', geometry: 'G' });
		const one = sets.map((p) => (p.options_from === 'objects:shape_set' ? { ...p, options: [{ value: 'A', label: 'A' }] } : p));
		expect(defaultValues(one)).toEqual({ first: 'A', second: 'A', geometry: 'G' });
	});

	it('offers (none) first for an optional object param, and does not hide the tool for want of one', () => {
		const levels: ParamDescriptor = { name: 'levels', label: 'Q read off', type: 'enum', default: '', options_from: 'objects:sine_level_set' };
		const [filled] = withObjects([levels], [{ name: 'Levels', kind: 'sine_level_set', groups: [] }]);
		expect(filled.options).toEqual([
			{ value: '', label: '(none)' },
			{ value: 'Levels', label: 'Levels' }
		]);
		expect(defaultValues([filled])).toEqual({ levels: '' });
		expect(hasObjectsFor(withObjects([levels], []))).toBe(true);
	});

	it('falls back sensibly when a default is missing or invalid', () => {
		expect(defaultFor({ ...tol, default: undefined })).toBe(0.5);
		expect(defaultFor({ ...tol, default: 99 })).toBe(6);
		expect(defaultFor({ ...avg, default: 2.5 })).toBe(8);
		expect(defaultFor(mode)).toBe('a');
		expect(defaultFor({ ...mode, default: 'nope' })).toBe('a');
		expect(defaultFor({ ...flag, default: undefined })).toBe(false);
		expect(defaultFor(label)).toBe('');
	});
});

describe('validation', () => {
	it('checks numbers against their range, in words', () => {
		expect(validate(tol, 3)).toBeNull();
		expect(validate(tol, 7)).toBe('Between 0.5 and 6 dB.');
		expect(validate(tol, NaN)).toBe('Enter a number.');
		expect(validate({ ...tol, max: undefined }, 0.1)).toBe('At least 0.5 dB.');
	});

	it('requires whole numbers for integers', () => {
		expect(validate(avg, 64)).toBeNull();
		expect(validate(avg, 64.5)).toBe('Enter a whole number.');
	});

	it('requires enum values from the options', () => {
		expect(validate(mode, 'b')).toBeNull();
		expect(validate(mode, 'c')).toBe('Choose one of the options.');
	});

	it('collects every error by name', () => {
		expect(validateAll([tol, avg, flag], { tolerance_db: 9, averages: 64, unity: 'yes' as unknown as boolean })).toEqual({
			tolerance_db: 'Between 0.5 and 6 dB.',
			unity: 'Choose on or off.'
		});
	});
});

describe('parsing and hints', () => {
	it('parses typed numbers strictly', () => {
		expect(parseNumber(' 2.5 ')).toBe(2.5);
		expect(parseNumber('-3')).toBe(-3);
		expect(parseNumber('−3')).toBe(-3);
		expect(parseNumber('1e-3')).toBe(0.001);
		expect(parseNumber('')).toBeNaN();
		expect(parseNumber('3 dB')).toBeNaN();
		expect(parseNumber('1.2.3')).toBeNaN();
	});

	it('describes ranges', () => {
		expect(rangeHint(tol)).toBe('0.5 to 6 dB');
		expect(rangeHint(flag)).toBeNull();
	});

	it('keeps still-valid values when the options change', () => {
		const prev = { mode: 'b', tolerance_db: 4 };
		expect(reconcile([mode, tol], prev)).toEqual({ mode: 'b', tolerance_db: 4 });
		expect(reconcile([{ ...mode, options: [{ value: 'x', label: 'X' }] }, tol], prev)).toEqual({ mode: 'x', tolerance_db: 4 });
	});
});

describe('object params', () => {
	const data: ParamDescriptor = { name: 'data', label: 'Data', type: 'enum', options_from: 'objects:psd|frf' };
	const objects: ProjectObject[] = [
		{ name: 'Channel Table', kind: 'channel_table', groups: [] },
		{ name: 'Run FRFs', kind: 'frf', groups: [] },
		{ name: 'Run PSDs', kind: 'psd', groups: [] }
	];

	it('lists the project\'s objects of the kinds a param takes, in the project\'s order', () => {
		expect(objectKinds(data)).toEqual(['psd', 'frf']);
		const [filled] = withObjects([data], objects);
		expect(filled.options?.map((o) => o.value)).toEqual(['Run FRFs', 'Run PSDs']);
		expect(defaultFor(filled)).toBe('Run FRFs');
		expect(validate(filled, 'Run PSDs')).toBeNull();
		expect(validate(filled, 'Channel Table')).toBe('Choose one of the options.');
	});

	it('says which kind is missing when the project has none', () => {
		const [filled] = withObjects([{ ...data, options_from: 'objects:shape_set' }], objects);
		expect(validate(filled, '')).toBe('The project has no shape set.');
	});

	it('leaves other params alone', () => {
		const number: ParamDescriptor = { name: 'n', label: 'N', type: 'number' };
		expect(withObjects([number], objects)[0]).toBe(number);
	});

	it('lists the objects a verb applies to, by the verbs the summary gives each', () => {
		const source: ParamDescriptor = { name: 'source', label: 'Source', type: 'enum', options_from: 'verb:integrate' };
		const histories: ProjectObject[] = [
			{ name: 'Forces', kind: 'time_history', groups: [], verbs: ['filter_data'] },
			{ name: 'Accels', kind: 'time_history', groups: [], verbs: ['integrate', 'filter_data'] },
			{ name: 'Older', kind: 'time_history', groups: [] }
		];
		expect(objectVerb(source)).toBe('integrate');
		const [filled] = withObjects([source], histories);
		expect(filled.options?.map((o) => o.value)).toEqual(['Accels']);
		const [none] = withObjects([source], histories.slice(0, 1));
		expect(validate(none, '')).toBe('Nothing in the project takes this.');
	});

	it('says a tool has nothing to run on when an object param lists nothing', () => {
		const rate: ParamDescriptor = { name: 'rate', label: 'Rate', type: 'number' };
		expect(hasObjectsFor(withObjects([data, rate], objects))).toBe(true);
		expect(hasObjectsFor(withObjects([{ ...data, options_from: 'objects:shape_set' }, rate], objects))).toBe(false);
		expect(hasObjectsFor(withObjects([{ ...data, options_from: 'verb:integrate' }], objects))).toBe(false);
		expect(hasObjectsFor([rate])).toBe(true);
	});
});

describe('setting params', () => {
	const params: ParamDescriptor[] = [
		{ name: 'source', label: 'Source', type: 'enum', options_from: 'objects:time_history' },
		{ name: 'frame_length', label: 'Frame', type: 'integer', default: 4096, min: 2, setting: 'averaging.frame_length' },
		{ name: 'window', label: 'Window', type: 'enum', default: 'hann', setting: 'averaging.window', options: [{ value: 'hann', label: 'Hann' }, { value: 'flattop', label: 'Flat top' }] }
	];
	const run: ProjectObject = { name: 'Run', kind: 'time_history', groups: [], settings: { averaging: { frame_length: 8192, window: 'flattop' } } };

	it('opens on the settings the source object holds', () => {
		const out = withSettings(params, { source: 'Run', frame_length: 4096, window: 'hann' }, run);
		expect(out).toEqual({ source: 'Run', frame_length: 8192, window: 'flattop' });
	});

	it('keeps the values where the object has no such setting, or one the param would refuse', () => {
		const bare: ProjectObject = { name: 'Bare', kind: 'time_history', groups: [] };
		expect(withSettings(params, { source: 'Bare', frame_length: 2048, window: 'hann' }, bare).frame_length).toBe(2048);
		const odd: ProjectObject = { ...run, settings: { averaging: { frame_length: 1, window: 'bartlett' } } };
		expect(withSettings(params, { source: 'Run', frame_length: 2048, window: 'hann' }, odd)).toEqual({ source: 'Run', frame_length: 2048, window: 'hann' });
	});
});

describe('lengths typed in the display system', () => {
	const width: ParamDescriptor = { name: 'width_x', label: 'Width along X', type: 'number', unit: 'm', default: 1, length: true };
	const widths: ParamDescriptor = { name: 'widths', label: 'Widths [m] (empty keeps it)', type: 'string', default: '', length: true };
	const sizes: ParamDescriptor = { name: 'sizes', label: 'A size per piece [m], as "3: 0.01"', type: 'string', default: '', length: true };
	const angle: ParamDescriptor = { name: 'angle_x', label: 'Angle', type: 'number', unit: 'deg', default: 0 };

	it('names the display unit, for the unit and in the label', () => {
		const [w, ws, a] = withDisplayLengths([width, widths, angle], 'in');
		expect(w.unit).toBe('in');
		expect(ws.label).toBe('Widths [in] (empty keeps it)');
		expect(ws.unit).toBeUndefined();
		expect(a).toBe(angle);
	});

	it('names no unit on an undefined geometry', () => {
		const [w, ws] = withDisplayLengths([width, widths], null);
		expect(w.unit).toBeUndefined();
		expect(ws.label).toBe('Widths (empty keeps it)');
	});

	it('keeps the same length when the system changes, a piece id staying what it is', () => {
		const ratio = 0.0254;
		const out = rescaled([width, widths, sizes, angle], { width_x: 2, widths: '1, 2 3', sizes: '3: 100, 5: 50', angle_x: 90 }, ratio);
		expect(out.width_x).toBeCloseTo(0.0508);
		expect(out.widths).toBe('0.0254, 0.0508 0.0762');
		expect(out.sizes).toBe('3: 2.54, 5: 1.27');
		expect(out.angle_x).toBe(90);
	});

	it('leaves a field left empty empty', () => {
		expect(rescaled([widths], { widths: '' }, 0.0254).widths).toBe('');
	});
});

describe('a quantity typed in the display system', () => {
	// Brandon, 2026-11-11: a mate's stiffness was asked for in N/m with inches on screen
	const stiffness = { name: 'k', label: 'Normal stiffness', type: 'number', default: 1e6, unit: 'N/m', quantity: 'force/length' } as ParamDescriptor;
	const plain = { name: 'n', label: 'Count', type: 'number', default: 3, unit: 'N/m' } as ParamDescriptor;
	const units = { 'force/length': { unit: 'lbf/in', to_si: 175.1268352464764 } };

	it('shows its unit and its default in the system', () => {
		const [k, n] = withDisplayQuantities([stiffness, plain], units);
		expect(k.unit).toBe('lbf/in');
		expect(k.default).toBe(5710);
		expect(n).toBe(plain);
	});

	it('is left in SI when the system has no unit for it', () => {
		expect(withDisplayQuantities([stiffness], {})[0]).toBe(stiffness);
	});

	it('holds the same stiffness when the system changes', () => {
		const out = rescaledQuantities([stiffness, plain], { k: 1000, n: 3 }, { 'force/length': 175.1268352464764 });
		expect(out.k).toBeCloseTo(175126.8352, 3);
		expect(out.n).toBe(3);
	});
});
