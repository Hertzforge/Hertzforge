/**
 * Holds and display averages over successive frames: the largest or smallest value seen at each
 * point, the mean of the last N frames, or an exponential average with a time constant of N
 * frames. Each is kept per series and drawn beside the live series, named as what it is, so a
 * held line is never taken for the service's own.
 *
 * A series' hold starts again when its x grid changes (another resolution, another band) and on
 * Reset. Values that are not finite are left out: a gap in one frame does not wipe a hold.
 */

import type { NumericArray, Series, SpectrumView, View } from '$lib/protocol/types';
import type { HoldMode, LineFamily } from './types';

export type HoldKind = Exclude<HoldMode, 'none'>;

/** The families a hold applies to: measurements, never the specification's lines. */
export const HELD_FAMILIES: LineFamily[] = ['measured', 'drive', 'other'];

interface Track {
	grid: string;
	/** peak, min and exponential: the running value; average: unused. */
	value: Float64Array;
	/** average: the last frames, oldest first. */
	frames: Float64Array[];
	count: number;
}

/** A short fingerprint of an x grid: its length and a few of its values. */
export function gridKey(x: NumericArray, edges?: NumericArray): string {
	const n = x.length;
	if (n === 0) return '0';
	const pick = [0, n >> 2, n >> 1, (3 * n) >> 2, n - 1].map((i) => x[i]);
	return `${n}:${pick.join(',')}:${edges?.length ?? 0}`;
}

/** The legend's words for a hold, as a suffix to the series' name. */
export function holdWords(kind: HoldKind, frames: number, count: number): string {
	switch (kind) {
		case 'peak':
			return 'peak hold';
		case 'min':
			return 'minimum hold';
		case 'average':
			return `average of ${Math.min(count, frames)}`;
		case 'exponential':
			return `exponential average, ${frames} frames`;
	}
}

export class Holder {
	private tracks = new Map<string, Track>();
	private config = '';
	private kind: HoldKind = 'peak';
	private frames = 1;

	/** Set the hold; a change of kind, of frames or of `epoch` (Reset) starts every series again. */
	configure(kind: HoldKind, frames: number, epoch = 0): void {
		const next = `${kind}:${frames}:${epoch}`;
		if (next !== this.config) {
			this.config = next;
			this.kind = kind;
			this.frames = Math.max(1, Math.round(frames) || 1);
			this.tracks.clear();
		}
	}

	reset(): void {
		this.tracks.clear();
	}

	/** How many series are held. */
	get size(): number {
		return this.tracks.size;
	}

	/**
	 * Fold one frame of a series into its hold, kept under `key`, and return the held series: the
	 * same x (and bins), its own y, its name saying what it is. The kind and frames are those last
	 * given to `configure`.
	 */
	step(key: string, s: Series): Series {
		const { kind, frames } = this;
		const n = Math.min(s.x.length, s.y.length);
		const grid = gridKey(s.x, s.edges);
		let t = this.tracks.get(key);
		if (!t || t.grid !== grid || t.value.length !== n) {
			t = { grid, value: new Float64Array(n).fill(Number.NaN), frames: [], count: 0 };
			this.tracks.set(key, t);
		}
		t.count++;
		const y = s.y;
		let out: Float64Array;
		if (kind === 'average') {
			t.frames.push(Float64Array.from({ length: n }, (_, i) => y[i]));
			while (t.frames.length > frames) t.frames.shift();
			out = new Float64Array(n);
			for (let i = 0; i < n; i++) {
				let sum = 0;
				let k = 0;
				for (const f of t.frames) {
					if (Number.isFinite(f[i])) {
						sum += f[i];
						k++;
					}
				}
				out[i] = k ? sum / k : Number.NaN;
			}
		} else {
			const v = t.value;
			for (let i = 0; i < n; i++) {
				const now = y[i];
				if (!Number.isFinite(now)) continue;
				const was = v[i];
				if (!Number.isFinite(was)) v[i] = now;
				else if (kind === 'peak') v[i] = Math.max(was, now);
				else if (kind === 'min') v[i] = Math.min(was, now);
				else v[i] = was + (now - was) / frames;
			}
			out = Float64Array.from(v);
		}
		const held: Series = { name: `${s.name}, ${holdWords(kind, frames, t.count)}`, role: s.role, x: s.x, y: out };
		if (s.shape) held.shape = s.shape;
		if (s.edges) held.edges = s.edges;
		return held;
	}

	/** Forget series not among `keys`: a channel taken off the plot. */
	keep(keys: Set<string>): void {
		for (const k of [...this.tracks.keys()]) if (!keys.has(k)) this.tracks.delete(k);
	}
}

/**
 * The views with each measured series' hold drawn beside it. With `kind` none the views come
 * back as they are and the holder is emptied.
 */
export function withHolds(holder: Holder, views: View[], kind: HoldMode, frames: number, epoch = 0): View[] {
	if (kind === 'none') {
		holder.reset();
		return views;
	}
	holder.configure(kind, frames, epoch);
	const seen = new Set<string>();
	const out = views.map((v, vi): View => {
		if (v.view !== 'spectrum') return v;
		const extra: Series[] = [];
		v.series.forEach((s) => {
			if (!(HELD_FAMILIES as string[]).includes(s.role) || s.follows !== undefined) return;
			const key = `${vi}\u0000${v.title}\u0000${s.name}`;
			seen.add(key);
			extra.push(holder.step(key, s));
		});
		return extra.length ? ({ ...v, series: [...v.series, ...extra] } satisfies SpectrumView) : v;
	});
	holder.keep(seen);
	return out;
}
