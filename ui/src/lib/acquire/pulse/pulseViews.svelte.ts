/**
 * One window's run views for a pulse-by-pulse test (classical shock, SRS shock): the layout's
 * pages, the page this window shows, and the subscription that keeps the window's plots equal to
 * what that page draws. The service sends a window's data frame once per pulse and at once on
 * subscribe, so a window that opens late has the latest pulse.
 *
 * The operator adds, removes and renames pages during a run, and opens one in a window of its own
 * for another monitor. A page that is not on screen asks for nothing. The layout is kept per setup
 * and shared between this browser's windows through the storage event: a window takes the layout
 * as storage holds it and writes only its own edits.
 */

import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { View } from '$lib/protocol/types';
import { lastLayoutKey, layoutKey } from '../display/layout';
import { toRefusal } from '../words';
import {
	defaultPulseLayout,
	pulseLayoutFromSaved,
	pulseLayoutToSaved,
	pulsePagePlots,
	pulseSubscribeParams,
	type PulseLayout,
	type PulsePageSet,
	type PulsePageSpec,
	type PulsePlotSpec
} from './pulseLayout';

export interface PulseViewsOptions {
	/** The setup's title, read reactively: the layout is kept per setup. */
	setupTitle: () => string | null;
	/** True while this window shows its pages; otherwise it asks for nothing. */
	showing: () => boolean;
	/** Where layouts are kept (shared by this browser's windows); null keeps nothing. */
	storage?: Storage | null;
	/** Where this window remembers its page; null keeps nothing. */
	session?: Storage | null;
	/** Where the other windows' storage events reach this one; null hears none. */
	events?: EventTarget | null;
	/** The page to open on (a window opened for one page). */
	page?: string | null;
	subscribeDelayMs?: number;
	pixelRatio?: number;
}

/** Where a test type's pages come from on the wire. */
export interface PulseWire<K extends string, Q extends string> {
	pages: PulsePageSet<K, Q>;
	/** The data event's topic (`shock.data`). */
	dataTopic: string;
	/** The subscribe method (`shock.subscribe`). */
	subscribe: string;
	/** Where this window remembers its page. */
	pageKey: string;
}

/** A data frame as the views read it: one entry per subscribed plot. */
export interface PulseDataFrame {
	plots: { id: string; quantity: string; views: View[] }[];
}

/** What a subscription was accepted as. */
export interface PulseSubscribed {
	default: boolean;
	rate_hz: number;
	plots: { id: string; quantity: string; points?: number }[] | null;
	quantities: string[];
}

function storageOf(kind: 'sessionStorage' | 'localStorage'): Storage | null {
	try {
		return typeof globalThis[kind] === 'undefined' ? null : globalThis[kind];
	} catch {
		return null;
	}
}

function read(storage: Storage | null, key: string): string | null {
	try {
		return storage?.getItem(key) ?? null;
	} catch {
		return null;
	}
}

function write(storage: Storage | null, key: string, value: string) {
	try {
		storage?.setItem(key, value);
	} catch {
		/* not kept; the layout still works for this window */
	}
}

export class PulseViewsState<K extends string, Q extends string, D extends PulseDataFrame = PulseDataFrame, S extends PulseSubscribed = PulseSubscribed> {
	readonly control: ServiceClient;
	readonly wire: PulseWire<K, Q>;
	private readonly opts: PulseViewsOptions;
	private readonly storage: Storage | null;
	private readonly session: Storage | null;

	/** Set from the page set in the constructor. */
	layout = $state<PulseLayout<K>>({ version: 1, rate: 1, serial: 0, pages: [] });
	pageId = $state('');
	/** Each page's drawn width, CSS pixels. */
	widths = $state<Record<string, number>>({});
	renaming = $state<string | null>(null);
	key = $state('');
	/** The last frame for this window, or null. */
	data = $state.raw<D | null>(null);
	/** What the service accepted for this window, or null before the first answer. */
	accepted = $state.raw<S | null>(null);
	/** Why the last subscription was refused, in words, or null. */
	error = $state<string | null>(null);

	private stored: string | null = null;
	private want: { key: string; params: Record<string, unknown> } | null = null;
	private sentKey = '';
	private timer: ReturnType<typeof setTimeout> | null = null;
	private inflight = false;

	constructor(control: ServiceClient, wire: PulseWire<K, Q>, opts: PulseViewsOptions) {
		this.control = control;
		this.wire = wire;
		this.opts = opts;
		this.layout = defaultPulseLayout(wire.pages);
		this.storage = opts.storage === undefined ? storageOf('localStorage') : opts.storage;
		this.session = opts.session === undefined ? storageOf('sessionStorage') : opts.session;
		this.pageId = opts.page ?? read(this.session, wire.pageKey) ?? this.layout.pages[0].id;
		control.on(wire.dataTopic, (d) => {
			this.data = d as unknown as D;
		});
	}

	// =========================================================================================
	// Reading

	get pages(): PulsePageSpec<K>[] {
		return this.layout.pages;
	}

	get page(): PulsePageSpec<K> {
		return this.layout.pages.find((p) => p.id === this.pageId) ?? this.layout.pages[0];
	}

	get canAddPage(): boolean {
		return this.layout.pages.length < this.wire.pages.maxPages;
	}

	get canRemovePage(): boolean {
		return this.layout.pages.length > 1;
	}

	/** The plots this window asks the service for now: the page on screen's, or none. */
	get wanted(): PulsePlotSpec<Q>[] {
		if (!this.opts.showing()) return [];
		const page = this.page;
		const ratio = this.opts.pixelRatio ?? (typeof devicePixelRatio === 'number' ? Math.min(2, devicePixelRatio) : 1);
		return page ? pulsePagePlots(this.wire.pages, page, this.widths[page.id] ?? 900, ratio) : [];
	}

	/** The views the last frame holds for one of this window's plots. */
	plotViews(id: string): View[] {
		return this.data?.plots.find((p) => p.id === id)?.views ?? [];
	}

	// =========================================================================================
	// Pages

	selectPage(id: string): void {
		if (!this.layout.pages.some((p) => p.id === id)) return;
		this.pageId = id;
		this.renaming = null;
		write(this.session, this.wire.pageKey, id);
	}

	addPage(kind: K, name?: string): string | null {
		if (!this.canAddPage) return null;
		const ids = this.layout.pages.map((p) => Number(p.id.replace(/^[a-z]+-/, ''))).filter(Number.isInteger);
		this.layout.serial = Math.max(this.layout.serial, ...ids) + 1;
		const id = `${this.wire.pages.prefix}-${this.layout.serial}`;
		const used = new Set(this.layout.pages.map((p) => p.name.toLowerCase()));
		let base = (name ?? '').replace(/\s+/g, ' ').trim().slice(0, 40) || this.wire.pages.words[kind];
		if (used.has(base.toLowerCase())) {
			let k = 2;
			while (used.has(`${base} ${k}`.toLowerCase())) k++;
			base = `${base} ${k}`;
		}
		this.layout.pages.push({ id, name: base, kind });
		this.selectPage(id);
		return id;
	}

	renamePage(id: string, name: string): boolean {
		const clean = name.replace(/\s+/g, ' ').trim().slice(0, 40);
		const p = this.layout.pages.find((x) => x.id === id);
		if (!p || !clean) return false;
		p.name = clean;
		return true;
	}

	removePage(id: string): boolean {
		if (!this.canRemovePage) return false;
		const i = this.layout.pages.findIndex((p) => p.id === id);
		if (i < 0) return false;
		this.layout.pages.splice(i, 1);
		if (this.pageId === id) this.selectPage(this.layout.pages[Math.max(0, i - 1)].id);
		return true;
	}

	setWidth(pageId: string, px: number): void {
		const w = Math.round(px);
		if (!(w > 0) || this.widths[pageId] === w) return;
		this.widths[pageId] = w;
	}

	setRate(rate: number): void {
		this.layout.rate = rate;
	}

	resetLayout(): void {
		this.layout = defaultPulseLayout(this.wire.pages);
		this.selectPage(this.layout.pages[0].id);
	}

	// =========================================================================================
	// Wiring

	/** Load and keep the layout, and keep the subscription equal to the page on screen. */
	watch(): () => void {
		const testType = this.wire.pages.testType;
		const onStorage = (ev: Event) => {
			if ((ev as StorageEvent).key === this.key) this.adopt();
		};
		const events = this.opts.events === undefined ? (typeof window === 'undefined' ? null : window) : this.opts.events;
		events?.addEventListener('storage', onStorage);
		const stop = $effect.root(() => {
			$effect(() => {
				const key = layoutKey(testType, this.opts.setupTitle());
				untrack(() => this.load(key));
			});
			$effect(() => {
				const text = pulseLayoutToSaved($state.snapshot(this.layout) as PulseLayout<K>);
				const key = this.key;
				untrack(() => {
					if (!key) return;
					if (text !== this.stored) {
						this.stored = text;
						write(this.storage, key, text);
					}
					write(this.storage, lastLayoutKey(testType), text);
				});
			});
			$effect(() => {
				const params = pulseSubscribeParams(this.wire.pages, $state.snapshot(this.wanted) as PulsePlotSpec<Q>[], this.layout.rate);
				const open = this.control.status === 'open';
				untrack(() => {
					if (!open) {
						this.want = null;
						this.sentKey = '';
						this.cancelTimer();
						return;
					}
					this.want = { key: JSON.stringify(params), params };
					if (this.want.key !== this.sentKey) this.schedule();
				});
			});
		});
		return () => {
			events?.removeEventListener('storage', onStorage);
			this.cancelTimer();
			stop();
			this.want = null;
			this.sentKey = '';
		};
	}

	private schedule() {
		if (this.timer) return;
		this.timer = setTimeout(() => {
			this.timer = null;
			void this.send();
		}, this.opts.subscribeDelayMs ?? 40);
	}

	private cancelTimer() {
		if (this.timer) clearTimeout(this.timer);
		this.timer = null;
	}

	private async send() {
		const want = this.want;
		if (this.inflight || !want || want.key === this.sentKey) return;
		this.inflight = true;
		this.sentKey = want.key;
		try {
			this.accepted = await this.control.request<S>(this.wire.subscribe, want.params);
			this.error = null;
			if (this.accepted.default) this.data = null;
		} catch (err) {
			this.error = toRefusal('Show the plots', err).message;
		} finally {
			this.inflight = false;
			if (this.want && this.want.key !== this.sentKey) void this.send();
		}
	}

	private load(key: string) {
		if (key === this.key) return;
		const testType = this.wire.pages.testType;
		const own = pulseLayoutFromSaved(this.wire.pages, read(this.storage, key));
		const next = own ?? pulseLayoutFromSaved(this.wire.pages, read(this.storage, lastLayoutKey(testType))) ?? defaultPulseLayout(this.wire.pages);
		this.key = key;
		this.stored = own ? pulseLayoutToSaved(own) : null;
		this.layout = next;
		if (!next.pages.some((p) => p.id === this.pageId)) this.pageId = next.pages[0].id;
	}

	private adopt() {
		const next = pulseLayoutFromSaved(this.wire.pages, read(this.storage, this.key));
		if (!next) return;
		const text = pulseLayoutToSaved(next);
		this.stored = text;
		if (text === pulseLayoutToSaved($state.snapshot(this.layout) as PulseLayout<K>)) return;
		this.layout = next;
		if (!next.pages.some((p) => p.id === this.pageId)) this.pageId = next.pages[0].id;
	}
}
