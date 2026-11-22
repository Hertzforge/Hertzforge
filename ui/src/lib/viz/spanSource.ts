/**
 * How a zoomed plot asks for its samples again. A view whose curves the service thinned names the
 * request that drew it (`SpectrumView.span`); the pane sends that again with the x range it shows,
 * through the asker the app sets here, so the viz kit needs no connection of its own. Without one
 * (a page with no service) the plot keeps the curves it was sent.
 */
import type { DrawingSpan, SpanSource, SpectrumView } from '../protocol/types';

/** The view `source.view` of the answer to `source`'s request sent with `span`; null when there is none. */
export type SpanAsker = (source: SpanSource, span: DrawingSpan) => Promise<SpectrumView | null>;

let asker: SpanAsker | null = null;

export function setSpanAsker(next: SpanAsker | null): void {
	asker = next;
}

export function spanAsker(): SpanAsker | null {
	return asker;
}

/** The series of `answer` in place of `view`'s, when they are as many and named alike; else null. */
export function spanSeries(view: SpectrumView, answer: SpectrumView | null): SpectrumView['series'] | null {
	if (!answer || answer.view !== 'spectrum' || answer.series.length !== view.series.length) return null;
	return answer.series.every((s, i) => s.name === view.series[i].name) ? answer.series : null;
}

/** Whether `low` to `high` shows every series of `view` whole, so its own curves are what to draw. */
export function showsWhole(view: SpectrumView, low: number, high: number): boolean {
	for (const s of view.series) {
		if (!s.x.length) continue;
		if (low > s.x[0] || high < s.x[s.x.length - 1]) return false;
	}
	return true;
}
