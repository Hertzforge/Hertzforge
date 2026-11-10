/**
 * Signals on the wire (schema/PROTOCOL.md, "Signals"): the time history of every channel a run
 * reads or drives, whatever the test type. A window subscribes for the channels its Signals plots
 * show and is sent `signals.data` while a run acquires, and once more when that run ends. Inputs
 * are in their row's unit, drives in volts, and the x axis is station time in seconds, counted
 * from the run's start at the rate the station programmed.
 */
import type { SpectrumView } from './types';

/** Event topics of Signals. */
export const SIGNALS_TOPICS = {
	data: 'signals.data'
} as const;

/** The methods of Signals. */
export const SIGNALS_METHODS = {
	subscribe: 'signals.subscribe',
	unsubscribe: 'signals.unsubscribe'
} as const;

/** Frames per second when a subscription does not say, and its bounds. */
export const SIGNALS_DEFAULT_RATE_HZ = 10;
export const SIGNALS_MIN_RATE_HZ = 0.5;
export const SIGNALS_MAX_RATE_HZ = 10;
/** The window a plot shows when it does not say, and the longest it may ask for, seconds. */
export const SIGNALS_DEFAULT_WINDOW_S = 2;
export const SIGNALS_MAX_WINDOW_S = 10;
/** Points per channel: the default and the bounds. */
export const SIGNALS_DEFAULT_POINTS = 1024;
export const SIGNALS_MIN_POINTS = 64;
export const SIGNALS_MAX_POINTS = 8192;
/** The most plots one subscription may hold. */
export const SIGNALS_MAX_PLOTS = 64;

/** Whether a channel is read or driven. */
export type SignalKind = 'input' | 'drive';

/** One channel a run reads or drives. */
export interface SignalChannel {
	/** The channel-table row's index. */
	index: number;
	/** The row's description, or its role when it has none. */
	name: string;
	/** The row's unit for an input; `V` for a drive. */
	unit: string;
	kind: SignalKind;
}

/** Which channels a plot shows: table rows, every channel, the inputs, the drives or the control rows. */
export type SignalChannelSelector = number[] | 'all' | 'inputs' | 'drives' | 'control';

/** `overlay`: one view for each unit, its channels drawn together; `split`: one view for each channel. */
export type SignalArrange = 'overlay' | 'split';

/** One plot of a subscription; every field but `id` takes the service's default when left out. */
export interface SignalsSubscribePlot {
	id: string;
	/** Default `all`. */
	channels?: SignalChannelSelector | null;
	/** Seconds, above 0 and at most 10; default 2. */
	window_s?: number | null;
	/** 64 to 8192; default 1024. */
	points?: number | null;
	/** Default `overlay`. */
	arrange?: SignalArrange | null;
}

export interface SignalsSubscribeParams {
	/** 0.5 to 10; default 10. */
	rate_hz?: number;
	plots: SignalsSubscribePlot[];
}

/** A subscribed plot, every default filled in. */
export interface SignalsSubscribedPlot {
	id: string;
	channels: SignalChannelSelector;
	window_s: number;
	points: number;
	arrange: SignalArrange;
}

export interface SignalsSubscribeResult {
	/** A run is acquiring now. */
	available: boolean;
	/** The rate the station programmed; null with no run. */
	fs_hz: number | null;
	/** The longest window the run holds; null with no run. */
	max_window_s: number | null;
	rate_hz: number;
	/** The run's channels in table order; empty with no run. */
	channels: SignalChannel[];
	plots: SignalsSubscribedPlot[];
}

/** One plot's data: its channels as resolved, the window drawn, and its views. */
export interface SignalsDataPlot {
	id: string;
	/** The rows it resolved to, in table order. */
	channels: number[];
	/** The window drawn, seconds: as asked, or the longest the run holds. */
	window_s: number;
	/** The station frame the window starts at. */
	first_frame: number;
	views: SpectrumView[];
}

/** `signals.data`: a binary frame to one subscribed connection. */
export interface SignalsData {
	/** False on the one frame sent when the run has ended. */
	available: boolean;
	fs_hz: number;
	/** Station time of the newest frame read, seconds. */
	t_s: number;
	/** Station frames read so far. */
	frames: number;
	max_window_s: number;
	/** Frames a window lost to the run overwriting them while they were copied. */
	lost_frames: number;
	channels: SignalChannel[];
	plots: SignalsDataPlot[];
}
