/**
 * Seeds: an analysis object in the open project fills part of a test's draft (`project.seeds`,
 * `project.seed`). The analysis service converts the object and answers the part it fills as
 * `sets`, each a dotted member of the configure request and the value put there whole; this module
 * puts them into a copy of the draft's configuration exactly as the analysis side's `seeds.apply`
 * does, and names every member the draft would change before it takes them.
 *
 * A protective number the object gave (an abort band, a ceiling) is never among the `sets`: it
 * comes under `protective`, and is put in only once the operator has typed it. A module whose seed
 * needs a file stored on the control service first (a TWR reference) names the request under
 * `imports`; it is sent only when the operator takes the seed, and the control service checks it as
 * it checks a file the operator picks. Nothing here applies, arms or starts anything: the draft then
 * goes through the same configure call and checks as one typed in.
 */

import { configDifferences, type SetupModule, type SetupOrigin } from './setupFile';

/** The modules a project object can seed, in the order a person meets them. */
export const SEED_MODULES: readonly SetupModule[] = ['random', 'sine', 'srs', 'classical_shock', 'twr', 'mixed', 'mimo_random'];

/** One object `project.seeds` offers. */
export interface SeedOffer {
	name: string;
	kind: string;
	fills: string;
	holds: string;
	/** The channels (or records) it could be read at. */
	channels: string[];
	/** Mixed Mode: the tones beside the floor. */
	tones?: string;
}

export interface SeedSet {
	path: string;
	value: unknown;
}

export interface ProtectiveNumber extends SeedSet {
	/** What the number is, in words. */
	words: string;
}

export interface SeedImport {
	method: string;
	params: Record<string, unknown>;
	/** Where the stored reference is put. */
	set: string;
}

/** What `project.seed` answers. */
export interface SeedAnswer {
	module: SetupModule;
	sets: SeedSet[];
	protective: ProtectiveNumber[];
	imports: SeedImport[];
	notes: string[];
	origin: Extract<SetupOrigin, { kind: 'seed' }>;
}

type Config = Record<string, unknown>;

const isObject = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);

/** A plain copy, of reactive state too. */
const copy = <T>(v: T): T => (v === undefined ? v : (JSON.parse(JSON.stringify(v)) as T));

/** `config` with each of `sets` put in, in order: the analysis side's `seeds.apply`. */
export function applySets(config: Config, sets: readonly SeedSet[]): Config {
	const out = copy(config);
	for (const s of sets) {
		const keys = s.path.split('.');
		let at: unknown = out;
		for (const key of keys.slice(0, -1)) {
			if (Array.isArray(at)) {
				if (!isObject(at[Number(key)]) && !Array.isArray(at[Number(key)])) at[Number(key)] = {};
				at = at[Number(key)];
				continue;
			}
			const o = at as Record<string, unknown>;
			if (!isObject(o[key]) && !Array.isArray(o[key])) o[key] = {};
			at = o[key];
		}
		const last = keys[keys.length - 1];
		const value = copy(s.value);
		if (Array.isArray(at)) at[Number(last)] = value;
		else (at as Record<string, unknown>)[last] = value;
	}
	return out;
}

/** The numbers of a protective value, in order. */
function numbersOf(v: unknown): number[] | null {
	if (typeof v === 'number') return [v];
	if (Array.isArray(v) && v.every((x) => typeof x === 'number')) return v as number[];
	return null;
}

/** A protective value as the operator types it: its numbers, separated by commas. */
export function typedForm(v: unknown): string {
	const n = numbersOf(v);
	return n ? n.map((x) => String(x)).join(', ') : JSON.stringify(v);
}

/** Whether `text` is the protective value `v`, typed: the same numbers, in order. */
export function typedMatches(text: string, v: unknown): boolean {
	const want = numbersOf(v);
	if (!want) return text.trim() === JSON.stringify(v);
	const got = text
		.split(/[\s,;]+/)
		.filter((t) => t !== '')
		.map(Number);
	return got.length === want.length && got.every((x, i) => Number.isFinite(x) && Math.abs(x - want[i]) <= 1e-9 * Math.max(1, Math.abs(want[i])));
}

/** What a seed would change, waiting for the operator. */
export interface SeedVetting {
	refused: string | null;
	/** Every member the draft would change, in words, the protective numbers among them. */
	changes: string[];
	/** What the seed sets that this screen's draft cannot hold exactly. */
	lost: string[];
	/** The configuration with the sets and the protective numbers in, before the imports. */
	merged: Config | null;
}

/** Whether a seed may fill this screen's draft, and what the operator must see first. */
export function vetSeed(
	answer: SeedAnswer,
	ctx: { module: SetupModule; protocol: string | null; config: Config | null; roundTrip: (c: Config) => Config | null; canImport: boolean }
): SeedVetting {
	const refuse = (refused: string): SeedVetting => ({ refused, changes: [], lost: [], merged: null });
	if (answer.module !== ctx.module) return refuse(`This seed is for ${answer.module}, not this screen.`);
	if (ctx.protocol === null) return refuse('Connect to the control service first: a seeded setup is checked by it before it can be applied.');
	if (!ctx.config) return refuse('The draft makes no setup yet, so there is nothing to seed into.');
	if (answer.imports.length && !ctx.canImport) return refuse('This screen cannot store what the seed needs on the control service.');
	const merged = applySets(ctx.config, [...answer.sets, ...answer.protective]);
	let ours: Config | null;
	try {
		ours = ctx.roundTrip(merged);
	} catch (err) {
		return refuse(`This screen cannot hold the seed: ${err instanceof Error ? err.message : String(err)}.`);
	}
	if (!ours) return refuse('This screen cannot make a setup from the seed.');
	const changes = configDifferences(ctx.config, ours).map(seedWords);
	for (const imp of answer.imports) changes.push(`${imp.set}: the record "${String(imp.params.name ?? answer.origin.object)}", stored on the control service when the seed is taken`);
	return { refused: null, changes, lost: configDifferences(merged, ours), merged };
}

/** A difference between the draft and the seeded draft, said of the seed. */
function seedWords(line: string): string {
	return line
		.replace(/^(.*): (.*) in the setup; this screen would send (.*)$/, '$1: $2 now; the seed makes it $3')
		.replace(/^(.*): not in the setup; this screen adds (.*)$/, '$1: the seed adds $2')
		.replace(/^(.*): in the setup \((.*)\); this screen does not keep it$/, '$1: $2 now; the seed takes it out');
}
