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

/** The one analysis module's id: the id the recordings' module had, which addresses and scripts name. */
export const ANALYSIS: Workspace = 'signals';

/**
 * The rail: control and simulation, then one analysis module, then the report. The analysis was
 * one module per kind of test until an action was hard to find from the wrong one (Brandon,
 * 2026-11-17: one module with every action until there is a reason to split it).
 */
export const WORKSPACES: WorkspaceInfo[] = [
	{ id: 'acquire', title: 'Acquire & Control', rail: 'Acquire & Control', summary: 'Closed-loop Random control on the simulator or NI hardware.' },
	{ id: 'simulate', title: 'Simulate', rail: 'Simulate', summary: 'Run the control loop against a simulated system.' },
	{ id: ANALYSIS, title: 'Analysis', rail: 'Analysis', study: true, summary: 'Everything offline: geometries and models, recordings and their spectra, specifications, and the analysis of every kind of test.' },
	{ id: 'report', title: 'Report', rail: 'Report', summary: 'Reports from every workspace.' }
];

/**
 * The ids the analysis had when it was nine modules. Operations and table rows still name them
 * as their homes, and an address may; every one of them is the analysis module now.
 */
export const STUDIES = new Set<Workspace>(['geometry', 'signals', 'specify', 'sysid', 'random', 'shock', 'transient', 'sine', 'modal']);

/** The module an id opens: the analysis module for any of its earlier ids. */
export function moduleOf(id: Workspace): Workspace {
	return STUDIES.has(id) ? ANALYSIS : id;
}

export function workspaceInfo(id: Workspace): WorkspaceInfo {
	return WORKSPACES.find((w) => w.id === moduleOf(id))!;
}

export function isWorkspace(value: string): value is Workspace {
	return WORKSPACES.some((w) => w.id === value) || STUDIES.has(value as Workspace);
}

/** Whether a module offers an operation or a table row; a row with no list is offered in every module, and the analysis module offers what any of its earlier ids did. */
export function offeredIn(op: { modules?: Workspace[] }, workspace: Workspace): boolean {
	return !op.modules || op.modules.some((m) => moduleOf(m) === moduleOf(workspace));
}
