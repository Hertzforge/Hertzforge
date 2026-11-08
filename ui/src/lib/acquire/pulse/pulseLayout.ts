/**
 * The pages of a pulse-by-pulse run (classical shock, SRS shock), shared by both: a set of page
 * kinds and the quantities each draws, the plain-data form a layout is saved in, what a window
 * subscribes to, and the pulses an operator picks in the pulse table to lay over a page. Each test
 * type names its own pages and quantities in a `PulsePageSet`; nothing here computes a
 * measurement: an overlay only places the service's own series side by side, each named for its
 * pulse.
 */

import type { Series, SpectrumView, View } from '$lib/protocol/types';
import { pointsForWidth } from '../display/mimoLayout';

/** One page of a run view. */
export interface PulsePageSpec<K extends string = string> {
	id: string;
	name: string;
	kind: K;
}

export interface PulseLayout<K extends string = string> {
	version: 1;
	/** Frames per second the window asks for (the service sends once per pulse and on subscribe). */
	rate: number;
	serial: number;
	pages: PulsePageSpec<K>[];
}

/** One plot a page subscribes to. */
export interface PulsePlotSpec<Q extends string = string> {
	id: string;
	quantity: Q;
	points: number;
}

/** A test type's pages: their kinds, words and quantities, and the subscription's rules. */
export interface PulsePageSet<K extends string, Q extends string> {
	/** The test type a layout is kept under. */
	testType: string;
	/** The page ids' prefix (`sp-1`). */
	prefix: string;
	kinds: readonly K[];
	words: Record<K, string>;
	help: Record<K, string>;
	/** The quantities each page draws, in order. */
	quantities: Record<K, readonly Q[]>;
	/** The quantities drawn as time histories: the only plots that take a point count. */
	timeQuantities: readonly Q[];
	rates: readonly number[];
	defaultRate: number;
	maxPages: number;
}

/** Pulses laid over a page at once. */
export const MAX_OVERLAY = 6;

export function defaultPulseLayout<K extends string>(set: PulsePageSet<K, string>): PulseLayout<K> {
	return {
		version: 1,
		rate: set.defaultRate,
		serial: set.kinds.length,
		pages: set.kinds.map((kind, i) => ({ id: `${set.prefix}-${i + 1}`, name: set.words[kind], kind }))
	};
}

/** The plot id a page uses for one of its parts. */
export const pulsePlotId = (page: { id: string }, part: string) => `${page.id}:${part}`;

/** Everything a page subscribes to, at its drawn width (CSS pixels). */
export function pulsePagePlots<K extends string, Q extends string>(set: PulsePageSet<K, Q>, page: PulsePageSpec<K>, width = 900, pixelRatio = 1): PulsePlotSpec<Q>[] {
	const points = pointsForWidth(width, pixelRatio);
	return set.quantities[page.kind].map((quantity) => ({ id: pulsePlotId(page, quantity), quantity, points }));
}

/**
 * What a window sends: its plots, or the default subscription when it shows none. A point count
 * goes with the time histories only; the service refuses it on any other plot.
 */
export function pulseSubscribeParams<Q extends string>(set: { timeQuantities: readonly Q[] }, plots: PulsePlotSpec<Q>[], rate: number): Record<string, unknown> {
	const wanted = plots.map((p) => (set.timeQuantities.includes(p.quantity) ? { id: p.id, quantity: p.quantity, points: p.points } : { id: p.id, quantity: p.quantity }));
	return wanted.length ? { rate_hz: rate, plots: wanted } : { default: true };
}

export function pulseLayoutFromSaved<K extends string>(set: PulsePageSet<K, string>, text: string | null): PulseLayout<K> | null {
	if (!text) return null;
	const isKind = (v: unknown): v is K => typeof v === 'string' && (set.kinds as readonly string[]).includes(v);
	try {
		const raw = JSON.parse(text) as Partial<PulseLayout<K>>;
		if (raw.version !== 1 || !Array.isArray(raw.pages)) return null;
		const pages = raw.pages
			.filter((p): p is PulsePageSpec<K> => !!p && typeof p.id === 'string' && typeof p.name === 'string' && isKind(p.kind))
			.slice(0, set.maxPages)
			.map((p) => ({ id: p.id, name: p.name.slice(0, 40), kind: p.kind }));
		if (!pages.length || new Set(pages.map((p) => p.id)).size !== pages.length) return null;
		const rate = set.rates.includes(raw.rate as number) ? (raw.rate as number) : set.defaultRate;
		const serial = Number.isInteger(raw.serial) ? (raw.serial as number) : pages.length;
		return { version: 1, rate, serial, pages };
	} catch {
		return null;
	}
}

export function pulseLayoutToSaved(layout: PulseLayout): string {
	return JSON.stringify({ version: 1, rate: layout.rate, serial: layout.serial, pages: layout.pages.map((p) => ({ id: p.id, name: p.name, kind: p.kind })) });
}

// ---------------------------------------------------------------------------------------------
// Pulses laid over a page

/**
 * The pulses picked to lay over a page, after a click on a row: picked again, it is taken off; a
 * new one goes last, and the oldest falls off past `max`.
 */
export function toggleOverlay(picked: readonly number[], index: number, max = MAX_OVERLAY): number[] {
	if (picked.includes(index)) return picked.filter((i) => i !== index);
	const next = [...picked, index];
	return next.length > max ? next.slice(next.length - max) : next;
}

/** Which of a pulse's views is its pulse (time) view, and which its SRS. */
export function pulseViewOf(views: readonly View[], kind: 'pulse' | 'srs'): SpectrumView | null {
	const spectra = views.filter((v): v is SpectrumView => v.view === 'spectrum');
	const titled = spectra.find((v) => (kind === 'srs' ? /\bsrs\b|shock response/i.test(v.title) : /^pulse\b|\bpulse$/i.test(v.title.trim())));
	if (titled) return titled;
	if (kind === 'srs') return spectra.find((v) => v.x.unit === 'Hz' && v.x.scale === 'log') ?? null;
	return spectra.find((v) => v.x.unit !== 'Hz' && (v.y.unit ?? '') === 'g') ?? null;
}

/**
 * The page's view with each picked pulse's measured series laid over it, named for the pulse
 * ("Pulse 2, measured"). The base view's own series stay as the service sent them.
 */
export function withOverlay(base: SpectrumView | null, picked: readonly { index: number; views: readonly View[] }[], kind: 'pulse' | 'srs'): SpectrumView | null {
	if (!base) return null;
	const extra: Series[] = [];
	for (const p of picked) {
		const v = pulseViewOf(p.views, kind);
		if (!v) continue;
		for (const s of v.series) if (s.role === 'measured') extra.push({ ...s, name: `Pulse ${p.index + 1}, ${s.name.toLowerCase()}`, follows: undefined });
	}
	return extra.length ? { ...base, series: [...base.series, ...extra] } : base;
}

/**
 * A page's views with the picked pulses laid over its Pulse (or SRS) view, the others as the
 * service sent them. Before a window's first frame, the last pulse event's views stand in.
 */
export function overlaidViews(got: View[], lastViews: readonly View[], picked: readonly { index: number; views: readonly View[] }[], kind: 'pulse' | 'srs'): View[] {
	let list = got;
	if (!list.length) {
		const base = pulseViewOf(lastViews, kind);
		const drive = kind === 'pulse' ? lastViews.find((v) => v.view === 'spectrum' && /^drive\b/i.test(v.title)) : undefined;
		list = [base, drive].filter((v): v is SpectrumView => !!v);
	}
	const target = pulseViewOf(list, kind);
	return list.map((v) => (v === target ? (withOverlay(target, picked, kind) ?? v) : v));
}
