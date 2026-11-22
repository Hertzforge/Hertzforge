/**
 * The linear or log choice made on a 2-D plot's axes, kept by what is plotted: the kinds of the
 * objects selected and the axis's unit. A PSD read on linear frequency stays so the next time a
 * PSD is selected, and says nothing about a time history, which opens as the service draws it
 * (Brandon, 2026-11-17). Kept across launches; an axis never chosen takes the view's own scale.
 */
const KEY = 'hertzforge.plot.scales';

export interface ScaleChoice {
	x?: boolean;
	y?: boolean;
}

function read(): Record<string, ScaleChoice> {
	try {
		const kept = JSON.parse(localStorage.getItem(KEY) ?? '{}');
		return kept && typeof kept === 'object' ? kept : {};
	} catch {
		return {};
	}
}

let kept = $state.raw<Record<string, ScaleChoice>>(read());

/** What was chosen for `key`: `x` and `y` true for log, false for linear, absent when never chosen. */
export function scaleChoice(key: string): ScaleChoice {
	return kept[key] ?? {};
}

export function chooseScale(key: string, axis: 'x' | 'y', log: boolean) {
	kept = { ...kept, [key]: { ...kept[key], [axis]: log } };
	try {
		localStorage.setItem(KEY, JSON.stringify(kept));
	} catch {
		// no storage: the choice lasts as long as the window
	}
}

/** The key a plot's choices are kept under: what is plotted (`of`, the selection's kinds) and its x unit. */
export function scaleKey(of: string | undefined, xUnit: string): string {
	return `${of ?? 'plot'}|${xUnit}`;
}
