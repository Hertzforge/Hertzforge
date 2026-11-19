/**
 * The shared law form: what a manifest's every parameter kind becomes on screen, the values kept
 * across a change of law, the words for a value out of range, the parameters a setup sends, and
 * the catalog filtered by world.
 */
import { describe, expect, it } from 'vitest';
import type { LawEntry, LawParam } from '$lib/protocol/mimo';
import { lawParamDescriptor, lawParamError, lawParamErrors, lawsOfWorld, lawValues, lawWireParams, shortHash } from './lawForm';

const PARAMS: LawParam[] = [
	{ key: 'beta', label: 'Correction gain', help: 'How much of the error each pulse corrects.', kind: 'number', unit: '', min: 0.05, max: 1, step: 0.05, default: 0.5 },
	{ key: 'passes', label: 'Passes', help: '', kind: 'integer', unit: 'pulses', min: 1, max: 10, step: 1, default: 3 },
	{ key: 'correct', label: 'Correct', help: '', kind: 'toggle', default: true },
	{ key: 'window', label: 'Window', help: '', kind: 'choice', options: ['hann', 'flat_top'], default: 'hann' },
	{ key: 'weight', label: 'Weight', help: '', kind: 'per_channel', unit: '', min: 0, max: 10, step: 0.5, default: 1 },
	{ key: 'shape', label: 'Shape', help: '', kind: 'curve', unit: 'dB', min: -20, max: 20, default: [[10, 0], [1000, -3]] }
];

function entry(over: Partial<LawEntry> = {}): LawEntry {
	return {
		id: 'fd_correct',
		world: 'shock_classical',
		builtin: true,
		name: 'frequency-domain correction',
		version: '0.1.0',
		author: 'Hertzforge',
		summary: 'Corrects the next drive by the measured error on the pulse grid.',
		sha256: null,
		check: { status: 'built_in', date: null, tool_version: null, cases: [], hardware: true, message: 'Built in: cleared for hardware.' },
		params: PARAMS,
		diagnostics: [],
		shapes: null,
		needs: null,
		...over
	};
}

describe('a law parameter form from its manifest', () => {
	it('draws number, integer, toggle and choice with the shared form, and leaves the rest to the panel', () => {
		const forms = PARAMS.map(lawParamDescriptor);
		expect(forms.map((f) => f?.type ?? null)).toEqual(['number', 'integer', 'boolean', 'enum', null, null]);
		expect(forms[0]).toMatchObject({ name: 'beta', min: 0.05, max: 1, step: 0.05, default: 0.5, unit: null });
		expect(forms[1]).toMatchObject({ unit: 'pulses', default: 3 });
		expect(forms[3]?.options).toEqual([
			{ value: 'hann', label: 'Hann' },
			{ value: 'flat_top', label: 'Flat top' }
		]);
	});

	it('starts each kind at its default, keeping what the operator set when it is valid', () => {
		const fresh = lawValues(entry(), undefined, 2);
		expect(fresh).toEqual({ beta: 0.5, passes: 3, correct: true, window: 'hann', weight: [1, 1], shape: [[10, 0], [1000, -3]] });
		const kept = lawValues(entry(), { beta: 0.25, passes: NaN, correct: false, window: 'nope', weight: [4], shape: [[5, 1]] }, 3);
		expect(kept).toEqual({ beta: 0.25, passes: 3, correct: false, window: 'hann', weight: [4, 1, 1], shape: [[5, 1]] });
		// The default curve is copied, never shared with the manifest.
		(fresh.shape as [number, number][])[0][1] = 9;
		expect((PARAMS[5] as { default: [number, number][] }).default[0][1]).toBe(0);
	});

	it('names a value out of its range in words, by kind', () => {
		expect(lawParamError(PARAMS[0], 2)).toBe('From 0.05 to 1.');
		expect(lawParamError(PARAMS[0], NaN)).toBe('Enter a number.');
		expect(lawParamError(PARAMS[1], 2.5)).toBe('Enter a whole number.');
		expect(lawParamError(PARAMS[1], 11)).toBe('From 1 to 10 pulses.');
		expect(lawParamError(PARAMS[2], true)).toBeNull();
		expect(lawParamError(PARAMS[4], [1, 11])).toBe('From 0 to 10.');
		expect(lawParamError(PARAMS[5], [])).toBe('Give at least one point.');
		expect(lawParamError(PARAMS[5], [[100, 0], [50, 0]])).toContain('rise');
		expect(lawParamError(PARAMS[5], [[10, 30]])).toBe('Point 1: From -20 to 20 dB.');
		expect(lawParamErrors(entry(), { ...lawValues(entry(), undefined, 1), beta: 3 })).toEqual({ 'law.params.beta': ['From 0.05 to 1.'] });
	});

	it('sends only the parameters the manifest declares', () => {
		const values = { ...lawValues(entry(), undefined, 1), stale: 7 };
		expect(Object.keys(lawWireParams(entry(), values)).sort()).toEqual(['beta', 'correct', 'passes', 'shape', 'weight', 'window']);
		expect(lawWireParams(null, values)).toEqual({});
	});
});

describe('the catalog by world', () => {
	it('keeps the order the service lists and only the world asked for', () => {
		const laws = [entry({ id: 'pinv_closed', world: 'mimo' }), entry(), entry({ id: 'abc', builtin: false, sha256: 'a'.repeat(64) }), entry({ id: 'ratio', world: 'single' })];
		expect(lawsOfWorld(laws, 'shock_classical').map((l) => l.id)).toEqual(['fd_correct', 'abc']);
		expect(lawsOfWorld(null, 'mimo')).toEqual([]);
		expect(shortHash('0123456789abcdef'.repeat(4))).toBe('0123456789ab...abcdef');
		expect(shortHash(null)).toBe('Built in');
	});
});
