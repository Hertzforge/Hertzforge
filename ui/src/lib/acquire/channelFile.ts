/**
 * The control channel table as a channel table file. One table serves a test and its analysis:
 * the file holds the analysis columns (node, direction, sensor, calibration and the rest) and the
 * controller's settings, and a row of the control table is a row of it. The columns the control
 * table edits are mapped here; every other column a file brought is kept for its channel and
 * written back unchanged, so a table opened and saved loses nothing. Pure functions.
 */

import type { ChannelRole, ChannelRow } from '$lib/protocol/types';
import type { TableCell, TableData } from '$lib/tables/tableFiles';

/** One channel's cells as a file holds them, by column. */
export type ChannelCells = Record<string, TableCell>;

/** The columns of each channel a file brought, by channel number: what the control table does not edit is kept here. */
export type ChannelExtras = Record<number, ChannelCells>;

/** The columns a control row is written to, in the order a new file gets them. */
export const CONTROL_COLUMNS = [
	'channel',
	'enabled',
	'comment',
	'role',
	'control',
	'drive',
	'physical',
	'iepe',
	'iepe_bias',
	'unit',
	'sensitivity',
	'range_max',
	'polarity',
	'control_group',
	'abort_accel_g',
	'abort_velocity_ips',
	'abort_displacement_mil'
] as const;

/** A number cell's value, or null for none. NaN is none: JSON has no NaN. */
const num = (v: number | null) => (v === null || !Number.isFinite(v) ? null : v);

/**
 * The cells a control row writes. A channel's role in the loop is three of the table's columns:
 * a drive is `drive`, a control channel is `control`, a reference is a `role` of reference, and a
 * monitor is none of them. The table's own role of each channel (what it is to an FRF) is kept
 * where the loop's role allows; a monitor is never a reference. When the row's unit is no longer
 * the one the file had, the channel type is left for the table to work out from the new unit.
 */
export function cellsOf(row: ChannelRow, kept: ChannelCells = {}): ChannelCells {
	const role = row.role;
	const tableRole = typeof kept.role === 'string' ? kept.role : '';
	const cells: ChannelCells = {
		...kept,
		channel: row.index,
		enabled: row.enabled,
		comment: row.description,
		role: role === 'reference' ? 'reference' : role === 'monitor' && tableRole === 'reference' ? '' : tableRole,
		control: role === 'control',
		drive: role === 'drive',
		physical: row.physical ? `${row.physical.device}/${row.physical.channel}` : '',
		iepe: row.interface === 'iepe',
		iepe_bias: num(row.iepe_bias_v),
		unit: row.units,
		sensitivity: num(row.sensitivity_mv_per_eu),
		range_max: num(row.range_max),
		polarity: row.polarity,
		control_group: row.control_group,
		abort_accel_g: num(row.abort.accel_g),
		abort_velocity_ips: num(row.abort.velocity_ips),
		abort_displacement_mil: num(row.abort.displacement_mil)
	};
	if ('unit' in kept && kept.unit !== row.units && 'channel_type' in kept) cells.channel_type = '';
	return cells;
}

/** The loop's role a file's row gives: `drive`, then `control`, then a `role` of reference, else a monitor. */
export function roleOf(cells: ChannelCells): ChannelRole {
	if (cells.drive === true) return 'drive';
	if (cells.control === true) return 'control';
	if (cells.role === 'reference') return 'reference';
	return 'monitor';
}

const numberOr = (v: TableCell | undefined, fallback: number) => (typeof v === 'number' && Number.isFinite(v) ? v : fallback);
const numberOrNull = (v: TableCell | undefined) => (typeof v === 'number' && Number.isFinite(v) ? v : null);

/** A control row from a file's row; what the file does not say takes a new row's value. */
export function rowOf(cells: ChannelCells): ChannelRow {
	const role = roleOf(cells);
	const drive = role === 'drive';
	const physical = typeof cells.physical === 'string' ? cells.physical.trim() : '';
	const slash = physical.indexOf('/');
	const unit = typeof cells.unit === 'string' && cells.unit.trim() ? cells.unit : drive ? 'V' : 'g';
	return {
		index: numberOr(cells.channel, NaN),
		enabled: cells.enabled === undefined || cells.enabled === null ? true : cells.enabled === true,
		description: typeof cells.comment === 'string' ? cells.comment : '',
		role,
		physical: slash > 0 && slash < physical.length - 1 ? { device: physical.slice(0, slash), channel: physical.slice(slash + 1) } : null,
		interface: cells.iepe === true ? 'iepe' : 'single_ended',
		iepe_bias_v: numberOrNull(cells.iepe_bias),
		units: unit,
		sensitivity_mv_per_eu: numberOr(cells.sensitivity, drive ? 1000 : 100),
		range_max: numberOr(cells.range_max, drive ? 10 : 100),
		polarity: cells.polarity === 'negative' ? 'negative' : 'positive',
		control_group: numberOrNull(cells.control_group),
		abort: { accel_g: numberOrNull(cells.abort_accel_g), velocity_ips: numberOrNull(cells.abort_velocity_ips), displacement_mil: numberOrNull(cells.abort_displacement_mil) }
	};
}

/**
 * The table a file is written from: every row's cells, under the columns the file had first, in
 * its order, then the control columns it lacked. A column a row does not have is null there.
 */
export function channelFile(rows: ChannelRow[], extras: ChannelExtras = {}, order: string[] = []): TableData {
	const cells = rows.map((r) => cellsOf(r, extras[r.index]));
	const columns = [...order];
	for (const name of [...CONTROL_COLUMNS, ...cells.flatMap((c) => Object.keys(c))]) if (!columns.includes(name)) columns.push(name);
	return { columns, rows: cells.map((c) => columns.map((name) => (name in c ? c[name] : null))) };
}

/** A file's table as control rows, every cell of each row kept by its channel number, and the file's column order. */
export function channelRowsFrom(table: TableData): { rows: ChannelRow[]; extras: ChannelExtras; order: string[] } {
	const extras: ChannelExtras = {};
	const rows = table.rows.map((values) => {
		const cells: ChannelCells = Object.fromEntries(table.columns.map((name, k) => [name, values[k] ?? null]));
		const row = rowOf(cells);
		if (Number.isFinite(row.index)) extras[row.index] = cells;
		return row;
	});
	return { rows, extras, order: [...table.columns] };
}

/** The kept cells after a row's channel number changes: they follow the channel. */
export function renumbered(extras: ChannelExtras, from: number, to: number): ChannelExtras {
	if (from === to || !(from in extras)) return extras;
	const out = { ...extras, [to]: extras[from] };
	delete out[from];
	return out;
}
