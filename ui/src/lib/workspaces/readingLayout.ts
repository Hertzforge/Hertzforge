import type { View } from '$lib/protocol/types';

/** The views that fill whatever height they are given; a table or a reading takes its rows. */
const FILLS = new Set<View['view']>(['spectrum', 'waterfall', 'matrix', 'shape3d']);

/** A reading of one plot, which fills its frame instead of keeping a plot's height in a column. */
export function lonePlot(views: View[]): boolean {
	return views.length === 1 && FILLS.has(views[0].view);
}

/**
 * Whether a reading draws one set of values two ways, as a surface (`waterfall`) and flat (`matrix`),
 * as a wavelet's scalogram is: the frame's 3D/2D toggle then shows one of them.
 */
export function twoForms(views: View[]): boolean {
	return views.some((v) => v.view === 'waterfall') && views.some((v) => v.view === 'matrix');
}

/** The views a reading shows: of a surface drawn two ways, the flat one in 2D and the surface in 3D. */
export function shownForms(views: View[], flat: boolean): View[] {
	if (!twoForms(views)) return views;
	const hidden = flat ? 'waterfall' : 'matrix';
	return views.filter((v) => v.view !== hidden);
}
