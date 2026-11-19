/**
 * How a plot presents the views the service sent for it: the operator's axis choices, and several
 * channels on one pane. Presentation only: every number drawn is the service's, unchanged.
 */

import type { DataPlot, SpectrumView, View } from '$lib/protocol/types';
import { plotTitle, type PlotSpec } from './layout';

/** The view with the operator's axis scales in place of the service's (`auto` keeps the service's). */
export function withScales(view: SpectrumView, plot: Pick<PlotSpec, 'x' | 'y'>): SpectrumView {
	const x = plot.x === 'auto' ? view.x : { ...view.x, scale: plot.x };
	const y = plot.y === 'auto' ? view.y : { ...view.y, scale: plot.y };
	return x === view.x && y === view.y ? view : { ...view, x, y };
}

/**
 * Several channels' views on one pane: every series of every view, each named for its channel so
 * the legend and the cursor read it by name. The axes are the first view's (the service sends one
 * quantity per plot, so they agree).
 */
export function overlay(plot: PlotSpec, data: DataPlot): SpectrumView | null {
	const views = data.views.filter((v): v is SpectrumView => v.view === 'spectrum');
	if (!views.length) return null;
	const series = views.flatMap((v, i) => {
		const ch = data.channels?.[i];
		return v.series.map((s) => (s.role === 'measured' || ch === undefined ? s : { ...s, name: `${s.name}, channel ${ch}` }));
	});
	return { ...views[0], title: plotTitle(plot), series };
}

/** The views a plot draws, in order, with the operator's arrangement and axes applied. */
export function presented(plot: PlotSpec, data: DataPlot | null): View[] {
	if (!data) return [];
	const several = (plot.quantity === 'response' || plot.quantity === 'time') && data.views.length > 1;
	const views: View[] = several && plot.arrange === 'overlay' ? [overlay(plot, data)].filter((v): v is SpectrumView => v !== null) : data.views;
	return views.map((v) => (v.view === 'spectrum' ? withScales(v, plot) : v));
}

/** The fixed y range the operator set, or null for the plot's own. */
export function fixedY(plot: Pick<PlotSpec, 'yMin' | 'yMax' | 'y'>, view: SpectrumView): [number, number] | null {
	const { yMin, yMax } = plot;
	if (yMin === null || yMax === null || !(yMax > yMin)) return null;
	if (view.y.scale === 'log' && yMin <= 0) return null;
	return [yMin, yMax];
}
