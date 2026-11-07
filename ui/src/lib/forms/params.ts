/**
 * Defaults, options and validation for descriptor-built panels. Pure functions, so the form
 * component stays thin and the rules are testable on their own.
 */

import type { ParamDescriptor, ParamOption, ProjectObject } from '$lib/protocol/types';

export type ParamValue = number | string | boolean;
export type ParamValues = Record<string, ParamValue>;

/** The options an enum shows: its own list, filled from the project's objects for `options_from`. */
export function optionsFor(p: ParamDescriptor): ParamOption[] {
	return p.type === 'enum' ? (p.options ?? []) : [];
}

/** The kinds an `objects:` param takes, or none for any other param. */
export function objectKinds(p: ParamDescriptor): string[] {
	return p.options_from?.startsWith('objects:') ? p.options_from.slice('objects:'.length).split('|') : [];
}

/** The core verb a `verb:` param's objects take, or '' for any other param. */
export function objectVerb(p: ParamDescriptor): string {
	return p.options_from?.startsWith('verb:') ? p.options_from.slice('verb:'.length) : '';
}

/**
 * The params with each `objects:` param's options filled from the project's objects of its kinds,
 * and each `verb:` param's from the objects its verb applies to, in the project's order, so the
 * form and its checks treat it as any other enum.
 */
export function withObjects(params: ParamDescriptor[], objects: ProjectObject[]): ParamDescriptor[] {
	return params.map((p) => {
		const kinds = objectKinds(p);
		const verb = objectVerb(p);
		if (!kinds.length && !verb) return p;
		const taken = objects.filter((o) => (verb ? (o.verbs ?? []).includes(verb) : kinds.includes(o.kind)));
		const none = p.default === '' ? [{ value: '', label: '(none)' }] : [];
		return { ...p, options: [...none, ...taken.map((o) => ({ value: o.name, label: o.name }))] };
	});
}

/**
 * Whether a project tool has something to run on: every param filled from the project's objects
 * (after `withObjects`) lists at least one, an optional one (default '') aside. A tool that does not is not shown, as Visual Dynamics
 * offers a verb only where it applies.
 */
export function hasObjectsFor(params: ParamDescriptor[]): boolean {
	return params.every((p) => !(objectKinds(p).length || objectVerb(p)) || p.default === '' || (p.options ?? []).length > 0);
}

function clamp(v: number, p: ParamDescriptor): number {
	let out = v;
	if (p.min !== undefined && out < p.min) out = p.min;
	if (p.max !== undefined && out > p.max) out = p.max;
	return out;
}

/** The starting value for one parameter: its valid default, else the first sensible value. */
export function defaultFor(p: ParamDescriptor): ParamValue {
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
			const opts = optionsFor(p);
			if (opts.some((o) => o.value === d)) return d as string | number;
			return opts[0]?.value ?? '';
		}
	}
}

/**
 * Every param's starting value. A project-object param filled from the same objects as an earlier
 * one starts on an object the earlier ones have not taken, when there is one: two shape sets to
 * match or compare start as two sets, not one set twice.
 */
export function defaultValues(params: ParamDescriptor[]): ParamValues {
	const taken = new Map<string, Set<ParamValue>>();
	return Object.fromEntries(
		params.map((p) => {
			let value = defaultFor(p);
			const source = objectKinds(p).length || objectVerb(p) ? p.options_from : undefined;
			if (source) {
				const used = taken.get(source) ?? new Set();
				if (used.has(value)) value = optionsFor(p).find((o) => !used.has(o.value))?.value ?? value;
				used.add(value);
				taken.set(source, used);
			}
			return [p.name, value];
		})
	);
}

/** Words a user can act on, or null when the value is valid. */
export function validate(p: ParamDescriptor, value: unknown): string | null {
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
			const opts = optionsFor(p);
			if (opts.length === 0) {
				const kinds = objectKinds(p);
				if (kinds.length) return `The project has no ${kinds.map((k) => k.replace(/_/g, ' ')).join(' or ')}.`;
				return objectVerb(p) ? 'Nothing in the project takes this.' : 'No options.';
			}
			return opts.some((o) => o.value === value) ? null : 'Choose one of the options.';
		}
	}
}

export function validateAll(params: ParamDescriptor[], values: ParamValues): Record<string, string> {
	const errors: Record<string, string> = {};
	for (const p of params) {
		const e = validate(p, values[p.name]);
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

/**
 * The values with each `setting` param taken from the source object's settings, where the object
 * has that setting and it is a valid value; every other value is left as it was.
 */
export function withSettings(params: ParamDescriptor[], values: ParamValues, source: ProjectObject | undefined): ParamValues {
	const out = { ...values };
	for (const p of params) {
		if (!p.setting) continue;
		const [group, field] = p.setting.split('.');
		const held = source?.settings?.[group]?.[field];
		if (held !== undefined && validate(p, held) === null) out[p.name] = held;
	}
	return out;
}

/**
 * The params as the display system shows them: a `length` param in `unit`, for its unit and for
 * '[m]' in its label; with none, an undefined geometry's numbers, no unit at all.
 */
export function withDisplayLengths(params: ParamDescriptor[], unit: string | null): ParamDescriptor[] {
	return params.map((p) =>
		p.length ? { ...p, unit: p.unit && unit ? unit : undefined, label: p.label.replace(' [m]', unit ? ` [${unit}]` : '') } : p
	);
}

function roundedLength(value: number): number {
	return Number(value.toPrecision(10));
}

/**
 * What a form holds after the display system changed: each `length` param times `ratio`, the old
 * unit in the new one, so it is the same length; a piece's id before its colon is not a length.
 */
export function rescaled(params: ParamDescriptor[], values: ParamValues, ratio: number): ParamValues {
	const out = { ...values };
	for (const p of params) {
		if (!p.length) continue;
		const held = out[p.name];
		if (typeof held === 'number' && Number.isFinite(held)) out[p.name] = roundedLength(held * ratio);
		else if (typeof held === 'string') {
			out[p.name] = held.replace(/(\d+\s*:)|([-+]?(?:\d+\.?\d*|\.\d+)(?:e[-+]?\d+)?)/gi, (found, piece) => piece ?? String(roundedLength(Number(found) * ratio)));
		}
	}
	return out;
}

/** Keep values that are still valid after the descriptor or the project's objects change; reset the rest. */
export function reconcile(params: ParamDescriptor[], previous: ParamValues | undefined): ParamValues {
	const out = defaultValues(params);
	if (!previous) return out;
	for (const p of params) {
		if (p.name in previous && validate(p, previous[p.name]) === null) out[p.name] = previous[p.name];
	}
	return out;
}
