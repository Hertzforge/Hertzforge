/**
 * A test type's own body for a plot type: what a panel draws in place of the plain plot body. It is
 * given the panel and the plot body as a snippet, so it can draw the plot with marks of its own
 * (a line at the tone, the limit marks) and words around it, keeping the panel's options, export
 * and trace list; or draw something else altogether.
 */

import type { Snippet } from 'svelte';
import type { PlotPanelSpec } from '../plots/types';
import type { Panel } from './types';

/** What a test type adds to a plot drawn with the plot body. */
export interface PlotExtras {
	/** A line at this x on every pane. */
	marker?: number | null;
	/** Thin lines at these x values on every pane. */
	guides?: number[];
	/** The x range the plot draws, as it changes. */
	onrange?: (low: number, high: number) => void;
}

/** A plot panel, as a body is given it. */
export type BodyPanel = Extract<Panel, { kind: 'plot' }>;

/** A test type's own body for a plot type: the panel, and the plot body to draw it with. */
export type PlotBodySnippet = Snippet<[BodyPanel, Snippet<[PlotExtras]>]>;

/** A test type's own bodies, by plot type. */
export type PlotBodies = Partial<Record<string, PlotBodySnippet>>;

/** The body a panel draws in place of the plain plot body, or null for the plain one (and for a card). */
export function customBody(bodies: PlotBodies | null | undefined, panel: Panel): PlotBodySnippet | null {
	if (panel.kind !== 'plot' || !bodies || !Object.hasOwn(bodies, panel.plot.quantity)) return null;
	return bodies[panel.plot.quantity] ?? null;
}

/**
 * A test type's own choices in a plot's options menu, under what the plot shows: the plot's choice
 * and how to change it (a control or a pair, a grid's pinned controls, a heatmap's order).
 */
export type PlotDataSnippet = Snippet<[PlotPanelSpec, (spec: PlotPanelSpec) => void]>;
