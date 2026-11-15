/**
 * What the shared setup bar needs of each test's store, one adapter per module over the module's
 * own conversions: the configuration the draft would send (exactly what its configure request
 * carries), the draft filled from a configuration, the same without filling (to name what the
 * draft cannot hold), and the service's words on the draft. Filling a draft is the module's own
 * "start the draft from a configuration" (what a reloaded screen does with the service's setup);
 * it changes the draft only, and the store then checks it with its dry run as it checks an edit.
 */

import type { AppState } from '$lib/app/state.svelte';
import type { ChannelRow } from '$lib/protocol/types';
import type { MimoSetup } from '$lib/protocol/mimo';
import type { AcquireState } from '../store.svelte';
import { fromConfig, toConfig } from '../setup';
import { fromSineConfig, toSineConfig } from '../sine/sineSetup';
import { fromMixedConfig, toMixedConfig } from '../mixed/mixedSetup';
import { fromShockConfig, toShockConfig } from '../shock/shockSetup';
import { fromSrsConfig, toSrsConfig } from '../srs/srsSetup';
import { fromTwrConfig, toTwrConfig } from '../twr/twrSetup';
import { fromExplorerConfig, toExplorerConfig } from '../explorer/explorerSetup';
import { fromRecorderConfig, toRecorderConfig } from '../recorder/recorderSetup';
import { fitChannels, fromMsConfig, toMsConfig } from '../mimoshock/msSetup';
import { toWireSetup } from '../mimo/mimoSetup';
import { fromMimoSetup } from '../mimo/mimoOpen';
import type { SetupModule, SetupTable } from './setupFile';

type Config = Record<string, unknown>;

export interface SetupHost {
	module: SetupModule;
	/** The test's store: what the screen's run verbs are given, so the origin is found from it. */
	store: object;
	/** The configuration the draft describes, as its configure request carries it; null while it cannot make one. */
	config(): Config | null;
	/** The draft's configuration as a key. */
	key(): string;
	/** The draft's title, the name a saved setup is offered. */
	title(): string;
	/** What the draft would send after taking `config`, without taking it. */
	roundTrip(config: Config): Config | null;
	/** The draft takes `config`. Nothing is sent, applied or armed. */
	fill(config: Config): void;
	/** The service's refusals of the draft as it stands, in its words. */
	findings(): string[];
	/** The service is checking the draft. */
	checking(): boolean;
	/** Why the draft cannot be filled now, or null. */
	locked(): string | null;
	/** The channel table the control service holds, or null when it has not said. */
	rows(): ChannelRow[] | null;
	/** The channel table to save with the setup: the one the service holds. */
	table(): SetupTable;
	/** The protocol the control service speaks, or null while it is not connected. */
	protocol(): string | null;
	/** What a seed is read with on this screen (the TWR control channel's unit). */
	seedParams?(): Record<string, unknown>;
	/**
	 * Store what a seed needs on the control service (a TWR reference), as the screen stores a file
	 * the operator picks; the value to put at the import's `set`, or null when it was not stored.
	 */
	importSeed?(imp: { method: string; params: Record<string, unknown> }): Promise<unknown>;
}

/** What every test store offers the bar beyond its draft. */
interface Station {
	acq: AcquireState;
	protocol: () => string | null;
}

function stationParts(s: Station, live: () => boolean): Pick<SetupHost, 'locked' | 'rows' | 'table' | 'protocol'> {
	return {
		locked: () => (live() ? 'The run is live; the setup is locked until it ends.' : !s.acq.isController ? 'Take control first: an observer does not edit the setup.' : null),
		rows: () => s.acq.server?.channels ?? null,
		table: () => (s.acq.server ? { rows: structuredClone($snapshot(s.acq.server.channels)), revision: s.acq.server.revision } : {}),
		protocol: s.protocol
	};
}

/** A plain copy of possibly reactive state. */
function $snapshot<T>(v: T): T {
	return JSON.parse(JSON.stringify(v)) as T;
}

const messages = (items: { message: string }[] | undefined | null): string[] => (items ?? []).map((i) => i.message);

/** The stores that keep their draft as `setup`, its configuration as `wire`, and the dry run as `check`. */
interface WireStore {
	setup: unknown;
	readonly wire: unknown;
	readonly key: string;
	check: { errors: { message: string }[] } | null;
	checkedKey: string;
	checking: boolean;
	readonly locked: boolean;
	readonly acq: AcquireState;
}

function wireHost<D>(module: SetupModule, store: WireStore, protocol: () => string | null, from: (c: Config) => D, to: (d: D) => unknown, fill?: (d: D) => void): SetupHost {
	const own = stationParts({ acq: store.acq, protocol }, () => store.locked);
	return {
		module,
		store,
		config: () => $snapshot(store.wire) as Config,
		key: () => store.key,
		title: () => String((store.wire as Config | null)?.title ?? ''),
		roundTrip: (c) => {
			const d = from($snapshot(c));
			fill?.(d);
			return $snapshot(to(d)) as Config;
		},
		fill: (c) => {
			const d = from($snapshot(c));
			fill?.(d);
			store.setup = d;
		},
		findings: () => (store.check && store.checkedKey === store.key ? messages(store.check.errors) : []),
		checking: () => store.checking,
		...own
	};
}

function randomHost(acq: AcquireState, protocol: () => string | null): SetupHost {
	return {
		module: 'random',
		store: acq,
		config: () => $snapshot(acq.config) as unknown as Config,
		key: () => acq.setupKey,
		title: () => acq.config.title ?? '',
		roundTrip: (c) => $snapshot(toConfig(fromConfig($snapshot(c) as never))) as unknown as Config,
		fill: (c) => {
			acq.setup = fromConfig($snapshot(c) as never);
		},
		findings: () => messages(acq.setupCheck?.errors),
		checking: () => acq.checking,
		...stationParts({ acq, protocol }, () => acq.locked)
	};
}

/** The adapter for `module` over the shell's own stores. */
export function hostFor(app: AppState, module: SetupModule): SetupHost {
	const protocol = () => (app.control.isOpen ? (app.control.hello?.protocol ?? null) : null);
	switch (module) {
		case 'random':
			return randomHost(app.acquire, protocol);
		case 'sine': {
			const s = app.sine;
			return wireHost(module, s, protocol, (c) => fromSineConfig(c as never), (d) => toSineConfig(d, s.sineLaws.find((l) => l.id === d.law.id) ?? null, s.acq.rows));
		}
		case 'mixed': {
			const s = app.mixed;
			return wireHost(module, s, protocol, (c) => fromMixedConfig(c as never), (d) =>
				toMixedConfig(d, { floor: s.floorLaws.find((l) => l.id === d.lawFloor.id) ?? null, tones: s.toneLaws.find((l) => l.id === d.lawTones.id) ?? null }, s.acq.rows)
			);
		}
		case 'classical_shock': {
			const s = app.shock;
			return wireHost(module, s, protocol, (c) => fromShockConfig(c as never), (d) => toShockConfig(d, s.classicalLaws.find((l) => l.id === d.law.id) ?? null));
		}
		case 'srs': {
			const s = app.srs;
			return wireHost(module, s, protocol, (c) => fromSrsConfig(c as never), (d) => toSrsConfig(d, s.srsLaws.find((l) => l.id === d.law.id) ?? null));
		}
		case 'twr': {
			const s = app.twr;
			return {
				...wireHost(module, s, protocol, (c) => fromTwrConfig(c as never, s.setup.facts), (d) => toTwrConfig(d, s.twrLaws.find((l) => l.id === d.law.id) ?? null)),
				// a seed's record is read in the control channel's unit, and stored as a pasted reference
				seedParams: () => ({ units: s.unit }),
				importSeed: async (imp) => {
					if (imp.method !== 'twr.reference.import') return null;
					const p = imp.params as { text: string; fs_hz?: number; units?: string; name?: string };
					return (await s.importReference({ text: p.text }, { fs_hz: p.fs_hz, units: p.units, name: p.name })) ? { sha256: s.setup.reference?.sha256 ?? '' } : null;
				}
			};
		}
		case 'explorer': {
			const s = app.explorer;
			return wireHost(module, s, protocol, (c) => fromExplorerConfig(c as never, s.setup.steps), (d) => toExplorerConfig(d));
		}
		case 'recorder': {
			const s = app.recorder;
			return wireHost(module, s, protocol, (c) => fromRecorderConfig(c as never), (d) => toRecorderConfig(d));
		}
		case 'mimo_shock': {
			const s = app.mimoShock;
			return wireHost(
				module,
				s,
				protocol,
				(c) => fromMsConfig(c as never),
				(d) => toMsConfig(d, (s.laws ?? []).find((l) => l.id === d.law.id) ?? null),
				// as the screen takes the service's setup: the draft fitted to the table's channels
				(d) => void fitChannels(d, s.acq.rows)
			);
		}
		case 'mimo_random':
			return mimoHost(app, protocol);
	}
}

function mimoHost(app: AppState, protocol: () => string | null): SetupHost {
	const m = app.mimo;
	const send = (d: ReturnType<typeof fromMimoSetup>) => {
		const law = m.laws?.find((l) => l.id === d.draft.law.id) ?? null;
		// a reference opened with the setup brings its own space, as the operator's matrices
		const space = { kind: d.draft.space.kind, dofs: d.draft.space.dofs, response: d.draft.space.kind === 'virtual' ? d.draft.space.response : null, drive: d.draft.space.kind === 'virtual' && d.draft.space.driveOn ? d.draft.space.drive : null, from: 'operator' as const };
		return toWireSetup(d.draft, { rows: m.rows, space, reference: d.reference, law, backend: m.stationKind });
	};
	return {
		module: 'mimo_random',
		store: m,
		config: () => (m.wire ? ($snapshot(m.wire) as unknown as Config) : null),
		key: () => m.key,
		title: () => m.setup.title,
		roundTrip: (c) => {
			const out = send(fromMimoSetup($snapshot(c) as unknown as MimoSetup));
			return out ? ($snapshot(out) as unknown as Config) : null;
		},
		fill: (c) => {
			const { draft, reference } = fromMimoSetup($snapshot(c) as unknown as MimoSetup);
			m.openSetup(draft, reference);
		},
		findings: () => (m.check && m.checkedKey === m.key ? messages(m.check.errors) : []),
		checking: () => m.checking,
		...stationParts({ acq: m.acq, protocol }, () => m.locked)
	};
}
