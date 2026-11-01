/**
 * The channel table as the operator edits it: new rows, binding rows to the selected devices'
 * channels, where each finding belongs, and what changed between two revisions. Pure functions.
 */

import type { ChannelRole, ChannelRow, NiDevice, ReportItem, ValidationReport } from '$lib/protocol/types';

/** A new row as the service makes one: single-ended (IEPE off), unbound, no abort limits. */
export function newRow(index: number, role: ChannelRole): ChannelRow {
	const drive = role === 'drive';
	return {
		index,
		enabled: true,
		description: drive ? 'Drive' : role === 'control' ? 'Control' : role === 'reference' ? 'Reference' : 'Monitor',
		role,
		physical: null,
		interface: 'single_ended',
		iepe_bias_v: null,
		units: drive ? 'V' : 'g',
		sensitivity_mv_per_eu: drive ? 1000 : 100,
		range_max: drive ? 10 : 100,
		polarity: 'positive',
		control_group: null,
		abort: { accel_g: null, velocity_ips: null, displacement_mil: null }
	};
}

export function nextIndex(rows: ChannelRow[]): number {
	return rows.reduce((m, r) => Math.max(m, Number.isFinite(r.index) ? r.index : 0), 0) + 1;
}

export interface ChannelChoice {
	device: string;
	channel: string;
	/** "cDAQ1Mod1/ai0" */
	key: string;
	label: string;
}

export interface ChannelGroup {
	device: NiDevice;
	label: string;
	channels: ChannelChoice[];
}

/** The selected devices' channels in one direction, grouped by device, in scan order. */
export function channelGroups(devices: NiDevice[], selected: string[], direction: 'input' | 'output'): ChannelGroup[] {
	return devices
		.filter((d) => selected.includes(d.id))
		.map((d) => ({
			device: d,
			label: `${d.id} (${d.product}${d.slot !== null ? `, slot ${d.slot}` : ''})`,
			channels: (direction === 'input' ? d.inputs : d.outputs).map((c) => ({ device: d.id, channel: c.id, key: `${d.id}/${c.id}`, label: c.label }))
		}))
		.filter((g) => g.channels.length > 0);
}

export const physicalKey = (r: ChannelRow): string | null => (r.physical ? `${r.physical.device}/${r.physical.channel}` : null);

/**
 * Bind every enabled, unbound row to the first free channel of its direction, in table order.
 * Rows already bound keep their channel. Returns the new rows and how many were bound.
 */
export function bindUnbound(rows: ChannelRow[], devices: NiDevice[], selected: string[]): { rows: ChannelRow[]; bound: number } {
	const used = new Set(rows.map(physicalKey).filter((k): k is string => k !== null));
	const free = (dir: 'input' | 'output') =>
		channelGroups(devices, selected, dir)
			.flatMap((g) => g.channels)
			.filter((c) => !used.has(c.key));
	const pools = { input: free('input'), output: free('output') };
	let bound = 0;
	const out = rows.map((r) => {
		if (!r.enabled || r.physical) return r;
		const pool = pools[r.role === 'drive' ? 'output' : 'input'];
		const next = pool.shift();
		if (!next) return r;
		used.add(next.key);
		bound++;
		return { ...r, physical: { device: next.device, channel: next.channel } };
	});
	return { rows: out, bound };
}

/** The table cell a finding belongs to. */
export type RowCell =
	| 'index'
	| 'role'
	| 'physical'
	| 'interface'
	| 'iepe_bias_v'
	| 'sensitivity_mv_per_eu'
	| 'range_max'
	| 'control_group'
	| 'row';

const CELL: Partial<Record<ReportItem['reason'], RowCell>> = {
	duplicate_index: 'index',
	zero_sensitivity: 'sensitivity_mv_per_eu',
	non_positive_range: 'range_max',
	channel_unbound: 'physical',
	physical_channel_unknown: 'physical',
	physical_channel_wrong_direction: 'physical',
	physical_channel_shared: 'physical',
	mixed_timing_families: 'physical',
	iepe_without_bias: 'iepe_bias_v',
	bias_on_single_ended: 'interface',
	drive_channel_present: 'role',
	control_group_gap: 'control_group'
};

export interface RowFinding {
	level: 'error' | 'warning';
	cell: RowCell;
	message: string;
	reason: ReportItem['reason'];
}

/**
 * Findings per row index, and those about the whole table. Advisory notes the operator cannot
 * act on (a drive row's sensitivity being advisory) are kept, as warnings.
 */
export function rowFindings(report: ValidationReport | null): { rows: Map<number, RowFinding[]>; table: RowFinding[] } {
	const rows = new Map<number, RowFinding[]>();
	const table: RowFinding[] = [];
	if (!report) return { rows, table };
	const push = (it: ReportItem, level: 'error' | 'warning') => {
		const f: RowFinding = { level, cell: CELL[it.reason] ?? 'row', message: it.message, reason: it.reason };
		if (it.channel === undefined || it.channel === null) table.push(f);
		else {
			const list = rows.get(it.channel) ?? [];
			list.push(f);
			rows.set(it.channel, list);
		}
	};
	report.errors.forEach((it) => push(it, 'error'));
	report.warnings.forEach((it) => push(it, 'warning'));
	return { rows, table };
}

const FIELD_WORDS: Record<keyof ChannelRow, string> = {
	index: 'number',
	enabled: 'on or off',
	description: 'description',
	role: 'role',
	physical: 'physical channel',
	interface: 'input type (IEPE)',
	iepe_bias_v: 'IEPE bias',
	units: 'units',
	sensitivity_mv_per_eu: 'sensitivity',
	range_max: 'range',
	polarity: 'polarity',
	control_group: 'control group',
	abort: 'abort limits'
};

/** What differs between two revisions of the table, in words, one line per channel. */
export function tableChanges(before: ChannelRow[], after: ChannelRow[]): string[] {
	const out: string[] = [];
	const was = new Map(before.map((r) => [r.index, r]));
	const now = new Map(after.map((r) => [r.index, r]));
	for (const r of after) {
		const old = was.get(r.index);
		if (!old) {
			out.push(`Channel ${r.index} added (${r.description || r.role}).`);
			continue;
		}
		const fields = (Object.keys(FIELD_WORDS) as (keyof ChannelRow)[]).filter((k) => JSON.stringify(old[k]) !== JSON.stringify(r[k]));
		if (fields.length) out.push(`Channel ${r.index}: ${fields.map((k) => FIELD_WORDS[k]).join(', ')} changed.`);
	}
	for (const r of before) if (!now.has(r.index)) out.push(`Channel ${r.index} removed (${r.description || r.role}).`);
	return out;
}

export function sameRows(a: ChannelRow[], b: ChannelRow[]): boolean {
	return JSON.stringify(a) === JSON.stringify(b);
}
