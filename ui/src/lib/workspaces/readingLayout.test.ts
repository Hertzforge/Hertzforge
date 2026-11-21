/**
 * The reading pane's grid: a reading with a 3-D view and several tables stacks each at its own
 * height and scrolls. With auto rows in a frame of fixed height, a narrow pane (under the 900px
 * container query) shrank every table to a sliver and their rows ran over one another.
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const SOURCE = readFileSync(fileURLToPath(new URL('./Analyze.svelte', import.meta.url)), 'utf-8');
const STYLE = SOURCE.slice(SOURCE.indexOf('<style>'), SOURCE.indexOf('</style>'));

/** The declarations of the rules outside every at-rule whose selector is exactly `selector`. */
function topLevel(selector: string): Record<string, string> {
	const found: Record<string, string> = {};
	let depth = 0;
	let start = 0;
	for (let i = 0; i < STYLE.length; i++) {
		const c = STYLE[i];
		if (c === '{') {
			const head = STYLE.slice(start, i).replace(/\/\*[\s\S]*?\*\//g, '').trim();
			if (depth === 0 && !head.startsWith('@') && head.split(',').map((s) => s.trim()).includes(selector)) {
				const body = STYLE.slice(i + 1, STYLE.indexOf('}', i));
				for (const decl of body.split(';')) {
					const [name, ...value] = decl.split(':');
					if (value.length) found[name.trim()] = value.join(':').trim();
				}
			}
			depth++;
			start = i + 1;
		} else if (c === '}') {
			depth--;
			start = i + 1;
		}
	}
	return found;
}

describe("the reading's views", () => {
	it('each take their own height at every width, and the reading scrolls', () => {
		const grid = topLevel('.svviews.several:not(.panes)');
		expect(grid['grid-auto-rows']).toBe('max-content');
		expect(grid['overflow-y']).toBe('auto');
	});

	it('give the 3-D view a height of its own when the rows size to their content', () => {
		expect(topLevel('.svviews.several:not(.panes) .view.shape3d')['min-height']).toBe('440px');
	});
});
