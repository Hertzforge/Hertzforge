import type { Workspace } from '$lib/protocol/types';

export interface WorkspaceInfo {
	id: Workspace;
	title: string;
	/** Rail label, two lines at most. */
	rail: string;
	live: boolean;
	/** One line for the palette and the placeholder. */
	summary: string;
	/** What moves here, for workspaces still to come. */
	moves?: string[];
}

export const WORKSPACES: WorkspaceInfo[] = [
	{
		id: 'acquire',
		title: 'Acquire & Control',
		rail: 'Acquire & Control',
		live: false,
		summary: 'Closed-loop test control and recording.',
		moves: ['Random', 'Sine', 'Shock', 'SRS', 'TWR', 'Recorder']
	},
	{ id: 'analyze', title: 'Analyze', rail: 'Analyze', live: true, summary: 'Run analysis tools on recorded data.' },
	{ id: 'modal', title: 'Modal', rail: 'Modal', live: true, summary: 'Natural frequencies, damping and mode shapes.' },
	{
		id: 'model',
		title: 'Model',
		rail: 'Model',
		live: false,
		summary: 'Finite element models of the test article and fixtures.',
		moves: ['Meshing', 'Modal solver', 'Frequency response solver', 'Model to test correlation']
	},
	{
		id: 'specify',
		title: 'Specify',
		rail: 'Specify',
		live: false,
		summary: 'Environmental test specification tools.',
		moves: ['Profile authoring', 'Enveloping and derivation', 'Fatigue damage equivalence', 'Specification library']
	},
	{ id: 'simulate', title: 'Simulate', rail: 'Simulate', live: true, summary: 'Run the control loop against a simulated system.' },
	{
		id: 'report',
		title: 'Report',
		rail: 'Report',
		live: false,
		summary: 'Reports from every workspace.',
		moves: ['Test reports', 'Analysis reports', 'Modal survey reports', 'Templates and export']
	}
];

export function workspaceInfo(id: Workspace): WorkspaceInfo {
	return WORKSPACES.find((w) => w.id === id)!;
}

export function isWorkspace(value: string): value is Workspace {
	return WORKSPACES.some((w) => w.id === value);
}
