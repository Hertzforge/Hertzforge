/**
 * The channel table's home and the restored state (PROTOCOL.md, "Restored Tables" and "The
 * Channel Table's Home"), in words the screen shows. The project holds the table; the control
 * service holds the one a run arms with. A table the service kept across a restart, or took from
 * the project or a file, is restored until the operator applies it, and no run arms on it.
 */

import type { ChannelRow, RestoredTable, TableSource } from '$lib/protocol/types';
import { tableChanges } from './table';

/** Where a restored table came from, as a sentence's object: "the project table "Plate"". */
export function sourceWords(r: Pick<RestoredTable, 'source' | 'name'>): string {
	if (r.source === 'kept') return 'the table the control service kept across its restart';
	if (r.source === 'project') return `the project table "${r.name}"`;
	return `the file "${r.name}"`;
}

/** The restored state in one sentence, source and time included. */
export function restoredWords(r: RestoredTable): string {
	return `Restored from ${sourceWords(r)} at ${r.at}, and not applied: no run arms on it until it is applied.`;
}

/** One row as the apply review shows it: every number a run arms with, by channel. */
export interface ApplyRow {
	channel: number;
	enabled: boolean;
	description: string;
	role: string;
	unit: string;
	sensitivity: string;
	range: string;
	polarity: string;
	abort: string;
}

const num = (v: number | null | undefined) => (v === null || v === undefined || !Number.isFinite(v) ? 'none' : String(v));

/** The rows the operator reviews before applying a restored table: every row, on or off. */
export function applyRows(rows: ChannelRow[]): ApplyRow[] {
	return rows.map((r) => {
		const limits = [
			r.abort.accel_g !== null ? `${r.abort.accel_g} g` : null,
			r.abort.velocity_ips !== null ? `${r.abort.velocity_ips} in/s` : null,
			r.abort.displacement_mil !== null ? `${r.abort.displacement_mil} mil` : null
		].filter((x) => x !== null);
		return {
			channel: r.index,
			enabled: r.enabled,
			description: r.description,
			role: r.role,
			unit: r.units,
			sensitivity: `${num(r.sensitivity_mv_per_eu)} mV/${r.units}`,
			range: `${num(r.range_max)} ${r.units}`,
			polarity: r.polarity,
			abort: limits.length ? limits.join(', ') : 'none'
		};
	});
}

/** What taking `incoming` in place of `present` would change, one line per channel. */
export function whatChanges(present: ChannelRow[] | null, incoming: ChannelRow[]): string[] {
	if (!present) return incoming.map((r) => `Channel ${r.index} added (${r.description || r.role}).`);
	return tableChanges(present, incoming);
}

/** The source a draft carries when it was read from a file at `path`. */
export function fileSource(path: string): TableSource {
	return { kind: 'file', name: path.split(/[\\/]/).filter(Boolean).pop() ?? path };
}
