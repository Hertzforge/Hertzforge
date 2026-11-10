/**
 * What a plot panel on a test screen shows and how it draws it: the plot types a test offers, the
 * panel's choice among them, and its display options. Plain data, saved with the screen's layout.
 *
 * Every number drawn is the service's. The display options choose what is shown and how (scales,
 * ranges, lines, legend, cursors); a hold or an average is drawn as such and named in the legend.
 */

import type { ChannelSelector } from '$lib/protocol/types';

/** What a test type offers to plot: one entry per plot type, in the order `+ Plot` lists them. */
export interface PlotType {
	/** The service's quantity id for the test type (`control`, `time`, `srs`, ...). */
	id: string;
	/** Its name in the add menu and as the default title. */
	words: string;
	/** One plain line on what it shows. */
	help: string;
	group: 'frequency' | 'time' | 'numbers' | 'other';
	/** Takes a channel selector. */
	channels: boolean;
	/** The named channel choices the service takes for it; all three when absent. */
	selectors?: ('all' | 'control' | 'limited')[];
	/** Drawn against time: takes a point count and a time window, waveform or envelope. */
	time: boolean;
	/** Unit choices the service offers for the quantity; absent when it has one. */
	units?: { id: string; words: string }[];
	/** The families of lines it draws, which the Display section switches on and off. */
	families?: LineFamily[];
}

/** The line families a plot may draw, by the role the service gives each series. */
export type LineFamily = 'measured' | 'reference' | 'tolerance' | 'abort' | 'limit' | 'drive' | 'other';

export type AxisScale = 'auto' | 'log' | 'linear';
/** `auto` ranges itself; `band` keeps the test's band (x); `fixed` uses min and max; `held` stays where it is. */
export type RangeMode = 'auto' | 'band' | 'fixed' | 'held';

export interface AxisDisplay {
	/** `auto` keeps the service's scale for the quantity. */
	scale: AxisScale;
	range: RangeMode;
	/** Used when `range` is `fixed`. */
	min: number | null;
	max: number | null;
	/** Replaces the service's axis label; null keeps it. */
	label: string | null;
	/** One of the plot type's unit choices; null keeps the service's. */
	unit: string | null;
	ticks: 'sparse' | 'normal' | 'dense';
}

export type GridLines = 'major' | 'minor' | 'none';
export type LegendPlace = 'inside' | 'outside' | 'off';
export type Corner = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';
export type CursorMode = 'none' | 'one' | 'two' | 'peak' | 'peaks' | 'harmonics';
export type HoldMode = 'none' | 'peak' | 'min' | 'average' | 'exponential';
export type Dash = 'solid' | 'dashed' | 'dotted' | 'dashdot';
/** A shape drawn along a line at intervals: a cue besides colour and dash. */
export type Marker = 'none' | 'circle' | 'square' | 'triangle' | 'diamond' | 'cross';

/** One series' look, by the series' name; null fields keep the role's own look. */
export interface TraceStyle {
	visible: boolean;
	width: number | null;
	dash: Dash | null;
	color: string | null;
	/** Absent or null keeps the plot's own choice. */
	marker?: Marker | null;
}

/** A note placed on the plot, at fractions of its drawing area. */
export interface PlotNote {
	id: string;
	text: string;
	fx: number;
	fy: number;
}

export interface PlotDisplay {
	x: AxisDisplay;
	y: AxisDisplay;
	grid: GridLines;
	legend: LegendPlace;
	legendCorner: Corner;
	/** The cursor's readout drawn in the plot. */
	readout: boolean;
	/** Families switched off; a family not listed is shown. */
	hidden: LineFamily[];
	/** By series name. */
	traces: Record<string, TraceStyle>;
	cursor: CursorMode;
	/** Where the two cursors sit on x (the second only with `two`); null where not placed. */
	cursorAt?: [number | null, number | null];
	/** How many peaks `peaks` marks, and how many harmonics `harmonics` marks. */
	count: number;
	hold: HoldMode;
	/** Frames in an average, or the exponential average's time constant in frames. */
	average: number;
	/** Raised by Reset: a hold starts again whenever it changes. */
	holdEpoch?: number;
	/** Time plots: each sample, or each pixel's minimum and maximum. */
	timeRender: 'waveform' | 'envelope';
	/** Several channels on a time plot: on one pane, or one lane each. */
	lanes: 'overlay' | 'split';
	/** Time plots: the window shown, seconds; null for what the service sends. */
	window_s: number | null;
	notes: PlotNote[];
}

/** One plot panel's choice: what it shows and how. */
export interface PlotPanelSpec {
	/** A `PlotType.id` of the test type. */
	quantity: string;
	channels: ChannelSelector;
	/** Several channels: one pane each, or all on one pane. */
	arrange: 'split' | 'overlay';
	/** Time plots: the most points per channel. */
	points: number;
	/** Null for the plot type's own title. */
	title: string | null;
	display: PlotDisplay;
}

export function defaultAxis(): AxisDisplay {
	return { scale: 'auto', range: 'auto', min: null, max: null, label: null, unit: null, ticks: 'normal' };
}

export function defaultDisplay(): PlotDisplay {
	return {
		x: defaultAxis(),
		y: defaultAxis(),
		grid: 'major',
		legend: 'inside',
		legendCorner: 'top-right',
		readout: true,
		hidden: [],
		traces: {},
		cursor: 'none',
		count: 5,
		hold: 'none',
		average: 8,
		timeRender: 'waveform',
		lanes: 'overlay',
		window_s: null,
		notes: []
	};
}

/** A new plot of `type`, with the type's own defaults. */
export function newPlotSpec(type: PlotType, channels: ChannelSelector = 'all'): PlotPanelSpec {
	return { quantity: type.id, channels: type.channels ? channels : 'all', arrange: type.time ? 'overlay' : 'split', points: 1024, title: null, display: defaultDisplay() };
}
