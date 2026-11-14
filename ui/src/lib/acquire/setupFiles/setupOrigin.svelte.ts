/**
 * Where each test screen's draft came from: typed here, a saved setup by its name, a run by its id,
 * or a seed. The origin holds until the operator edits the draft, after which it reads as edited.
 * It is kept with the draft in the browser's storage, as a cache of the draft only; the setup
 * object in the project is what is saved.
 *
 * The origin is found from the test's store, the object the screen's run verbs are given, so the
 * Arm verb can say where the setup it arms came from.
 */

import { originWords, type SetupOrigin } from './setupFile';

function readStore(storage: Storage | null, key: string): string | null {
	try {
		return storage?.getItem(key) ?? null;
	} catch {
		return null;
	}
}

function writeStore(storage: Storage | null, key: string, value: string) {
	try {
		storage?.setItem(key, value);
	} catch {
		/* storage refused; the origin just does not persist */
	}
}

export class SetupOriginState {
	origin = $state.raw<SetupOrigin>({ kind: 'typed' });
	/** The draft's setup, as a key, when it was opened; null for a typed setup. */
	openedKey = $state<string | null>(null);

	constructor(
		/** The draft's setup as it stands, as a key (canonical JSON). */
		private readonly currentKey: () => string,
		private readonly storage: Storage | null = null,
		private readonly storageKey = ''
	) {
		const raw = this.storageKey ? readStore(this.storage, this.storageKey) : null;
		if (!raw) return;
		try {
			const saved = JSON.parse(raw) as { origin?: SetupOrigin; openedKey?: string | null };
			if (saved.origin && typeof saved.origin.kind === 'string') {
				this.origin = saved.origin;
				this.openedKey = typeof saved.openedKey === 'string' ? saved.openedKey : null;
			}
		} catch {
			/* a cache that does not read is no cache */
		}
	}

	/** The draft now holds a setup from `origin`, as `key` describes it. */
	set(origin: SetupOrigin, key: string | null): void {
		this.origin = origin;
		this.openedKey = origin.kind === 'typed' ? null : key;
		if (this.storageKey) writeStore(this.storage, this.storageKey, JSON.stringify({ origin: this.origin, openedKey: this.openedKey }));
	}

	/** The operator changed the draft since it was opened. */
	get edited(): boolean {
		return this.origin.kind !== 'typed' && this.openedKey !== null && this.currentKey() !== this.openedKey;
	}

	/** The origin as a saved setup records it: an edited draft is the operator's own. */
	get recorded(): SetupOrigin {
		return this.edited ? { kind: 'typed' } : this.origin;
	}

	get words(): string {
		return originWords(this.origin, this.edited);
	}
}

const origins = new WeakMap<object, SetupOriginState>();

/** The origin of the draft of the test whose store is `store`, made the first time it is asked. */
export function originFor(store: object, make: () => SetupOriginState): SetupOriginState {
	let found = origins.get(store);
	if (!found) {
		found = make();
		origins.set(store, found);
	}
	return found;
}

/** The origin kept for `store`, or null when no setup bar has shown it. */
export function knownOrigin(store: object): SetupOriginState | null {
	return origins.get(store) ?? null;
}
