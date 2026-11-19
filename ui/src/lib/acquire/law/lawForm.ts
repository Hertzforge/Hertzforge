/**
 * The control law as a setup holds it, for every test type that offers a law choice (MIMO Random,
 * classical shock): the law's parameter form from its manifest, the values the operator keeps
 * across a change of law, the words for a value out of the manifest's range, and the parameters
 * a setup sends. Pure functions only. Nothing here is specific to a law or a test type: a law's
 * manifest (`laws.list`, PROTOCOL.md "Control Laws") says what its form holds.
 */

import type { ParamDescriptor } from '$lib/protocol/types';
import type { LawEntry, LawParam, LawWorld } from '$lib/protocol/mimo';

/** One law parameter's value as a draft keeps it. */
export type LawValue = number | string | boolean | number[] | [number, number][];

/** The law a draft has chosen and its parameter values, by key. */
export interface LawDraft {
	id: string;
	params: Record<string, LawValue>;
}

/** Findings by the editor's key (`law.id`, `law.params.<key>`). */
export type LawErrors = Record<string, string[]>;

/** The laws of one world, in the catalog's order: built in first, then imported. */
export function lawsOfWorld(laws: readonly LawEntry[] | null | undefined, world: LawWorld): LawEntry[] {
	return (laws ?? []).filter((l) => l.world === world);
}

/** A law parameter the shared form draws, or null for one the panel draws itself. */
export function lawParamDescriptor(p: LawParam): ParamDescriptor | null {
	switch (p.kind) {
		case 'number':
			return { name: p.key, label: p.label, type: 'number', unit: p.unit || null, default: p.default, min: p.min, max: p.max, step: p.step };
		case 'integer':
			return { name: p.key, label: p.label, type: 'integer', unit: p.unit || null, default: p.default, min: p.min, max: p.max, step: p.step };
		case 'toggle':
			return { name: p.key, label: p.label, type: 'boolean', default: p.default };
		case 'choice':
			return { name: p.key, label: p.label, type: 'enum', default: p.default, options: p.options.map((o) => ({ value: o, label: choiceWord(o) })) };
		default:
			return null;
	}
}

/** A manifest's choice value in words: "tikhonov" reads "Tikhonov", "per_line" reads "Per line". */
export function choiceWord(v: string): string {
	const s = v.replaceAll('_', ' ');
	return s.charAt(0).toUpperCase() + s.slice(1);
}

/**
 * The law's values: kept where the operator set a valid one, the manifest's default elsewhere. A
 * per-channel parameter has one value for each of `controls` controls.
 */
export function lawValues(entry: LawEntry, previous: Record<string, LawValue> | undefined, controls: number): Record<string, LawValue> {
	const out: Record<string, LawValue> = {};
	for (const p of entry.params) {
		const prev = previous?.[p.key];
		switch (p.kind) {
			case 'number':
			case 'integer':
				out[p.key] = typeof prev === 'number' && Number.isFinite(prev) ? prev : p.default;
				break;
			case 'toggle':
				out[p.key] = typeof prev === 'boolean' ? prev : p.default;
				break;
			case 'choice':
				out[p.key] = typeof prev === 'string' && p.options.includes(prev) ? prev : p.default;
				break;
			case 'per_channel': {
				const list = Array.isArray(prev) ? (prev as number[]) : [];
				out[p.key] = Array.from({ length: controls }, (_, i) => (typeof list[i] === 'number' ? list[i] : p.default));
				break;
			}
			case 'curve':
				out[p.key] = Array.isArray(prev) && prev.every((x) => Array.isArray(x)) ? prev : p.default.map((x) => [x[0], x[1]] as [number, number]);
				break;
		}
	}
	return out;
}

/** Words for a law parameter's value that is out of its manifest's range, or null. */
export function lawParamError(p: LawParam, v: LawValue | undefined): string | null {
	const range = (x: number, min: number, max: number, unit: string) =>
		!Number.isFinite(x) ? 'Enter a number.' : x < min || x > max ? `From ${min} to ${max}${unit ? ` ${unit}` : ''}.` : null;
	switch (p.kind) {
		case 'number':
			return typeof v === 'number' ? range(v, p.min, p.max, p.unit) : 'Enter a number.';
		case 'integer':
			return typeof v === 'number' ? (Number.isInteger(v) ? range(v, p.min, p.max, p.unit) : 'Enter a whole number.') : 'Enter a whole number.';
		case 'per_channel':
			return Array.isArray(v) ? ((v as number[]).map((x) => range(x, p.min, p.max, p.unit)).find((e) => e) ?? null) : 'Give one value per control.';
		case 'curve':
			if (!Array.isArray(v) || !v.length) return 'Give at least one point.';
			for (let i = 0; i < v.length; i++) {
				const pt = v[i] as [number, number];
				if (!Number.isFinite(pt[0]) || !Number.isFinite(pt[1])) return `Point ${i + 1}: enter numbers.`;
				if (i > 0 && pt[0] <= (v[i - 1] as [number, number])[0]) return `Point ${i + 1}: frequencies rise from point to point.`;
				const e = range(pt[1], p.min, p.max, p.unit);
				if (e) return `Point ${i + 1}: ${e}`;
			}
			return null;
		default:
			return null;
	}
}

/** Each parameter out of its manifest's range, by `law.params.<key>`. */
export function lawParamErrors(law: LawEntry | null, values: Record<string, LawValue>): LawErrors {
	const out: LawErrors = {};
	if (!law) return out;
	for (const p of law.params) {
		const e = lawParamError(p, values[p.key]);
		if (e) (out[`law.params.${p.key}`] ??= []).push(e);
	}
	return out;
}

/** The parameter values a setup sends: those the law's manifest declares, by key. */
export function lawWireParams(law: LawEntry | null, values: Record<string, LawValue>): Record<string, unknown> {
	const params: Record<string, unknown> = {};
	if (law) for (const p of law.params) if (p.key in values) params[p.key] = values[p.key];
	return params;
}

/**
 * The labels of the chosen law's parameters that may change during a run, for a test type whose
 * laws declare them (Sine); null when the test type marks none. Only a number parameter can be live.
 */
export function liveParamLabels(law: LawEntry | null, live: readonly string[] | undefined): string[] | null {
	if (live === undefined) return null;
	return (law?.params ?? []).filter((p) => live.includes(p.key) && (p.kind === 'number' || p.kind === 'integer')).map((p) => p.label);
}

/** A law's check record in a word or two. */
export const CHECK_WORDS: Record<LawEntry['check']['status'], string> = {
	built_in: 'Built in',
	passed: 'Check passed',
	unchecked: 'Not checked',
	failed: 'Check failed',
	unreadable: 'Check record unreadable'
};

/** A file hash short enough to read; a built-in law has none. */
export function shortHash(hash: string | null): string {
	return hash ? `${hash.slice(0, 12)}...${hash.slice(-6)}` : 'Built in';
}
