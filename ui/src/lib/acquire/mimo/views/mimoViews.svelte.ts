/**
 * One window's MIMO run views: the layout's pages, the page this window shows, the run's status,
 * and the feed that keeps this window's subscription equal to what the page on screen draws.
 *
 * The pages are the four views (matrix grid, error heatmap, drive and plant, geometry); the
 * operator adds, removes and renames them during a run, and opens one in a window of its own for
 * another monitor. Each window asks the service for its own page only (`MimoFeed`), so a page
 * that is removed or switched away from stops costing anything at once. The layout is saved per
 * setup, and shared between this browser's windows through the storage event: a window takes the
 * layout as storage holds it and writes only its own edits, so windows never echo each other.
 */

import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import { MIMO_TOPICS, type MimoDataPlot, type MimoQuantity, type MimoStatus } from '$lib/protocol/mimo';
import { lastLayoutKey, layoutKey } from '$lib/acquire/display/layout';
import { MimoFeed } from '$lib/acquire/display/mimoFeed.svelte';
import {
	defaultMimoLayout,
	MAX_MIMO_PAGES,
	MIMO_PAGE_WORDS,
	mimoLayoutFromSaved,
	mimoLayoutToSaved,
	type MimoLayout,
	type MimoPageKind,
	type MimoPageSpec,
	type MimoPlotSpec
} from '$lib/acquire/display/mimoLayout';
import { revealControl } from '$lib/viz/matrixGrid';
import { gridSettings, pagePlots, type PageContext } from './pages';

export const MIMO_TEST_TYPE = 'mimo_random';
const PAGE_KEY = 'hertzforge.display.mimo.page';

export interface MimoViewsOptions {
	/** The setup's title, read reactively: the layout is kept per setup. */
	setupTitle: () => string | null;
	/** True while this window shows its pages; otherwise it asks for nothing. */
	showing: () => boolean;
	/** How many monitors the defaults are laid out for (1 to 4). */
	monitors?: number;
	/** Where layouts are kept (shared by this browser's windows); null keeps nothing. */
	storage?: Storage | null;
	/** Where this window remembers its page; null keeps nothing. */
	session?: Storage | null;
	/** Where the other windows' storage events reach this one; null hears none. */
	events?: EventTarget | null;
	/** The page to open on (a window opened for one page). */
	page?: string | null;
	/** Which window this is, for the default page it opens on: 0 the main window. */
	windowIndex?: number;
	subscribeDelayMs?: number;
	pixelRatio?: number;
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

export class MimoViewsState {
	readonly control: ServiceClient;
	readonly feed: MimoFeed;
	private readonly opts: MimoViewsOptions;
	private readonly storage: Storage | null;
	private readonly session: Storage | null;

	layout = $state<MimoLayout>(defaultMimoLayout().layout);
	pageId = $state('mp-1');
	/** The run's last status, or null before one has come. */
	status = $state.raw<MimoStatus | null>(null);
	/** Each page's drawn widths, by part, in CSS pixels. */
	widths = $state<Record<string, Record<string, number>>>({});
	renaming = $state<string | null>(null);
	key = $state('');

	private stored: string | null = null;

	constructor(control: ServiceClient, opts: MimoViewsOptions) {
		this.control = control;
		this.opts = opts;
		this.storage = opts.storage === undefined ? storageOf('localStorage') : opts.storage;
		this.session = opts.session === undefined ? storageOf('sessionStorage') : opts.session;
		const defaults = defaultMimoLayout(opts.monitors ?? 1);
		this.layout = defaults.layout;
		this.pageId = opts.page ?? read(this.session, PAGE_KEY) ?? defaults.windows[Math.min(opts.windowIndex ?? 0, defaults.windows.length - 1)];
		this.feed = new MimoFeed(control, {
			plots: () => this.wanted,
			rate: () => this.layout.rate,
			subscribeDelayMs: opts.subscribeDelayMs
		});
		control.on(MIMO_TOPICS.status, (d) => {
			this.status = d as unknown as MimoStatus;
		});
	}

	// =========================================================================================
	// Reading

	get pages(): MimoPageSpec[] {
		return this.layout.pages;
	}

	get page(): MimoPageSpec {
		return this.layout.pages.find((p) => p.id === this.pageId) ?? this.layout.pages[0];
	}

	get canAddPage(): boolean {
		return this.layout.pages.length < MAX_MIMO_PAGES;
	}

	get canRemovePage(): boolean {
		return this.layout.pages.length > 1;
	}

	get labels(): string[] {
		return this.status?.shape?.labels ?? [];
	}

	get driveLabels(): string[] {
		return this.status?.shape?.drive_labels ?? [];
	}

	get quantities(): MimoQuantity[] | null {
		return this.feed.accepted?.quantities ?? null;
	}

	get error(): string | null {
		return this.feed.error;
	}

	/** What a page's plots depend on besides its settings. */
	context(pageId: string): PageContext {
		return {
			labels: this.labels,
			drive_labels: this.driveLabels,
			space: this.status?.shape?.space ?? null,
			quantities: this.quantities,
			widths: this.widths[pageId] ?? {},
			pixelRatio: this.opts.pixelRatio ?? (typeof devicePixelRatio === 'number' ? Math.min(2, devicePixelRatio) : 1)
		};
	}

	/** The plots this window asks the service for now: the page on screen's, or none. */
	get wanted(): MimoPlotSpec[] {
		if (!this.opts.showing()) return [];
		const page = this.page;
		return page ? pagePlots(page, this.context(page.id)) : [];
	}

	plotData(id: string): MimoDataPlot | null {
		return this.feed.plotData(id);
	}

	// =========================================================================================
	// Pages

	selectPage(id: string): void {
		if (!this.layout.pages.some((p) => p.id === id)) return;
		this.pageId = id;
		this.renaming = null;
		write(this.session, PAGE_KEY, id);
	}

	/** A new page of `kind`, named for it, shown at once; null when the layout is full. */
	addPage(kind: MimoPageKind, name?: string): string | null {
		if (!this.canAddPage) return null;
		const ids = this.layout.pages.map((p) => Number(p.id.replace(/^[a-z]+-/, ''))).filter(Number.isInteger);
		this.layout.serial = Math.max(this.layout.serial, ...ids) + 1;
		const id = `mp-${this.layout.serial}`;
		const used = new Set(this.layout.pages.map((p) => p.name.toLowerCase()));
		let base = (name ?? '').replace(/\s+/g, ' ').trim().slice(0, 40) || MIMO_PAGE_WORDS[kind];
		if (used.has(base.toLowerCase())) {
			let k = 2;
			while (used.has(`${base} ${k}`.toLowerCase())) k++;
			base = `${base} ${k}`;
		}
		this.layout.pages.push({ id, name: base, kind, settings: {} });
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
		const { [id]: _gone, ...rest } = this.widths;
		void _gone;
		this.widths = rest;
		if (this.pageId === id) this.selectPage(this.layout.pages[Math.max(0, i - 1)].id);
		return true;
	}

	/** Change a page's own choices (merged into its settings). */
	patch(pageId: string, patch: Record<string, unknown>): void {
		const p = this.layout.pages.find((x) => x.id === pageId);
		if (!p) return;
		p.settings = { ...p.settings, ...patch };
	}

	/** A page's part was drawn this wide (CSS pixels); its plots ask for points to match. */
	setWidth(pageId: string, part: string, px: number): void {
		const w = Math.round(px);
		if (!(w > 0) || this.widths[pageId]?.[part] === w) return;
		this.widths[pageId] = { ...(this.widths[pageId] ?? {}), [part]: w };
	}

	setRate(rate: number): void {
		this.layout.rate = rate;
	}

	resetLayout(): void {
		const d = defaultMimoLayout(this.opts.monitors ?? 1);
		this.layout = d.layout;
		this.selectPage(d.windows[0]);
	}

	/** Bring a control into a grid page's window and show that page (from the heatmap). */
	openInGrid(position: number): void {
		const grid = this.layout.pages.find((p) => p.kind === 'grid') ?? null;
		const id = grid?.id ?? this.addPage('grid');
		if (!id) return;
		const page = this.layout.pages.find((p) => p.id === id)!;
		this.patch(id, { window: revealControl(this.labels, gridSettings(page).window, position) });
		this.selectPage(id);
	}

	// =========================================================================================
	// Wiring

	/** Load and keep the layout, keep the status current, and keep the subscription current. */
	watch(): () => void {
		const onStorage = (ev: Event) => {
			if ((ev as StorageEvent).key === this.key) this.adopt();
		};
		const events = this.opts.events === undefined ? (typeof window === 'undefined' ? null : window) : this.opts.events;
		events?.addEventListener('storage', onStorage);
		const stopFeed = this.feed.watch();
		const stop = $effect.root(() => {
			$effect(() => {
				const key = layoutKey(MIMO_TEST_TYPE, this.opts.setupTitle());
				untrack(() => this.load(key));
			});
			$effect(() => {
				const text = mimoLayoutToSaved($state.snapshot(this.layout) as MimoLayout);
				const key = this.key;
				untrack(() => {
					if (!key) return;
					if (text !== this.stored) {
						this.stored = text;
						write(this.storage, key, text);
					}
					write(this.storage, lastLayoutKey(MIMO_TEST_TYPE), text);
				});
			});
			// The run's status on every new connection; events keep it current after that.
			$effect(() => {
				if (this.control.status !== 'open') return;
				untrack(() => {
					this.control
						.request<MimoStatus>('mimo_random.status', {})
						.then((s) => (this.status = s))
						.catch(() => {
							/* a service without MIMO Random: the views wait */
						});
				});
			});
		});
		return () => {
			events?.removeEventListener('storage', onStorage);
			stopFeed();
			stop();
		};
	}

	private load(key: string) {
		if (key === this.key) return;
		const own = mimoLayoutFromSaved(read(this.storage, key));
		const next = own ?? mimoLayoutFromSaved(read(this.storage, lastLayoutKey(MIMO_TEST_TYPE))) ?? defaultMimoLayout(this.opts.monitors ?? 1).layout;
		this.key = key;
		this.stored = own ? mimoLayoutToSaved(own) : null;
		this.layout = next;
		if (!next.pages.some((p) => p.id === this.pageId)) this.pageId = next.pages[0].id;
	}

	private adopt() {
		const next = mimoLayoutFromSaved(read(this.storage, this.key));
		if (!next) return;
		const text = mimoLayoutToSaved(next);
		this.stored = text;
		if (text === mimoLayoutToSaved($state.snapshot(this.layout) as MimoLayout)) return;
		this.layout = next;
		if (!next.pages.some((p) => p.id === this.pageId)) this.pageId = next.pages[0].id;
	}
}
