import type { Workspace } from '$lib/protocol/types';

export interface WorkspaceInfo {
	id: Workspace;
	title: string;
	/** Rail label, two lines at most. */
	rail: string;
	/** One line for the palette and the rail's tooltip. */
	summary: string;
	/**
	 * A study: the shared host (the project tree, the selection shown, the action table) with this
	 * module's operations. The others have screens of their own.
	 */
	study?: boolean;
}

/**
 * The rail, in the order a test runs: control first, then the recordings and what each kind of
 * test reads from them, then what is built, then what is written. Each analysis is a module, so a
 * sine test
 * is not offered shock tools (Brandon, 2026-11-04).
 */
export const WORKSPACES: WorkspaceInfo[] = [
	{ id: 'acquire', title: 'Acquire & Control', rail: 'Acquire & Control', summary: 'Closed-loop Random control on the simulator or NI hardware.' },
	{ id: 'simulate', title: 'Simulate', rail: 'Simulate', summary: 'Run the control loop against a simulated system.' },
	{ id: 'signals', title: 'Signals', rail: 'Signals', study: true, summary: 'Recordings: averaging, filtering, truncation, statistics, spectra and what is read straight off them.' },
	{ id: 'sysid', title: 'System ID', rail: 'System ID', study: true, summary: 'A noise floor and an excitation read into PSDs, FRFs and coherence for a controller to use.' },
	{ id: 'random', title: 'Random', rail: 'Random', study: true, summary: 'PSDs and cross-spectral matrices against their specification, coherence, control checks.' },
	{ id: 'shock', title: 'Shock', rail: 'Shock', study: true, summary: 'Shock response spectra, pulses and synthesis.' },
	{ id: 'transient', title: 'Transient', rail: 'Transient', study: true, summary: 'A waveform replicated on a shaker: whether it fits, its waveform error, its segments.' },
	{ id: 'sine', title: 'Sine', rail: 'Sine', study: true, summary: 'Sine sweeps and dwells: extracted levels, distortion, frequency, sweep rate.' },
	{ id: 'modal', title: 'Modal', rail: 'Modal', study: true, summary: 'FRFs fitted to modes, shape sets compared, matched and projected.' },
	{ id: 'geometry', title: 'Geometry', rail: 'Geometry', study: true, summary: 'Nodes, coordinate systems, elements and solids; sensors and channel tables on them.' },
	{ id: 'model', title: 'Model', rail: 'Model', study: true, summary: 'Finite element models: element groups, meshing, solving for modes.' },
	{ id: 'specify', title: 'Specify', rail: 'Specify', study: true, summary: 'Test specifications derived, authored and predicted.' },
	{ id: 'report', title: 'Report', rail: 'Report', summary: 'Reports from every workspace.' }
];

/** The study modules, which share one host. */
export const STUDIES = new Set(WORKSPACES.filter((w) => w.study).map((w) => w.id));

export function workspaceInfo(id: Workspace): WorkspaceInfo {
	return WORKSPACES.find((w) => w.id === id)!;
}

export function isWorkspace(value: string): value is Workspace {
	return WORKSPACES.some((w) => w.id === value);
}

/** Whether a module offers an operation or a table row; a row with no list is offered in every module. */
export function offeredIn(op: { modules?: Workspace[] }, workspace: Workspace): boolean {
	return !op.modules || op.modules.includes(workspace);
}
