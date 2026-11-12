/**
 * The verdict grid: controls down, pulses across, one cell per control per pulse with its verdict,
 * its reasons by name and its signed error. Built from the service's pulse rows only; a control
 * the run did not judge on a pulse has an empty cell. The grid shows the latest pulses when there
 * are more than fit.
 */

import type { MimoShockFailReason, MimoShockPulseRow } from '$lib/protocol/mimoShock';
import { FAIL_WORDS, signedDb } from './msWords';

export interface GridCell {
	pulse: number;
	channel: number;
	passed: boolean;
	reasons: MimoShockFailReason[];
	over_test_db: number;
	/** What the cell shows: "Pass", or the reasons' short words. */
	text: string;
	/** What the cell says on hover and to a screen reader. */
	label: string;
	/** Over its scaled abort line on this pulse. */
	scaledOver: boolean;
}

export interface GridColumn {
	index: number;
	level_db: number;
	passed: boolean;
}

export interface VerdictGrid {
	controls: number[];
	columns: GridColumn[];
	/** `cells[c][p]`: control `c` (in `controls`' order) on column `p`, or null. */
	cells: (GridCell | null)[][];
	/** Pulses left out on the left. */
	hidden: number;
}

export const GRID_COLUMNS = 24;

export function verdictGrid(rows: readonly MimoShockPulseRow[], controls: readonly number[], max = GRID_COLUMNS): VerdictGrid {
	const shown = rows.slice(Math.max(0, rows.length - max));
	const chans = controls.length ? [...controls] : [...new Set(rows.flatMap((r) => r.controls.map((c) => c.channel)))];
	const cells = chans.map((ch) =>
		shown.map((r): GridCell | null => {
			const c = r.controls.find((x) => x.channel === ch);
			if (!c) return null;
			const scaledOver = c.scaled_margin_db > 0;
			const why = c.reasons.map((x) => FAIL_WORDS[x]?.word ?? x).join(', ');
			return {
				pulse: r.index,
				channel: ch,
				passed: c.passed,
				reasons: [...c.reasons],
				over_test_db: c.over_test_db,
				text: c.passed ? 'Pass' : c.reasons.map((x) => FAIL_WORDS[x]?.short ?? x).join(' ') || 'Fail',
				label: `Control ${ch}, pulse ${r.index + 1}: ${c.passed ? 'pass' : `fail${why ? ` (${why})` : ''}`}, ${signedDb(c.over_test_db)}${scaledOver ? ', over its scaled abort line' : ''}`,
				scaledOver
			};
		})
	);
	return { controls: chans, columns: shown.map((r) => ({ index: r.index, level_db: r.level_db, passed: r.passed })), cells, hidden: rows.length - shown.length };
}
