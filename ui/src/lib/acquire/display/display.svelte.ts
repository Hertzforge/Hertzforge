/**
 * One window's live display: the layout's pages and plots, the page this window shows, and the
 * data the control service sends for it.
 *
 * Every window subscribes for exactly the plots it has on screen (PROTOCOL.md, "Live Data
 * Subscriptions"): the page in view while the run view is up, and nothing but the run's numbers
 * otherwise. Adding, removing or changing a plot sends the new list; the service answers with one
 * frame at once, so a plot added during a run fills without waiting and the run itself is never
 * touched. A window on another monitor is the same thing with its own page.
 *
 * The layout is saved per test type and per setup, and shared between this browser's windows: an
 * edit in one window reaches the others through the storage event.
 */

import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { DataPlot, LiveQuantity, RandomData, RandomSubscribeResult } from '$lib/protocol/types';
import { CONTROL_TOPICS as T } from '$lib/protocol/types';
import { toRefusal } from '../words';
import {
	cleanName,
	defaultRandomLayout,
	freeName,
	fromSaved,
	lastLayoutKey,
	layoutKey,
	MAX_PAGES,
	MAX_PLOTS,
	newPlot,
	nextId,
	subscriptionFor,
	toSaved,
	type Layout,
	type Page,
	type PlotSpec
} from './layout';

export interface DisplayOptions {
	/** The test type the layout belongs to. */
	testType: string;
	/** The setup's title, read reactively: the layout is kept per setup. */
	setupTitle: () => string | null;
	/** True while this window shows its plots; otherwise it asks only for the run's numbers. */
	showing: () => boolean;
	/** Where layouts are kept (shared by this browser's windows); null keeps nothing. */
	storage?: Storage | null;
	/** Where this window remembers its page; null keeps nothing. */
	session?: Storage | null;
	/** The page to open on (a window opened for one page). */
	page?: string | null;
	/** Debounce of the subscription, ms. */
	subscribeDelayMs?: number;
}

const PAGE_KEY = 'hertzforge.display.page';

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
		/* storage refused; the layout just does not persist */
	}
}

export class DisplayState {
	readonly control: ServiceClient;
	private readonly opts: DisplayOptions;
	private readonly storage: Storage | null;
	private readonly session: Storage | null;

	layout = $state<Layout>(defaultRandomLayout());
	/** The page this window shows. */
	pageId = $state('pg-1');
	/** The last frame for this window, or null. */
	data = $state.raw<RandomData | null>(null);
	/** What the service accepted for this window, or null before the first answer. */
	accepted = $state.raw<RandomSubscribeResult | null>(null);
	/** Why the last subscription was refused, in words, or null. */
	error = $state<string | null>(null);
	/** The plot whose settings are open, or null. */
	editing = $state<string | null>(null);
	/** The page being renamed, or null. */
	renaming = $state<string | null>(null);
	/** Where the layout on screen is kept. */
	key = $state('');

	/**
	 * The subscription last asked for, sent or waiting out the debounce. A change is compared with
	 * this, not with what was sent, so going away and straight back within the debounce still ends
	 * on the latest. Cleared when the connection closes, as the subscription goes with it.
	 */
	private wantKey = '';
	private seq = 0;
	private timer: ReturnType<typeof setTimeout> | null = null;

	constructor(control: ServiceClient, opts: DisplayOptions) {
		this.control = control;
		this.opts = opts;
		this.storage = opts.storage === undefined ? storageOf('localStorage') : opts.storage;
		this.session = opts.session === undefined ? storageOf('sessionStorage') : opts.session;
		this.pageId = opts.page ?? read(this.session, PAGE_KEY) ?? 'pg-1';
		control.on(T.data, (d) => this.onData(d as unknown as RandomData));
	}

	// =========================================================================================
	// Reading

	get pages(): Page[] {
		return this.layout.pages;
	}

	get page(): Page {
		return this.layout.pages.find((p) => p.id === this.pageId) ?? this.layout.pages[0];
	}

	get canAddPage(): boolean {
		return this.layout.pages.length < MAX_PAGES;
	}

	get canRemovePage(): boolean {
		return this.layout.pages.length > 1;
	}

	get canAddPlot(): boolean {
		return this.page.plots.length < MAX_PLOTS;
	}

	/** The quantities the service offers (all of them until it has said). */
	get quantities(): LiveQuantity[] {
		return this.accepted?.quantities ?? ['control', 'drive', 'response', 'time', 'coherence', 'readings'];
	}

	/** The plots this window asks the service for now. */
	get wanted(): PlotSpec[] {
		return this.opts.showing() ? this.page.plots : [];
	}

	/** This plot's data in the last frame, or null. */
	plotData(id: string): DataPlot | null {
		return this.data?.plots.find((p) => p.id === id) ?? null;
	}

	plot(id: string): PlotSpec | null {
		for (const p of this.layout.pages) {
			const x = p.plots.find((y) => y.id === id);
			if (x) return x;
		}
		return null;
	}

	// =========================================================================================
	// Pages

	selectPage(id: string): void {
		if (!this.layout.pages.some((p) => p.id === id)) return;
		this.pageId = id;
		this.editing = null;
		write(this.session, PAGE_KEY, id);
	}

	addPage(name?: string): string | null {
		if (!this.canAddPage) return null;
		const id = nextId(this.layout, 'pg');
		const clean = (name && cleanName(name)) || freeName(this.layout);
		this.layout.pages.push({ id, name: clean, plots: [] });
		this.selectPage(id);
		return id;
	}

	/** Rename a page; an empty name is refused (false). */
	renamePage(id: string, name: string): boolean {
		const clean = cleanName(name);
		const p = this.layout.pages.find((x) => x.id === id);
		if (!p || !clean) return false;
		p.name = clean;
		return true;
	}

	/** Remove a page; the last page stays. */
	removePage(id: string): boolean {
		if (!this.canRemovePage) return false;
		const i = this.layout.pages.findIndex((p) => p.id === id);
		if (i < 0) return false;
		this.layout.pages.splice(i, 1);
		if (this.pageId === id) this.selectPage(this.layout.pages[Math.max(0, i - 1)].id);
		return true;
	}

	// =========================================================================================
	// Plots

	addPlot(quantity: LiveQuantity = 'response'): string | null {
		if (!this.canAddPlot) return null;
		const id = nextId(this.layout, 'pl');
		this.page.plots.push(newPlot(id, quantity));
		this.editing = id;
		return id;
	}

	updatePlot(id: string, patch: Partial<Omit<PlotSpec, 'id'>>): void {
		const p = this.plot(id);
		if (!p) return;
		Object.assign(p, patch);
	}

	removePlot(id: string): void {
		for (const page of this.layout.pages) {
			const i = page.plots.findIndex((p) => p.id === id);
			if (i >= 0) page.plots.splice(i, 1);
		}
		if (this.editing === id) this.editing = null;
	}

	/** Move a plot one place earlier (-1) or later (+1) on its page. */
	movePlot(id: string, by: -1 | 1): void {
		const plots = this.page.plots;
		const i = plots.findIndex((p) => p.id === id);
		const j = i + by;
		if (i < 0 || j < 0 || j >= plots.length) return;
		[plots[i], plots[j]] = [plots[j], plots[i]];
	}

	setRate(rate: number): void {
		this.layout.rate = rate;
	}

	/** Back to the test type's own pages. */
	resetLayout(): void {
		this.layout = defaultRandomLayout();
		this.selectPage(this.layout.pages[0].id);
	}

	// =========================================================================================
	// Wiring

	/** Load the layout for the setup on screen, keep it saved, and keep the subscription current. */
	watch(): () => void {
		const onStorage = (e: StorageEvent) => {
			if (e.key !== this.key || e.newValue === null) return;
			const next = fromSaved(e.newValue);
			if (!next || toSaved(next) === toSaved($state.snapshot(this.layout) as Layout)) return;
			this.layout = next;
			if (!next.pages.some((p) => p.id === this.pageId)) this.pageId = next.pages[0].id;
		};
		if (typeof window !== 'undefined') window.addEventListener('storage', onStorage);
		const stop = $effect.root(() => {
			// The layout follows the setup: its own, else the test type's last, else the defaults.
			$effect(() => {
				const key = layoutKey(this.opts.testType, this.opts.setupTitle());
				untrack(() => this.load(key));
			});
			// Every change is kept, for this setup and as the test type's last.
			$effect(() => {
				const text = toSaved($state.snapshot(this.layout) as Layout);
				const key = this.key;
				untrack(() => {
					if (!key) return;
					if (read(this.storage, key) !== text) write(this.storage, key, text);
					write(this.storage, lastLayoutKey(this.opts.testType), text);
				});
			});
			// The subscription follows the plots on screen, on every connection: a subscription belongs
			// to its connection, so a new one is asked again.
			$effect(() => {
				const plots = subscriptionFor($state.snapshot(this.wanted) as PlotSpec[]);
				const rate = this.layout.rate;
				const open = this.control.status === 'open';
				untrack(() => {
					if (!open) {
						this.wantKey = '';
						if (this.timer) clearTimeout(this.timer);
						return;
					}
					const key = JSON.stringify([rate, plots]);
					if (key === this.wantKey) return;
					this.wantKey = key;
					this.schedule(rate, plots);
				});
			});
		});
		return () => {
			if (typeof window !== 'undefined') window.removeEventListener('storage', onStorage);
			if (this.timer) clearTimeout(this.timer);
			stop();
		};
	}

	private load(key: string) {
		if (key === this.key) return;
		const own = fromSaved(read(this.storage, key));
		const next = own ?? fromSaved(read(this.storage, lastLayoutKey(this.opts.testType))) ?? defaultRandomLayout();
		this.key = key;
		this.layout = next;
		if (!next.pages.some((p) => p.id === this.pageId)) this.pageId = next.pages[0].id;
		this.editing = null;
	}

	/** The latest change wins: a change within the debounce replaces the one waiting. */
	private schedule(rate: number, plots: ReturnType<typeof subscriptionFor>) {
		if (this.timer) clearTimeout(this.timer);
		this.timer = setTimeout(() => void this.send(rate, plots), this.opts.subscribeDelayMs ?? 40);
	}

	/**
	 * One subscription in flight at a time: the service may take two requests in either order, so
	 * a change made while one is on its way is sent when it has been answered (the latest only).
	 */
	private inflight = false;
	private queued: { rate: number; plots: ReturnType<typeof subscriptionFor> } | null = null;

	private async send(rate: number, plots: ReturnType<typeof subscriptionFor>) {
		if (this.inflight) {
			this.queued = { rate, plots };
			return;
		}
		this.inflight = true;
		const seq = ++this.seq;
		try {
			const res = await this.control.request<RandomSubscribeResult>('random.subscribe', { rate_hz: rate, plots });
			if (seq === this.seq) {
				this.accepted = res;
				this.error = null;
			}
		} catch (err) {
			// Asked again on the next change or the next connection.
			this.wantKey = '';
			this.error = toRefusal('Show the plots', err).message;
		} finally {
			this.inflight = false;
			const next = this.queued;
			this.queued = null;
			if (next) void this.send(next.rate, next.plots);
		}
	}

	private onData(d: RandomData) {
		this.data = d;
	}
}
