import { describe, expect, it } from 'vitest';
import type { ParamDescriptor, ProjectObject } from '$lib/protocol/types';
import { defaultFor, defaultValues, hasObjectsFor, objectKinds, objectVerb, withSettings, parseNumber, rangeHint, reconcile, validate, validateAll, withObjects } from './params';

const channels = [
	{ value: 'ctrl1', label: 'Control 1' },
	{ value: 'resp1', label: 'Response 1' }
];

const tol: ParamDescriptor = { name: 'tolerance_db', label: 'Tolerance', type: 'number', unit: 'dB', default: 3, min: 0.5, max: 6, step: 0.5 };
const avg: ParamDescriptor = { name: 'averages', label: 'Averages', type: 'integer', default: 64, min: 8, max: 512 };
const ch: ParamDescriptor = { name: 'channel', label: 'Channel', type: 'enum', options_from: 'channels', default: 'resp1' };
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
		expect(defaultValues([tol, avg, ch, flag], channels)).toEqual({ tolerance_db: 3, averages: 64, channel: 'resp1', unity: true });
	});

	it('falls back sensibly when a default is missing or invalid', () => {
		expect(defaultFor({ ...tol, default: undefined })).toBe(0.5);
		expect(defaultFor({ ...tol, default: 99 })).toBe(6);
		expect(defaultFor({ ...avg, default: 2.5 })).toBe(8);
		expect(defaultFor(mode)).toBe('a');
		expect(defaultFor({ ...ch, default: 'nope' }, channels)).toBe('ctrl1');
		expect(defaultFor({ ...flag, default: undefined })).toBe(false);
		expect(defaultFor(label)).toBe('');
	});

	it('fills options_from channels from the dataset, and is empty without one', () => {
		expect(defaultFor(ch, [])).toBe('');
		expect(validate(ch, '', [])).toMatch(/Open a dataset/);
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
		expect(validate(ch, 'resp1', channels)).toBeNull();
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

	it('keeps still-valid values when channels change', () => {
		const prev = { channel: 'resp1', tolerance_db: 4 };
		expect(reconcile([ch, tol], prev, channels)).toEqual({ channel: 'resp1', tolerance_db: 4 });
		expect(reconcile([ch, tol], prev, [{ value: 'x', label: 'X' }])).toEqual({ channel: 'x', tolerance_db: 4 });
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
