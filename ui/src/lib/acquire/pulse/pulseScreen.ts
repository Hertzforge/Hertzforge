/**
 * The test screen for the test types on the pulse page layer (Sine, classical shock, SRS shock,
 * TWR, Explorer and the Recorder): every quantity the service draws offered as a plot type, the
 * pages a screen opens with, what a window asks the service for, and how a layout kept by the page
 * layer is taken over.
 *
 * A screen's first pages are its page kinds': each a page of that kind's quantities as plots tiled
 * evenly, with the kind's own cards on it where it has them (the pulse table on the Pulses page).
 * A layout kept before is taken over the same way, its pages in their order and with their names.
 */

import type { ChannelSelector } from '$lib/protocol/types';
import type { NoticesHost, RunBarHost, RunHead, SetupStatus } from '../actions';
import { lastLayoutKey, layoutKey } from '../display/layout';
import { newPlotSpec, type LineFamily, type PlotType } from '../plots/types';
import { tiling } from '../screen/grid';
import { DEFAULT_ASPECT, DEFAULT_RATE, defaultDock, RATES } from '../screen/layout';
import type { PlotPanel, ScreenHooks } from '../screen/screen.svelte';
import { GRID_COLS, GRID_ROWS, type CardKind, type Panel, type ScreenLayout, type ScreenPage } from '../screen/types';
import { pulseLayoutFromSaved, type PulsePageSet } from './pulseLayout';

/** What a test type's screen reads of its store, and the steps and sections it moves. */
export interface PulseScreenHost extends RunBarHost, NoticesHost {
	run: RunHead | null;
	step: 'setup' | 'run';
	section: string;
	counts: Record<string, { errors: number; warnings: number }>;
	locked: boolean;
	setupStatus: SetupStatus;
}

/** How a quantity is offered as a plot type. */
export interface QuantityWords {
	words: string;
	help: string;
	group: PlotType['group'];
	families?: LineFamily[];
}

/** A test type's screen on the pulse page layer. */
export interface PulseScreenSpec<K extends string, Q extends string> {
	/** Its page kinds, the quantities each draws, which take channels and which a point count. */
	pages: PulsePageSet<K, Q>;
	/** The quantities offered as plot types, in the order `+ Plot` lists them. */
	plots: readonly (readonly [Q, QuantityWords])[];
	/** Its cards, in the dock's first order. */
	cards: CardKind[];
	/** Cards a page kind holds besides its plots (a table that picks what the plots show). */
	pageCards?: Partial<Record<K, CardKind[]>>;
	/** The named channel choices the service takes; all three when absent. */
	selectors?: ('all' | 'control' | 'limited')[];
}

/** The plot types a test type offers: one per quantity it draws. */
export function pulsePlotTypes<K extends string, Q extends string>(spec: PulseScreenSpec<K, Q>): PlotType[] {
	return spec.plots.map(([id, w]) => {
		const channels = spec.pages.channelQuantities?.includes(id) ?? false;
		const t: PlotType = { id, words: w.words, help: w.help, group: w.group, channels, time: spec.pages.timeQuantities.includes(id) };
		if (w.families) t.families = [...w.families];
		if (channels && spec.selectors) t.selectors = [...spec.selectors];
		return t;
	});
}

/** A page's panels on cells of its own, tiled evenly in their order. */
function tiled(id: string, name: string, panels: Panel[], aspect: number): ScreenPage {
	const cells = tiling(panels.length, GRID_COLS, GRID_ROWS, aspect);
	return { id, name, cols: GRID_COLS, rows: GRID_ROWS, linkTime: false, panels: panels.map((p, i) => ({ ...p, at: cells[i] })) };
}

/**
 * A layout of these pages, each named, of its kind: the kind's cards first, then its quantities as
 * plots, tiled evenly. A card goes on the first page that holds it, and is not in the dock.
 */
export function pulseScreenLayout<K extends string, Q extends string>(
	spec: PulseScreenSpec<K, Q>,
	types: PlotType[],
	pages: readonly { name: string; kind: K }[],
	rate: number = spec.pages.defaultRate,
	aspect = DEFAULT_ASPECT
): ScreenLayout {
	let n = 0;
	const placed = new Set<CardKind>();
	const out = pages.map((p, i) => {
		const panels: Panel[] = [];
		for (const card of spec.pageCards?.[p.kind] ?? []) {
			if (placed.has(card) || !spec.cards.includes(card)) continue;
			placed.add(card);
			panels.push({ id: `cd-${++n}`, kind: 'card', at: { x: 0, y: 0, w: 1, h: 1 }, card });
		}
		for (const q of spec.pages.quantities[p.kind]) {
			const type = types.find((t) => t.id === q);
			if (!type) continue;
			const channels: ChannelSelector = (type.channels && spec.pages.channels?.[p.kind]) || 'all';
			panels.push({ id: `pl-${++n}`, kind: 'plot', at: { x: 0, y: 0, w: 1, h: 1 }, plot: newPlotSpec(type, channels) });
		}
		return tiled(`pg-${i + 1}`, p.name, panels, aspect);
	});
	const dock = defaultDock(spec.cards);
	for (const c of dock.cards) if (placed.has(c.kind)) c.shown = false;
	return { version: 2, pages: out, dock, rate: RATES.includes(rate) ? rate : DEFAULT_RATE, serial: Math.max(n, out.length) };
}

/** The test type's pages until the operator changes them: one for each of its default page kinds. */
export function defaultPulseScreen<K extends string, Q extends string>(spec: PulseScreenSpec<K, Q>, types: PlotType[], aspect = DEFAULT_ASPECT): ScreenLayout {
	const kinds = spec.pages.defaults ?? spec.pages.kinds;
	return pulseScreenLayout(
		spec,
		types,
		kinds.map((kind) => ({ name: spec.pages.words[kind], kind })),
		spec.pages.defaultRate,
		aspect
	);
}

/** A layout the page layer kept (pages of fixed kinds), taken over: the same pages, names and rate. */
export function pulseFromSaved<K extends string, Q extends string>(spec: PulseScreenSpec<K, Q>, types: PlotType[], text: string | null, aspect = DEFAULT_ASPECT): ScreenLayout | null {
	const v1 = pulseLayoutFromSaved(spec.pages, text);
	return v1 ? pulseScreenLayout(spec, types, v1.pages, v1.rate, aspect) : null;
}

/**
 * What a window sends for the plots it shows, or the default subscription when it shows none. A
 * channel selector goes with the plots that take one (a named choice the service does not take
 * for the quantity is sent as every channel), and a point count with the time histories only:
 * the service refuses either on any other plot.
 */
export function pulseSubscription(types: PlotType[], plots: PlotPanel[], rate: number): Record<string, unknown> {
	if (!plots.length) return { default: true };
	return {
		rate_hz: rate,
		plots: plots.map((p) => {
			const type = types.find((t) => t.id === p.plot.quantity);
			const out: Record<string, unknown> = { id: p.id, quantity: p.plot.quantity };
			if (type?.channels) {
				const c = p.plot.channels;
				out.channels = Array.isArray(c) ? [...c] : !type.selectors || type.selectors.includes(c) ? c : 'all';
			}
			if (type?.time) out.points = p.plot.points;
			return out;
		})
	};
}

/** Everything a test type gives its screen: its plot types, cards, first pages, layouts kept before and its feed. */
export function pulseHooks<K extends string, Q extends string>(spec: PulseScreenSpec<K, Q>, feed: { method: string; topic: string }): ScreenHooks {
	const types = pulsePlotTypes(spec);
	const testType = spec.pages.testType;
	return {
		testType,
		types,
		cards: spec.cards,
		defaultLayout: (aspect) => defaultPulseScreen(spec, types, aspect),
		legacy: {
			key: (title) => layoutKey(testType, title),
			last: lastLayoutKey(testType),
			read: (text) => pulseFromSaved(spec, types, text)
		},
		feed: { method: feed.method, topic: feed.topic, params: (plots, rate) => pulseSubscription(types, plots, rate) }
	};
}
