/**
 * Defaults, options and validation for descriptor-built panels. Pure functions, so the form
 * component stays thin and the rules are testable on their own.
 */

import type { ParamDescriptor, ParamOption } from '$lib/protocol/types';

export type ParamValue = number | string | boolean;
export type ParamValues = Record<string, ParamValue>;

/** The options an enum shows: its own list, or the dataset's channels for `options_from`. */
export function optionsFor(p: ParamDescriptor, channels: ParamOption[] = []): ParamOption[] {
	if (p.type !== 'enum') return [];
	if (p.options_from === 'channels') return channels;
	return p.options ?? [];
}

function clamp(v: number, p: ParamDescriptor): number {
	let out = v;
	if (p.min !== undefined && out < p.min) out = p.min;
	if (p.max !== undefined && out > p.max) out = p.max;
	return out;
}

/** The starting value for one parameter: its valid default, else the first sensible value. */
export function defaultFor(p: ParamDescriptor, channels: ParamOption[] = []): ParamValue {
	const d = p.default;
	switch (p.type) {
		case 'number': {
			if (typeof d === 'number' && Number.isFinite(d)) return clamp(d, p);
			return clamp(p.min ?? 0, p);
		}
		case 'integer': {
			if (typeof d === 'number' && Number.isInteger(d)) return clamp(d, p);
			return Math.round(clamp(p.min ?? 0, p));
		}
		case 'boolean':
			return typeof d === 'boolean' ? d : false;
		case 'string':
			return typeof d === 'string' ? d : '';
		case 'enum': {
			const opts = optionsFor(p, channels);
			if (opts.some((o) => o.value === d)) return d as string | number;
			return opts[0]?.value ?? '';
		}
	}
}

export function defaultValues(params: ParamDescriptor[], channels: ParamOption[] = []): ParamValues {
	return Object.fromEntries(params.map((p) => [p.name, defaultFor(p, channels)]));
}

/** Words a user can act on, or null when the value is valid. */
export function validate(p: ParamDescriptor, value: unknown, channels: ParamOption[] = []): string | null {
	const unit = p.unit ? ` ${p.unit}` : '';
	switch (p.type) {
		case 'number':
		case 'integer': {
			if (typeof value !== 'number' || !Number.isFinite(value)) return 'Enter a number.';
			if (p.type === 'integer' && !Number.isInteger(value)) return 'Enter a whole number.';
			if (p.min !== undefined && p.max !== undefined && (value < p.min || value > p.max)) {
				return `Between ${p.min} and ${p.max}${unit}.`;
			}
			if (p.min !== undefined && value < p.min) return `At least ${p.min}${unit}.`;
			if (p.max !== undefined && value > p.max) return `At most ${p.max}${unit}.`;
			return null;
		}
		case 'boolean':
			return typeof value === 'boolean' ? null : 'Choose on or off.';
		case 'string':
			return typeof value === 'string' ? null : 'Enter text.';
		case 'enum': {
			const opts = optionsFor(p, channels);
			if (opts.length === 0) return p.options_from === 'channels' ? 'Open a dataset to list its channels.' : 'No options.';
			return opts.some((o) => o.value === value) ? null : 'Choose one of the options.';
		}
	}
}

export function validateAll(params: ParamDescriptor[], values: ParamValues, channels: ParamOption[] = []): Record<string, string> {
	const errors: Record<string, string> = {};
	for (const p of params) {
		const e = validate(p, values[p.name], channels);
		if (e) errors[p.name] = e;
	}
	return errors;
}

/** Parse what was typed into a number field. Empty or junk gives NaN, which fails validation. */
export function parseNumber(text: string): number {
	const t = text.trim().replace(/−/g, '-');
	if (t === '' || !/^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$/i.test(t)) return NaN;
	return Number(t);
}

/** A short "0.5 to 6 dB" hint for a numeric field, or null. */
export function rangeHint(p: ParamDescriptor): string | null {
	if (p.type !== 'number' && p.type !== 'integer') return null;
	const unit = p.unit ? ` ${p.unit}` : '';
	if (p.min !== undefined && p.max !== undefined) return `${p.min} to ${p.max}${unit}`;
	if (p.min !== undefined) return `${p.min}${unit} or more`;
	if (p.max !== undefined) return `up to ${p.max}${unit}`;
	return null;
}

/** Keep values that are still valid after the descriptor or channels change; reset the rest. */
export function reconcile(params: ParamDescriptor[], previous: ParamValues | undefined, channels: ParamOption[] = []): ParamValues {
	const out = defaultValues(params, channels);
	if (!previous) return out;
	for (const p of params) {
		if (p.name in previous && validate(p, previous[p.name], channels) === null) out[p.name] = previous[p.name];
	}
	return out;
}
