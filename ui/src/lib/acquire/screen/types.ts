/**
 * A test screen's layout: pages of panels on an even grid, and the control dock beside them.
 *
 * A page is a grid of equal cells that fills the space under the header; every panel is a
 * rectangle of whole cells. A panel is a plot or a card (the run's verbs, its level, its numbers,
 * its log). The dock holds the cards not placed on a page, on the right, at the bottom, or
 * collapsed to a rail. Plain data, saved per test type and per setup.
 */

import type { PlotPanelSpec } from '../plots/types';

/** A rectangle of whole grid cells: column and row of its top-left cell, width and height in cells. */
export interface Cell {
	x: number;
	y: number;
	w: number;
	h: number;
}

/**
 * The cards a screen offers. Shared ones first; a test type adds its own (`steer` for Explorer's
 * frequency and level, `law` for the live law values, `station`, `pulses`).
 */
export type CardKind = 'run' | 'level' | 'readings' | 'inputs' | 'log' | 'marks' | (string & {});

export type Panel = { id: string; at: Cell; kind: 'plot'; plot: PlotPanelSpec } | { id: string; at: Cell; kind: 'card'; card: CardKind };

export interface ScreenPage {
	id: string;
	name: string;
	/** The grid's size in cells. */
	cols: number;
	rows: number;
	panels: Panel[];
	/** Time plots on the page share one time window and pan together. */
	linkTime: boolean;
}

export interface DockCard {
	kind: CardKind;
	/** Expanded, or folded to its title. */
	open: boolean;
	/** In the dock; false when hidden from it (or placed on a page). */
	shown: boolean;
}

export interface Dock {
	side: 'right' | 'bottom' | 'collapsed';
	/** Width (right) or height (bottom), CSS pixels. */
	size: number;
	cards: DockCard[];
}

export interface ScreenLayout {
	version: 2;
	pages: ScreenPage[];
	dock: Dock;
	/** Display updates per second the window asks for. */
	rate: number;
	/** The last number given to a page or a panel; numbers are never given twice. */
	serial: number;
}

export const GRID_COLS = 12;
export const GRID_ROWS = 12;
