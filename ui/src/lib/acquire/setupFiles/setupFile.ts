/**
 * A test setup as the project holds it (the `test_setup` kind, `project.save_setup`,
 * `project.open_setup`), and the checks a setup passes before it may fill a test's draft.
 *
 * Opening a setup fills the draft and nothing else. Before it does:
 *
 * - a setup for another test module is refused;
 * - a setup written under another protocol than the control service speaks is refused, and so is
 *   any setup while the control service is not connected, since nothing could check it;
 * - a setup made with a different channel table is flagged, row by row, and opens only when the
 *   operator says so;
 * - a setup the screen's draft cannot hold exactly (a member it would drop, add or change) is
 *   flagged, member by member, and opens only when the operator says so. Nothing is adapted
 *   without being named.
 *
 * Once in the draft, the setup goes through the same configure call and checks as one typed in.
 */

import type { ChannelRow } from '$lib/protocol/types';

/** The test modules, by the name a run's manifest gives its test. */
export type SetupModule = 'random' | 'sine' | 'mixed' | 'classical_shock' | 'srs' | 'twr' | 'explorer' | 'recorder' | 'mimo_random' | 'mimo_shock' | 'playback';

export const SETUP_MODULES: readonly SetupModule[] = ['random', 'sine', 'mixed', 'classical_shock', 'srs', 'twr', 'explorer', 'recorder', 'mimo_random', 'mimo_shock', 'playback'];

/** Each module's name on its screen. */
export const MODULE_TITLES: Record<SetupModule, string> = {
	random: 'Random',
	sine: 'Sine',
	mixed: 'Mixed Mode',
	classical_shock: 'Classical Shock',
	srs: 'SRS',
	twr: 'TWR',
	explorer: 'Explorer',
	recorder: 'Recorder',
	mimo_random: 'MIMO Random',
	mimo_shock: 'MIMO Shock',
	playback: 'Playback'
};

/**
 * Where a setup came from. A `seed` names the analysis object that filled part of it and the
 * object's SHA-256, so the setup says which version of the object it took.
 */
export type SetupOrigin =
	| { kind: 'typed' }
	| { kind: 'setup'; name: string }
	| { kind: 'run'; run_uid?: string; run_id?: string; name?: string }
	| { kind: 'seed'; object: string; checksum: string; fills?: string; also?: { object: string; checksum: string }[] };

/** The channel table a setup was made with, each member when known. */
export interface SetupTable {
	rows?: ChannelRow[];
	revision?: number;
	sha256?: string;
}

/** A test setup as `project.open_setup` answers it and `project.save_setup` takes it. */
export interface TestSetupObject {
	module: SetupModule;
	protocol: string;
	config: Record<string, unknown>;
	table: SetupTable;
	title: string;
	notes: string;
	origin: SetupOrigin;
}

export interface SavedSetupEntry {
	name: string;
	module: SetupModule;
	title: string;
	protocol: string;
	origin: SetupOrigin;
	holds: string;
}

export interface RunEntry {
	name: string;
	module: string;
	title: string;
	run_uid: string;
	run_id: string;
	outcome: string;
	source?: string | null;
	has_config: boolean;
	holds: string;
}

export interface SetupListing {
	setups: SavedSetupEntry[];
	runs: RunEntry[];
}

/** JSON with every object's keys sorted, so the same setup always gives the same text. */
export function canonical(v: unknown): string {
	return JSON.stringify(sorted(v));
}

function sorted(v: unknown): unknown {
	if (Array.isArray(v)) return v.map(sorted);
	if (v && typeof v === 'object') {
		const o = v as Record<string, unknown>;
		return Object.fromEntries(
			Object.keys(o)
				.filter((k) => o[k] !== undefined)
				.sort()
				.map((k) => [k, sorted(o[k])])
		);
	}
	return v;
}

const isObject = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);
const shown = (v: unknown): string => {
	const text = JSON.stringify(v);
	return text === undefined ? 'nothing' : text.length > 40 ? `${text.slice(0, 37)}...` : text;
};

/** Where two configurations differ, member by member, in words: what `ours` drops, adds or changes. */
export function configDifferences(theirs: unknown, ours: unknown, path = ''): string[] {
	if (canonical(theirs) === canonical(ours)) return [];
	if (isObject(theirs) && isObject(ours)) {
		const out: string[] = [];
		const keys = [...new Set([...Object.keys(theirs), ...Object.keys(ours)])].sort();
		for (const k of keys) {
			const at = path ? `${path}.${k}` : k;
			if (theirs[k] === undefined) out.push(`${at}: not in the setup; this screen adds ${shown(ours[k])}`);
			else if (ours[k] === undefined) out.push(`${at}: in the setup (${shown(theirs[k])}); this screen does not keep it`);
			else out.push(...configDifferences(theirs[k], ours[k], at));
		}
		return out;
	}
	if (Array.isArray(theirs) && Array.isArray(ours) && theirs.length === ours.length && theirs.length > 0) {
		// a list of the same length differs member by member
		return theirs.flatMap((t, i) => configDifferences(t, ours[i], path ? `${path}.${i}` : String(i)));
	}
	return [`${path || 'the setup'}: ${shown(theirs)} in the setup; this screen would send ${shown(ours)}`];
}

const ROW_WORDS: Record<string, string> = {
	enabled: 'on',
	role: 'role',
	units: 'units',
	sensitivity_mv_per_eu: 'sensitivity',
	range_max: 'range',
	physical: 'physical channel',
	interface: 'interface',
	iepe_bias_v: 'IEPE bias',
	polarity: 'polarity',
	control_group: 'control group',
	abort: 'kinematic aborts',
	description: 'description'
};

/** How the channel table a setup was made with differs from the one the service holds, by row. */
export function tableDifferences(made: ChannelRow[], now: ChannelRow[]): string[] {
	const out: string[] = [];
	const held = new Map(now.map((r) => [r.index, r]));
	const was = new Map(made.map((r) => [r.index, r]));
	for (const r of made) {
		const n = held.get(r.index);
		if (!n) {
			out.push(`Channel ${r.index}: in the setup's table, not in the table here`);
			continue;
		}
		const keys = [...new Set([...Object.keys(r), ...Object.keys(n)])].filter((k) => k !== 'index').sort();
		const changed = keys.filter((k) => canonical((r as unknown as Record<string, unknown>)[k]) !== canonical((n as unknown as Record<string, unknown>)[k]));
		if (changed.length) {
			out.push(
				`Channel ${r.index}: ${changed.map((k) => `${ROW_WORDS[k] ?? k} ${shown((r as unknown as Record<string, unknown>)[k])} in the setup, ${shown((n as unknown as Record<string, unknown>)[k])} here`).join('; ')}`
			);
		}
	}
	for (const n of now) if (!was.has(n.index)) out.push(`Channel ${n.index}: in the table here, not in the setup's table`);
	return out;
}

/** What a screen knows when it vets a setup to open. */
export interface VetContext {
	/** This screen's module. */
	module: SetupModule;
	/** The protocol the control service speaks, or null while it is not connected. */
	protocol: string | null;
	/** The channel table the control service holds, or null when it has not said. */
	rows: ChannelRow[] | null;
	/** The configuration this screen's draft would send after taking `config`, without taking it. */
	roundTrip: (config: Record<string, unknown>) => Record<string, unknown> | null;
}

/** The verdict on a setup to open: refused, with the reason; or the differences to confirm first. */
export interface Vetting {
	refused: string | null;
	/** Channel table differences: the setup was made with another table. */
	table: string[];
	/** Members the draft would drop, add or change. */
	config: string[];
}

/** Whether a setup may fill this screen's draft, and what the operator must see first. */
export function vetSetup(setup: TestSetupObject, ctx: VetContext): Vetting {
	const refuse = (refused: string): Vetting => ({ refused, table: [], config: [] });
	if (setup.module !== ctx.module) {
		const theirs = MODULE_TITLES[setup.module] ?? setup.module;
		return refuse(`This setup is for ${theirs}, not ${MODULE_TITLES[ctx.module]}. Open it on the ${theirs} screen.`);
	}
	if (ctx.protocol === null) return refuse('Connect to the control service first: an opened setup is checked by it before it can be applied.');
	if (setup.protocol !== ctx.protocol) {
		return refuse(`This setup was written under protocol ${setup.protocol}; the control service speaks ${ctx.protocol}. It is not opened, since its members may not mean the same.`);
	}
	if (!setup.config || typeof setup.config !== 'object') return refuse('The setup holds no configuration.');
	let ours: Record<string, unknown> | null;
	try {
		ours = ctx.roundTrip(setup.config);
	} catch (err) {
		return refuse(`This screen cannot read the setup: ${err instanceof Error ? err.message : String(err)}.`);
	}
	if (!ours) return refuse('This screen cannot make a setup from it.');
	const table = !setup.table?.rows
		? ['The setup does not say which channel table it was made with.']
		: ctx.rows === null
			? ['The control service has not said which channel table it holds.']
			: tableDifferences(setup.table.rows, ctx.rows);
	return { refused: null, table, config: configDifferences(setup.config, ours) };
}

/** Where the draft's setup came from, in words, and whether it was edited since. */
export function originWords(origin: SetupOrigin, edited: boolean): string {
	const whence =
		origin.kind === 'setup'
			? `the saved setup "${origin.name}"`
			: origin.kind === 'run'
				? `run ${origin.run_id || origin.run_uid || ''}`.trim()
				: origin.kind === 'seed'
					? `${origin.fills ? `${origin.fills} ` : ''}seeded from "${origin.object}" (SHA-256 ${origin.checksum.slice(0, 12)})`
					: '';
	if (origin.kind === 'typed') return 'Setup typed here';
	if (edited) return `Setup edited here, after opening ${whence}`;
	return origin.kind === 'seed' ? `Setup with ${whence}` : `Setup from ${whence}`;
}
