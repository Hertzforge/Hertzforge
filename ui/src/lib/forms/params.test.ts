import { describe, expect, it } from 'vitest';
import type { ParamDescriptor } from '$lib/protocol/types';
import { defaultFor, defaultValues, parseNumber, rangeHint, reconcile, validate, validateAll } from './params';

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
