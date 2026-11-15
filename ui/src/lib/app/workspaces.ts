import type { Workspace } from '$lib/protocol/types';

export interface WorkspaceInfo {
	id: Workspace;
	title: string;
	/** Rail label, two lines at most. */
	rail: string;
	/** The rail's icon, when it is not the one named for the module's id. */
	icon?: 'model';
	/** One line for the palette and the rail's tooltip. */
	summary: string;
	/**
	 * A study: the shared host (the project tree, the selection shown, the action table) with this
	 * module's operations. The others have screens of their own.
	 */
	study?: boolean;
}

/**
 * The rail, in Brandon's order (2026-11-13): control and simulation first, then the model and the
 * signal processing and specifications that every test shares, then one analysis module per kind
 * of test, so a sine test is not offered shock tools (2026-11-04), and the report last.
 */
export const WORKSPACES: WorkspaceInfo[] = [
	{ id: 'acquire', title: 'Acquire & Control', rail: 'Acquire & Control', summary: 'Closed-loop Random control on the simulator or NI hardware.' },
	{ id: 'simulate', title: 'Simulate', rail: 'Simulate', summary: 'Run the control loop against a simulated system.' },
	// shown as Model, its id still geometry, which projects, the service and scripts name
	// (Brandon, 2026-11-10: the module holds a geometry and its finite element model)
	{ id: 'geometry', title: 'Model', rail: 'Model', icon: 'model', study: true, summary: 'A geometry and its finite element model: nodes, coordinate systems, solids, meshing, properties and modes; sensors and channel tables on them.' },
	{ id: 'signals', title: 'Signal Processing', rail: 'Signal Processing', study: true, summary: 'Recordings: averaging, filtering, truncation, statistics, spectra and what is read straight off them.' },
	{ id: 'specify', title: 'Specifications', rail: 'Specifications', study: true, summary: 'Test specifications derived, authored and predicted.' },
	{ id: 'sysid', title: 'System ID Analysis', rail: 'System ID Analysis', study: true, summary: 'A noise floor and an excitation read into PSDs, FRFs and coherence for a controller to use.' },
	{ id: 'random', title: 'Random Analysis', rail: 'Random Analysis', study: true, summary: 'PSDs and cross-spectral matrices against their specification, coherence, control checks.' },
	{ id: 'shock', title: 'Shock Analysis', rail: 'Shock Analysis', study: true, summary: 'Shock response spectra, pulses and synthesis.' },
	{ id: 'transient', title: 'Transient Analysis', rail: 'Transient Analysis', study: true, summary: 'A waveform replicated on a shaker: whether it fits, its waveform error, its segments.' },
	{ id: 'sine', title: 'Sine Analysis', rail: 'Sine Analysis', study: true, summary: 'Sine sweeps and dwells: extracted levels, distortion, frequency, sweep rate.' },
	{ id: 'modal', title: 'Modal Analysis', rail: 'Modal Analysis', study: true, summary: 'FRFs fitted to modes, shape sets compared, matched and projected.' },
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
