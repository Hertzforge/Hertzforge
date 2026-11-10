/**
 * The page kinds of the test types on the pulse page layer (Sine, classical shock, SRS shock, TWR,
 * Explorer and the Recorder): each type's kinds and the quantities each draws, which make a
 * screen's first pages, and the form the page layer kept a layout in, read once to take it over.
 * Also the pulses an operator picks in the pulse table to lay over a plot: nothing here computes a
 * measurement; an overlay only places the service's own series side by side, each named for its
 * pulse.
 */

import type { ChannelSelector, Series, SpectrumView, View } from '$lib/protocol/types';

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

/**
 * A test type's page kinds: their words and the quantities each draws, which quantities take a
 * channel selector or a point count, and what a kept layout may hold.
 */
export interface PulsePageSet<K extends string, Q extends string> {
	/** The test type a layout is kept under. */
	testType: string;
	kinds: readonly K[];
	words: Record<K, string>;
	/** The quantities each page draws, in order. */
	quantities: Record<K, readonly Q[]>;
	/** The quantities drawn as time histories: the only plots that take a point count. */
	timeQuantities: readonly Q[];
	/** The rates a kept layout may hold. */
	rates: readonly number[];
	defaultRate: number;
	maxPages: number;
	/** The pages a new layout starts with, in order; every kind when absent. */
	defaults?: readonly K[];
	/** The quantities that take a channel selector; none when absent. */
	channelQuantities?: readonly Q[];
	/** The channels each page asks for on those quantities; every input when a page names none. */
	channels?: Partial<Record<K, ChannelSelector>>;
}

/** Pulses laid over a page at once. */
export const MAX_OVERLAY = 6;

/** A layout the page layer kept, read field by field; null when it is not one of this type's. */
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
