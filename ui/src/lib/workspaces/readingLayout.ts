import type { View } from '$lib/protocol/types';

/** The views that fill whatever height they are given; a table or a reading takes its rows. */
const FILLS = new Set<View['view']>(['spectrum', 'waterfall', 'matrix', 'shape3d']);

/** A reading of one plot, which fills its frame instead of keeping a plot's height in a column. */
export function lonePlot(views: View[]): boolean {
	return views.length === 1 && FILLS.has(views[0].view);
}
