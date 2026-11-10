/**
 * The channel table as the operator edits it: new rows, the table sized from the selected
 * station's channels, edits to one row or many, binding rows to the selected devices' channels,
 * where each finding belongs, and what changed between two revisions. Pure functions.
 */

import type { ChannelRole, ChannelRow, NiDevice, ReportItem, StationList, ValidationReport } from '$lib/protocol/types';

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

/** Findings that only inform: nothing to fix (a drive row's sensitivity and range are advisory). */
const ADVISORY = new Set<ReportItem['reason']>(['drive_channel_present']);

export interface RowFinding {
	level: 'error' | 'warning' | 'note';
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
	const push = (it: ReportItem, given: 'error' | 'warning') => {
		const level = given === 'warning' && ADVISORY.has(it.reason) ? 'note' : given;
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

// =============================================================================================
// The table sized from the hardware

/** The station the table is sized from: the simulator's channel counts, or the scanned channels. */
export type Hardware = { kind: 'sim'; inputs: number; outputs: number } | { kind: 'ni'; inputs: ChannelChoice[]; outputs: ChannelChoice[] };

/** What the hardware is, as one string: two stations with the same key size a table alike. */
export function hardwareKey(hw: Hardware | null): string | null {
	if (!hw) return null;
	if (hw.kind === 'sim') return `sim ${hw.inputs} ${hw.outputs}`;
	return `ni ${hw.inputs.map((c) => c.key).join(',')} ${hw.outputs.map((c) => c.key).join(',')}`;
}

/** The selected station's channels, or null before the service has said which is selected. */
export function hardwareOf(stations: StationList | null): Hardware | null {
	const sel = stations?.selected;
	if (!stations || !sel) return null;
	if (sel.kind === 'sim') return { kind: 'sim', inputs: sel.inputs, outputs: sel.outputs };
	const devices = stations.stations[1]?.devices ?? [];
	const flat = (dir: 'input' | 'output') => channelGroups(devices, sel.devices, dir).flatMap((g) => g.channels);
	return { kind: 'ni', inputs: flat('input'), outputs: flat('output') };
}

/**
 * Where a row stands against the hardware: `present` on one of its channels; `missing`, bound to
 * a channel the station no longer has; `unbound`, on NI hardware with no channel; `extra`, a second
 * row on one channel. On the simulator, which binds nothing, every row is present.
 */
export type RowStatus = 'present' | 'missing' | 'unbound' | 'extra';

export interface SizedRow {
	row: ChannelRow;
	status: RowStatus;
	/** Not in the table yet: a channel of the hardware no row is on, shown off at its defaults. */
	virtual: boolean;
	/** The direction the hardware fixes for the row, or null where it fixes none (the simulator's rows, and rows off the hardware). */
	direction: 'input' | 'output' | null;
}

/** A hardware channel's row before anyone has set it: off, at a new row's defaults. */
export function hardwareRow(index: number, direction: 'input' | 'output', channel: ChannelChoice | null): ChannelRow {
	const drive = direction === 'output';
	return {
		...newRow(index, drive ? 'drive' : 'monitor'),
		enabled: false,
		description: '',
		physical: channel ? { device: channel.device, channel: channel.channel } : null
	};
}

const isDrive = (r: ChannelRow) => r.role === 'drive';

/**
 * The table as shown: the rows it has, each placed against the hardware, then a row, off, for
 * every channel of the hardware no row is on. On NI hardware a row is on the channel it is bound
 * to. The simulator binds nothing and takes any role on any row (a test of several drives runs
 * on it), so its inputs and outputs are counted: the table has at least as many input rows and
 * drive rows as it has inputs and outputs. Rows the hardware no longer has stay, marked, until
 * removed.
 */
export function sizeTable(rows: ChannelRow[], hw: Hardware | null): SizedRow[] {
	const out: SizedRow[] = [];
	if (!hw) return rows.map((row) => ({ row, status: 'present', virtual: false, direction: null }));
	let next = nextIndex(rows);
	if (hw.kind === 'sim') {
		const outs = rows.filter(isDrive).length;
		const ins = rows.length - outs;
		for (const row of rows) out.push({ row, status: 'present', virtual: false, direction: null });
		for (let k = ins; k < hw.inputs; k++) out.push({ row: hardwareRow(next++, 'input', null), status: 'present', virtual: true, direction: null });
		for (let k = outs; k < hw.outputs; k++) out.push({ row: hardwareRow(next++, 'output', null), status: 'present', virtual: true, direction: null });
		return out;
	}
	const direction = new Map<string, 'input' | 'output'>([...hw.inputs.map((c) => [c.key, 'input'] as const), ...hw.outputs.map((c) => [c.key, 'output'] as const)]);
	const claimed = new Set<string>();
	for (const row of rows) {
		const key = physicalKey(row);
		if (key === null) out.push({ row, status: 'unbound', virtual: false, direction: null });
		else if (!direction.has(key)) out.push({ row, status: 'missing', virtual: false, direction: null });
		else if (claimed.has(key)) out.push({ row, status: 'extra', virtual: false, direction: null });
		else {
			claimed.add(key);
			out.push({ row, status: 'present', virtual: false, direction: direction.get(key)! });
		}
	}
	for (const [dir, list] of [['input', hw.inputs], ['output', hw.outputs]] as const) {
		for (const c of list) if (!claimed.has(c.key)) out.push({ row: hardwareRow(next++, dir, c), status: 'present', virtual: true, direction: dir });
	}
	return out;
}

/** Whether a row is off and as a hardware row starts, its binding apart: nothing anyone set would be lost with it. */
export function untouched(row: ChannelRow): boolean {
	const fresh = hardwareRow(row.index, isDrive(row) ? 'output' : 'input', null);
	return !row.enabled && sameRows([{ ...row, physical: null }], [fresh]);
}

/**
 * The table fitted to a station it was not made on, as when the station changes or a rescan finds
 * other channels. On NI hardware, rows bound to a channel it has keep it; every unbound row, in
 * table order, takes the next free channel of its direction (an input the next free input, a drive
 * the next free output) with all its settings, except a row that is off and untouched, which goes;
 * an unbound row left with no free channel stays, to be shown as not bound. Rows bound to hardware
 * the station does not have stay, to be shown missing. On the simulator, which binds nothing,
 * rows keep their bindings and its inputs and outputs are counted in table order: a row past its
 * count that is off and untouched goes, and every other row stays.
 */
export function fitToHardware(rows: ChannelRow[], hw: Hardware | null): ChannelRow[] {
	if (!hw) return rows;
	if (hw.kind === 'sim') {
		let ins = 0;
		let outs = 0;
		return rows.filter((row) => {
			const drive = isDrive(row);
			if ((drive ? outs >= hw.outputs : ins >= hw.inputs) && untouched(row)) return false;
			if (drive) outs++;
			else ins++;
			return true;
		});
	}
	const present = new Set([...hw.inputs, ...hw.outputs].map((c) => c.key));
	const claimed = new Set(rows.map(physicalKey).filter((k): k is string => k !== null && present.has(k)));
	const free = { input: hw.inputs.filter((c) => !claimed.has(c.key)), output: hw.outputs.filter((c) => !claimed.has(c.key)) };
	const out: ChannelRow[] = [];
	for (const row of rows) {
		if (row.physical) out.push(row);
		else if (untouched(row)) continue;
		else {
			const next = free[isDrive(row) ? 'output' : 'input'].shift();
			out.push(next ? { ...structuredClone(row), physical: { device: next.device, channel: next.channel } } : row);
		}
	}
	return out;
}

/** The rows of a sized table as the table holds them, the hardware's own rows among them. */
export const materialize = (sized: SizedRow[]): ChannelRow[] => sized.map((s) => structuredClone(s.row));

/** The roles a row may take: an input's, an output's, or any where the hardware fixes neither. */
export function rolesFor(direction: SizedRow['direction']): ChannelRole[] {
	if (direction === 'input') return ['control', 'monitor', 'reference'];
	if (direction === 'output') return ['drive'];
	return ['control', 'monitor', 'reference', 'drive'];
}

/** One field of a row as the table edits it. */
export type RowEdit =
	| { field: 'index'; value: number }
	| { field: 'enabled'; value: boolean }
	| { field: 'description' | 'units'; value: string }
	| { field: 'role'; value: ChannelRole }
	| { field: 'iepe'; value: boolean }
	| { field: 'sensitivity_mv_per_eu' | 'range_max'; value: number }
	| { field: 'iepe_bias_v' | 'control_group'; value: number | null }
	| { field: 'polarity'; value: 'positive' | 'negative' }
	| { field: 'abort_accel_g' | 'abort_velocity_ips' | 'abort_displacement_mil'; value: number | null };

/** Whether a row can take an edit: a role its hardware allows, and no IEPE or abort limit on an output. */
export function canTake(s: SizedRow, edit: RowEdit): boolean {
	const output = s.direction === 'output' || isDrive(s.row);
	switch (edit.field) {
		case 'role':
			return rolesFor(s.direction).includes(edit.value);
		case 'iepe':
		case 'iepe_bias_v':
		case 'abort_accel_g':
		case 'abort_velocity_ips':
		case 'abort_displacement_mil':
			return !output || (edit.field === 'iepe' && !edit.value);
		default:
			return true;
	}
}

/** Apply an edit to one row, in place, with what it brings: an output carries no IEPE, and a row that changes direction loses its binding. */
export function applyEdit(row: ChannelRow, edit: RowEdit): void {
	switch (edit.field) {
		case 'role': {
			const was = isDrive(row);
			row.role = edit.value;
			if (edit.value === 'drive') {
				row.interface = 'single_ended';
				row.iepe_bias_v = null;
			}
			if (was !== isDrive(row)) row.physical = null;
			return;
		}
		case 'iepe':
			row.interface = edit.value ? 'iepe' : 'single_ended';
			if (!edit.value) row.iepe_bias_v = null;
			return;
		case 'abort_accel_g':
			row.abort.accel_g = edit.value;
			return;
		case 'abort_velocity_ips':
			row.abort.velocity_ips = edit.value;
			return;
		case 'abort_displacement_mil':
			row.abort.displacement_mil = edit.value;
			return;
		default:
			(row as unknown as Record<string, unknown>)[edit.field] = edit.value;
	}
}

/**
 * An edit to the rows at `positions` of the sized table, and to nothing else. Rows that cannot
 * take it (an output asked to be a control, an IEPE switched on for an output) are left and
 * counted. When the edit reaches a hardware row the table does not hold yet, every such row is
 * added first, so the numbers shown stay the numbers kept. A channel number is set on one row
 * only.
 */
export function bulkEdit(rows: ChannelRow[], hw: Hardware | null, positions: number[], edit: RowEdit): { rows: ChannelRow[]; changed: number; skipped: number } {
	const sized = sizeTable(rows, hw);
	const reach = (edit.field === 'index' ? positions.slice(0, 1) : positions).filter((p) => p >= 0 && p < sized.length);
	const out = reach.some((p) => sized[p].virtual) ? materialize(sized) : rows.map((r) => structuredClone(r));
	let changed = 0;
	let skipped = 0;
	for (const p of reach) {
		if (!canTake(sized[p], edit)) {
			skipped++;
			continue;
		}
		applyEdit(out[p], edit);
		changed++;
	}
	return { rows: out, changed, skipped };
}

/** The channels a row that is not on its hardware can be moved onto: its direction's channels whose row, if any, is off. */
export function freeChannels(sized: SizedRow[], hw: Hardware | null, position: number): ChannelChoice[] {
	if (!hw || hw.kind !== 'ni') return [];
	const row = sized[position]?.row;
	if (!row) return [];
	const on = new Set(sized.filter((s) => s.status === 'present' && s.row.enabled).map((s) => physicalKey(s.row)));
	return (isDrive(row) ? hw.outputs : hw.inputs).filter((c) => !on.has(c.key));
}

/**
 * Move a row onto a hardware channel, settings and all: the row that was there, which is off, goes,
 * and the hardware's other rows are added to the table so their numbers hold.
 */
export function moveOnto(rows: ChannelRow[], hw: Hardware | null, position: number, key: string): ChannelRow[] {
	const sized = sizeTable(rows, hw);
	const table = materialize(sized);
	const moving = table[position];
	if (!moving) return rows;
	const [device, channel] = [key.slice(0, key.indexOf('/')), key.slice(key.indexOf('/') + 1)];
	moving.physical = { device, channel };
	return table.filter((r, k) => k === position || physicalKey(r) !== key || r.enabled);
}

/** What a sized table's status reads as, in a word. */
export const STATUS_WORDS: Record<Exclude<RowStatus, 'present'>, string> = { missing: 'Missing', unbound: 'Not bound', extra: 'Extra' };
