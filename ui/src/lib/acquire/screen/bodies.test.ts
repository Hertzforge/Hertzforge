/** Which body a panel draws: a test type's own for its plot type, else the plain plot body. */
import { describe, expect, it } from 'vitest';
import { newPlotSpec, type PlotType } from '../plots/types';
import { customBody, type PlotBodies, type PlotBodySnippet } from './bodies';
import type { Panel } from './types';

const T = (id: string): PlotType => ({ id, words: id, help: '', group: 'frequency', channels: false, time: false });
const plot = (quantity: string): Panel => ({ id: 'pl-1', kind: 'plot', at: { x: 0, y: 0, w: 1, h: 1 }, plot: newPlotSpec(T(quantity)) });
const survey = (() => {}) as unknown as PlotBodySnippet;

describe('custom bodies', () => {
	const bodies: PlotBodies = { survey };

	it('routes a plot type with a body of its own to that body', () => {
		expect(customBody(bodies, plot('survey'))).toBe(survey);
	});

	it('leaves every other plot type, a card, and a screen with no bodies to the plain plot body', () => {
		expect(customBody(bodies, plot('spectrum'))).toBeNull();
		expect(customBody(bodies, { id: 'cd-1', kind: 'card', at: { x: 0, y: 0, w: 1, h: 1 }, card: 'survey' })).toBeNull();
		expect(customBody(undefined, plot('survey'))).toBeNull();
		expect(customBody(bodies, plot('constructor'))).toBeNull();
	});
});
