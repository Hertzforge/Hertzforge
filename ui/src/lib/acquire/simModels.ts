/**
 * The simulator's models for the multiple-drive modules, in words. Which models there are, and
 * the table size each plays, come from the service's `station.list`; this file only says what
 * each one is, so the Configure view can name it. A model the service lists that is not here is
 * shown by its name.
 */
import type { SimModel } from '$lib/protocol/types';

/** MIMO Shock's plants, by name. */
export const SHOCK_PLANT_WORDS: Record<string, string> = {
	'shock-linear-4x4': 'Linear',
	'shock-jointed-4x4-moderate': 'Jointed, Moderate',
	'shock-jointed-4x4-severe': 'Jointed, Severe',
	'shock-jointed-6x3': 'Jointed'
};

/** MIMO Sine's and MIMO Mixed's sine structures, by name. */
export const SINE_STRUCTURE_WORDS: Record<string, string> = {
	'sine-linear-4x4': 'Linear',
	'sine-softening-4x4-mild': 'Softening With Level, Mild',
	'sine-softening-4x4-moderate': 'Softening With Level, Moderate',
	'sine-hardening-4x4': 'Hardening With Level',
	'sine-tall-6x3': 'Linear',
	'sine-wide-3x6': 'Linear',
	'sine-base-article-6x4': 'Base and Article',
	'sine-near-singular-4x4': 'Near-Singular at One Frequency',
	'sine-weak-4x4': 'Weakly Coupled Pairs',
	'sine-loopback-4x4': 'Loopback, Four Outputs Wired to Four Inputs'
};

/** A model as an option reads: what it is and the table size it plays. */
export function modelWords(m: SimModel, words: Record<string, string>): string {
	const what = words[m.name] ?? m.name;
	return `${what}, ${m.inputs} ${m.inputs === 1 ? 'input' : 'inputs'} by ${m.drives} ${m.drives === 1 ? 'drive' : 'drives'}`;
}
