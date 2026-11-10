/**
 * One window's test screen: the layout's pages and their panels, the page this window shows, the
 * panel that fills it and the one whose options are open, the control dock, and the data the
 * control service sends for the plots on screen. Any test type uses it with its own hooks: the
 * plot types it offers, its cards, its first layout and how it asks for its plots.
 *
 * A window asks for exactly the plots on the page it shows, at its own rate, and nothing while it
 * does not show them (PROTOCOL.md, "Live Data Subscriptions"). Adding, removing or changing a plot
 * sends the new list; the service answers with a frame at once, and the run is never touched.
 *
 * The layout is kept per test type and per setup, and shared between this browser's windows: an
 * edit in one window reaches the others through the storage event. Each window takes the layout as
 * storage holds it and writes only its own edits, so the windows agree on one layout and never
 * echo each other; each still shows its own page.
 */

import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { ChannelSelector, View } from '$lib/protocol/types';
import { newPlotSpec, type PlotPanelSpec, type PlotType } from '../plots/types';
import { toRefusal } from '../words';
import * as grid from './grid';
import {
	cleanName,
	DEFAULT_ASPECT,
	DOCK_SIZE,
	freePageName,
	fromSaved,
	GRID_SIZES,
	lastScreenKey,
	MAX_PAGES,
	MAX_PANELS,
	MAX_PLOTS,
	nextId,
	plotTitle,
	screenKey,
	toSaved,
	type Offer
} from './layout';
import type { CardKind, Dock, Panel, ScreenLayout, ScreenPage } from './types';

export type PlotPanel = Extract<Panel, { kind: 'plot' }>;
export type CardPanel = Extract<Panel, { kind: 'card' }>;

/** One plot's data in a frame: its views, and the channels the service resolved for it. */
export interface FramePlot {
	id: string;
	channels?: number[] | null;
	views: View[];
}

/** A frame of data for one window: its plots by id. */
export interface ScreenFrame {
	plots: FramePlot[];
}

/** How a test type asks the control service for the plots a window shows. */
export interface ScreenFeed {
	/** The request (`random.subscribe`); it belongs to the connection, so each new one asks again. */
	method: string;
	/** The event carrying this window's frames. */
	topic: string;
	/** The request's parameters for these plots at this rate. */
	params(plots: PlotPanel[], rate: number): Record<string, unknown>;
}

/** What a test type gives its screen. */
export interface ScreenHooks {
	testType: string;
	/** Its plot types, in the order `+ Plot` lists them. */
	types: PlotType[];
	/** Its cards, in the dock's first order. */
	cards: CardKind[];
	/** Its pages until the operator changes them, made for a page area `aspect` wide. */
	defaultLayout(aspect: number): ScreenLayout;
	/** Where its layouts were kept in an earlier form, and how one is taken over. */
	legacy?: { key(setupTitle: string | null): string; last: string; read(text: string | null): ScreenLayout | null };
	feed?: ScreenFeed;
}

export interface ScreenOptions {
	/** The setup's title, read reactively: the layout is kept per setup. */
	setupTitle: () => string | null;
	/** True while this window shows its plots; otherwise it asks for none. */
	showing: () => boolean;
	/** Where layouts are kept (shared by this browser's windows); null keeps nothing. */
	storage?: Storage | null;
	/** Where this window remembers its page; null keeps nothing. */
	session?: Storage | null;
	/** Where the other windows' storage events reach this one (the window); null hears none. */
	events?: EventTarget | null;
	/** The page to open on (a window opened for one page). */
	page?: string | null;
	/** Debounce of the subscription, ms. */
	subscribeDelayMs?: number;
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
		/* storage refused; the layout just does not persist */
	}
}

export class ScreenState<D extends ScreenFrame = ScreenFrame, A = unknown> {
	readonly control: ServiceClient;
	readonly hooks: ScreenHooks;
	private readonly opts: ScreenOptions;
	private readonly storage: Storage | null;
	private readonly session: Storage | null;
	private readonly pageKey: string;

	layout = $state<ScreenLayout>() as ScreenLayout;
	/** The page this window shows. */
	pageId = $state('pg-1');
	/** The panel filling this window's page, or null. */
	maximized = $state<string | null>(null);
	/** The panel whose options are open, or null. */
	options = $state<string | null>(null);
	/** The page being renamed, or null. */
	renaming = $state<string | null>(null);
	/** The page area's width over its height, as last measured. */
	aspect = $state(DEFAULT_ASPECT);
	/** The last frame for this window, or null. */
	data = $state.raw<D | null>(null);
	/** What the service accepted for this window, or null before the first answer. */
	accepted = $state.raw<A | null>(null);
	/** Why the last subscription was refused, in words, or null. */
	error = $state<string | null>(null);
	/** Where the layout on screen is kept. */
	key = $state('');

	/**
	 * The layout as this window last found it in storage or wrote it there, under `key` (its saved
	 * text). The layout is written only when it differs, so a layout read from another window is
	 * never written back: two windows never echo each other's edits.
	 */
	private stored: string | null = null;
	private want: { key: string; params: Record<string, unknown> } | null = null;
	/** The subscription last sent on this connection, by key ('' for none). */
	private sentKey = '';
	private timer: ReturnType<typeof setTimeout> | null = null;
	private inflight = false;

	constructor(control: ServiceClient, hooks: ScreenHooks, opts: ScreenOptions) {
		this.control = control;
		this.hooks = hooks;
		this.opts = opts;
		this.storage = opts.storage === undefined ? storageOf('localStorage') : opts.storage;
		this.session = opts.session === undefined ? storageOf('sessionStorage') : opts.session;
		this.pageKey = `hertzforge.screen.page:${hooks.testType}`;
		this.layout = hooks.defaultLayout(DEFAULT_ASPECT);
		this.pageId = opts.page ?? read(this.session, this.pageKey) ?? this.layout.pages[0].id;
		if (hooks.feed) control.on(hooks.feed.topic, (d) => (this.data = d as unknown as D));
	}

	// =========================================================================================
	// Reading

	get offer(): Offer {
		return { types: this.hooks.types, cards: this.hooks.cards };
	}

	get pages(): ScreenPage[] {
		return this.layout.pages;
	}

	get page(): ScreenPage {
		return this.layout.pages.find((p) => p.id === this.pageId) ?? this.layout.pages[0];
	}

	get dock(): Dock {
		return this.layout.dock;
	}

	get canAddPage(): boolean {
		return this.layout.pages.length < MAX_PAGES;
	}

	get canRemovePage(): boolean {
		return this.layout.pages.length > 1;
	}

	get canAddPlot(): boolean {
		const page = this.page;
		return page.panels.length < MAX_PANELS && page.panels.filter((p) => p.kind === 'plot').length < MAX_PLOTS;
	}

	typeOf(quantity: string): PlotType | null {
		return this.hooks.types.find((t) => t.id === quantity) ?? null;
	}

	/** A panel and the page it is on, or null. */
	find(id: string): { page: ScreenPage; panel: Panel } | null {
		for (const page of this.layout.pages) {
			const panel = page.panels.find((p) => p.id === id);
			if (panel) return { page, panel };
		}
		return null;
	}

	plot(id: string): PlotPanelSpec | null {
		const p = this.find(id)?.panel;
		return p?.kind === 'plot' ? p.plot : null;
	}

	titleOf(panel: Panel): string {
		return panel.kind === 'plot' ? plotTitle(panel.plot, this.typeOf(panel.plot.quantity)) : panel.card;
	}

	/** The plots this window asks the service for now: those on its page, while it shows them. */
	get wanted(): PlotPanel[] {
		return this.opts.showing() ? this.page.panels.filter((p): p is PlotPanel => p.kind === 'plot') : [];
	}

	/** A plot's data in the last frame, or null. */
	frame(id: string): FramePlot | null {
		return this.data?.plots.find((p) => p.id === id) ?? null;
	}

	views(id: string): View[] {
		return this.frame(id)?.views ?? [];
	}

	/** The page a card is placed on, with its panel, or null when it is in the dock or hidden. */
	cardPlace(kind: CardKind): { page: ScreenPage; panel: CardPanel } | null {
		for (const page of this.layout.pages) {
			const panel = page.panels.find((p): p is CardPanel => p.kind === 'card' && p.card === kind);
			if (panel) return { page, panel };
		}
		return null;
	}

	/** The card is in view in this window: open or folded in the dock, or on the page it shows. */
	cardInView(kind: CardKind): boolean {
		const place = this.cardPlace(kind);
		if (place) return place.page.id === this.page.id && (this.maximized === null || this.maximized === place.panel.id);
		return this.dock.side !== 'collapsed' && this.dock.cards.some((c) => c.kind === kind && c.shown);
	}

	// =========================================================================================
	// Pages

	selectPage(id: string): void {
		if (!this.layout.pages.some((p) => p.id === id)) return;
		this.pageId = id;
		this.maximized = null;
		this.options = null;
		write(this.session, this.pageKey, id);
	}

	addPage(name?: string): string | null {
		if (!this.canAddPage) return null;
		const id = nextId(this.layout, 'pg');
		const clean = (name && cleanName(name)) || freePageName(this.layout);
		this.layout.pages.push({ id, name: clean, cols: 12, rows: 12, panels: [], linkTime: false });
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

	/** Remove a page; the last page stays. Cards on it go back to the dock. */
	removePage(id: string): boolean {
		if (!this.canRemovePage) return false;
		const i = this.layout.pages.findIndex((p) => p.id === id);
		if (i < 0) return false;
		for (const p of this.layout.pages[i].panels) if (p.kind === 'card') this.showCard(p.card, true);
		this.layout.pages.splice(i, 1);
		if (this.pageId === id) this.selectPage(this.layout.pages[Math.max(0, i - 1)].id);
		return true;
	}

	/** Move a page one place earlier (-1) or later (+1) among the tabs. */
	movePage(id: string, by: -1 | 1): void {
		const pages = this.layout.pages;
		const i = pages.findIndex((p) => p.id === id);
		const j = i + by;
		if (i < 0 || j < 0 || j >= pages.length) return;
		[pages[i], pages[j]] = [pages[j], pages[i]];
	}

	setGrid(n: number, pageId = this.pageId): void {
		const page = this.layout.pages.find((p) => p.id === pageId);
		if (!page || !(GRID_SIZES as readonly number[]).includes(n) || (page.cols === n && page.rows === n)) return;
		page.panels = grid.rescale(page.panels, page, { cols: n, rows: n }, this.aspect);
		page.cols = n;
		page.rows = n;
	}

	setLinkTime(on: boolean, pageId = this.pageId): void {
		const page = this.layout.pages.find((p) => p.id === pageId);
		if (page) page.linkTime = on;
	}

	setRate(rate: number): void {
		this.layout.rate = rate;
	}

	arrangeEvenly(pageId = this.pageId): void {
		const page = this.layout.pages.find((p) => p.id === pageId);
		if (page) page.panels = grid.arrangeEvenly(page.panels, page.cols, page.rows, this.aspect);
	}

	/** Back to the test type's own pages. */
	resetLayout(): void {
		this.layout = this.hooks.defaultLayout(this.aspect);
		this.selectPage(this.layout.pages[0].id);
	}

	// =========================================================================================
	// Panels

	/** A new panel on `page`: in its largest free space, else with the page tiled evenly again. */
	private place(page: ScreenPage, panel: Panel): void {
		const minW = Math.max(1, Math.ceil(page.cols / 4));
		const minH = Math.max(1, Math.ceil(page.rows / 4));
		const free = grid.largestFree(page.panels, page.cols, page.rows, minW, minH);
		if (free) {
			page.panels.push({ ...panel, at: free });
		} else {
			page.panels = grid.arrangeEvenly([...page.panels, { ...panel, at: { x: page.cols, y: page.rows, w: 1, h: 1 } }], page.cols, page.rows, this.aspect);
		}
	}

	/** Add a plot of `type` to the page on screen; its id, or null when the page is full. */
	addPlot(type: PlotType | string, channels: ChannelSelector = 'all'): string | null {
		const t = typeof type === 'string' ? this.typeOf(type) : type;
		if (!t || !this.canAddPlot) return null;
		const id = nextId(this.layout, 'pl');
		this.place(this.page, { id, kind: 'plot', at: { x: 0, y: 0, w: 1, h: 1 }, plot: newPlotSpec(t, channels) });
		return id;
	}

	/** A plot's choice changed: a whole new one, or some of its fields. */
	updatePlot(id: string, patch: Partial<PlotPanelSpec>): void {
		const p = this.find(id)?.panel;
		if (p?.kind === 'plot') p.plot = { ...p.plot, ...patch };
	}

	/** Remove a panel; a card goes back to the dock. The page closes the gap. */
	removePanel(id: string): void {
		const found = this.find(id);
		if (!found) return;
		const { page, panel } = found;
		page.panels = grid.compact(page.panels.filter((p) => p.id !== id));
		if (panel.kind === 'card') this.showCard(panel.card, true);
		if (this.maximized === id) this.maximized = null;
		if (this.options === id) this.options = null;
	}

	private apply(id: string, change: (panels: Panel[], cols: number, rows: number) => Panel[] | null): boolean {
		const found = this.find(id);
		if (!found) return false;
		const out = change(found.page.panels, found.page.cols, found.page.rows);
		if (!out) return false;
		found.page.panels = out;
		return true;
	}

	/** Move a panel so its top-left cell is (`x`, `y`); false when there is no room. */
	movePanel(id: string, x: number, y: number): boolean {
		return this.apply(id, (ps, cols, rows) => grid.moveTo(ps, id, x, y, cols, rows));
	}

	/** Resize a panel to `w` by `h` cells; false when there is no room. */
	resizePanel(id: string, w: number, h: number): boolean {
		return this.apply(id, (ps, cols, rows) => grid.resize(ps, id, w, h, cols, rows));
	}

	/** Move a panel by a cell (the keyboard); false when it cannot go that way. */
	stepPanel(id: string, dx: number, dy: number): boolean {
		return this.apply(id, (ps, cols, rows) => grid.step(ps, id, dx, dy, cols, rows));
	}

	/** Grow or shrink a panel by a cell (the keyboard); false when it cannot. */
	growPanel(id: string, dw: number, dh: number): boolean {
		return this.apply(id, (ps, cols, rows) => grid.grow(ps, id, dw, dh, cols, rows));
	}

	/** Whether a keyboard step or resize would do anything, without doing it. */
	canStep(id: string, dx: number, dy: number): boolean {
		const f = this.find(id);
		return !!f && grid.step(f.page.panels, id, dx, dy, f.page.cols, f.page.rows) !== null;
	}

	canGrow(id: string, dw: number, dh: number): boolean {
		const f = this.find(id);
		return !!f && grid.grow(f.page.panels, id, dw, dh, f.page.cols, f.page.rows) !== null;
	}

	toggleMaximize(id: string): void {
		this.maximized = this.maximized === id ? null : this.find(id) ? id : null;
	}

	openOptions(id: string | null): void {
		this.options = id;
	}

	// =========================================================================================
	// The dock

	private dockCard(kind: CardKind) {
		return this.layout.dock.cards.find((c) => c.kind === kind) ?? null;
	}

	/** Put the dock on the right or at the bottom, or fold it to a rail (and open it again). */
	setDockSide(side: Dock['side']): void {
		const d = this.layout.dock;
		if (side === d.side) return;
		if (side === 'collapsed') {
			d.side = 'collapsed';
			return;
		}
		if (side !== d.restore) d.size = DOCK_SIZE[side].start;
		d.side = side;
		d.restore = side;
	}

	setDockSize(px: number): void {
		const d = this.layout.dock;
		const b = DOCK_SIZE[d.restore];
		d.size = Math.round(Math.min(b.max, Math.max(b.min, px)));
	}

	setCardOpen(kind: CardKind, open: boolean): void {
		const c = this.dockCard(kind);
		if (c) c.open = open;
	}

	/** Show a card in the dock (taking it off a page), or hide it. */
	showCard(kind: CardKind, shown: boolean): void {
		const c = this.dockCard(kind);
		if (!c) return;
		if (shown) {
			const place = this.cardPlace(kind);
			if (place) {
				place.page.panels = grid.compact(place.page.panels.filter((p) => p.id !== place.panel.id));
				if (this.maximized === place.panel.id) this.maximized = null;
			}
		}
		c.shown = shown;
	}

	/** Move a card to `index` in the dock's order. */
	moveCard(kind: CardKind, index: number): void {
		const cards = this.layout.dock.cards;
		const i = cards.findIndex((c) => c.kind === kind);
		if (i < 0) return;
		const to = Math.max(0, Math.min(cards.length - 1, index));
		if (to === i) return;
		const [c] = cards.splice(i, 1);
		cards.splice(to, 0, c);
	}

	/** Place a card on the page on screen, out of the dock; its panel's id, or null. */
	cardToPage(kind: CardKind): string | null {
		const c = this.dockCard(kind);
		const page = this.page;
		if (!c || this.cardPlace(kind) || page.panels.length >= MAX_PANELS) return null;
		const id = nextId(this.layout, 'cd');
		this.place(page, { id, kind: 'card', at: { x: 0, y: 0, w: 1, h: 1 }, card: kind });
		c.shown = false;
		return id;
	}

	/** Take a card panel off its page, back into the dock. */
	cardToDock(panelId: string): void {
		const p = this.find(panelId)?.panel;
		if (p?.kind === 'card') this.showCard(p.card, true);
	}

	// =========================================================================================
	// Wiring

	/** Load the layout for the setup on screen, keep it saved, and keep the subscription current. */
	watch(): () => void {
		// Another window wrote a layout. Storage holds the latest write of any window, which may be
		// newer than this event's own value, so every window ends on the same one.
		const onStorage = (ev: Event) => {
			if ((ev as StorageEvent).key === this.key) this.adopt();
		};
		const events = this.opts.events === undefined ? (typeof window === 'undefined' ? null : window) : this.opts.events;
		events?.addEventListener('storage', onStorage);
		const stop = $effect.root(() => {
			// The layout follows the setup: its own, else the test type's last, else the defaults.
			$effect(() => {
				const title = this.opts.setupTitle();
				untrack(() => this.load(title));
			});
			// Every change made here is kept, for this setup and as the test type's last. A layout read
			// from storage is already kept, and writing it back would answer the other window's edit
			// with its own echo.
			$effect(() => {
				const text = toSaved($state.snapshot(this.layout) as ScreenLayout);
				const key = this.key;
				untrack(() => {
					if (!key) return;
					if (text !== this.stored) {
						this.stored = text;
						write(this.storage, key, text);
					}
					write(this.storage, lastScreenKey(this.hooks.testType), text);
				});
			});
			// The subscription follows the plots on screen, on every connection: a subscription belongs
			// to its connection, so a new one is asked again.
			const feed = this.hooks.feed;
			if (feed) {
				$effect(() => {
					const params = feed.params($state.snapshot(this.wanted) as PlotPanel[], this.layout.rate);
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
			}
		});
		return () => {
			events?.removeEventListener('storage', onStorage);
			this.cancelTimer();
			stop();
		};
	}

	private parse(text: string | null): ScreenLayout | null {
		return fromSaved(text, this.offer, this.aspect);
	}

	private legacy(key: string): ScreenLayout | null {
		const l = this.hooks.legacy;
		return l ? l.read(read(this.storage, key)) : null;
	}

	private load(title: string | null) {
		const key = screenKey(this.hooks.testType, title);
		if (key === this.key) return;
		const own = this.parse(read(this.storage, key));
		const legacy = this.hooks.legacy;
		// A setup's layout from before is taken over once: written in the new form the first time it is read.
		const next =
			own ??
			(legacy ? this.legacy(legacy.key(title)) : null) ??
			this.parse(read(this.storage, lastScreenKey(this.hooks.testType))) ??
			(legacy ? this.legacy(legacy.last) : null) ??
			this.hooks.defaultLayout(this.aspect);
		this.key = key;
		// A setup with no layout of its own gets this one written for it; its own is kept as it is.
		this.stored = own ? toSaved(own) : null;
		this.layout = next;
		if (!next.pages.some((p) => p.id === this.pageId)) this.pageId = next.pages[0].id;
		this.maximized = null;
		this.options = null;
	}

	/** Take the layout kept for this setup, as another window left it. */
	private adopt() {
		const next = this.parse(read(this.storage, this.key));
		if (!next) return;
		const text = toSaved(next);
		this.stored = text;
		if (text === toSaved($state.snapshot(this.layout) as ScreenLayout)) return;
		this.layout = next;
		if (!next.pages.some((p) => p.id === this.pageId)) this.pageId = next.pages[0].id;
		if (this.maximized && !this.find(this.maximized)) this.maximized = null;
		if (this.options && !this.find(this.options)) this.options = null;
	}

	/**
	 * Send what is wanted after the debounce. A change while the timer runs does not restart it: the
	 * timer sends whatever is wanted when it fires, so changes coming faster than the debounce delay
	 * a subscription by one debounce at most and never hold it back.
	 */
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

	/**
	 * One subscription in flight at a time: the service may take two requests in either order, so
	 * a change made while one is on its way is sent when it has been answered (the latest only).
	 */
	private async send() {
		const want = this.want;
		const feed = this.hooks.feed;
		if (this.inflight || !want || !feed || want.key === this.sentKey) return;
		this.inflight = true;
		this.sentKey = want.key;
		try {
			this.accepted = await this.control.request<A>(feed.method, want.params);
			this.error = null;
		} catch (err) {
			this.error = toRefusal('Show the plots', err).message;
		} finally {
			this.inflight = false;
			if (this.want && this.want.key !== this.sentKey) void this.send();
		}
	}
}
