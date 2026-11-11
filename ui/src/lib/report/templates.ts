/**
 * The report templates the Report module offers, and the one each module makes by default:
 * Generate Report starts from the template of the module the person came from.
 */
import type { Workspace } from '$lib/protocol/types';

/** `[template, label]`, in the order the list shows them. */
export const TEMPLATES = [
	['modal', 'Modal test'],
	['random', 'Random vibration'],
	['shock', 'Shock'],
	['transient', 'Transient'],
	['sine', 'Sine sweep'],
	['sysid', 'System identification'],
	['mimo', 'MIMO random'],
	['model', 'Model'],
	['specify', 'Specification'],
	['signals', 'Signals'],
	['empty', 'Empty']
] as const;

export type Template = (typeof TEMPLATES)[number][0];

/** Each analysis module's own template. */
const BY_MODULE: Partial<Record<Workspace, Template>> = {
	modal: 'modal',
	random: 'random',
	shock: 'shock',
	transient: 'transient',
	sine: 'sine',
	sysid: 'sysid',
	geometry: 'model',
	specify: 'specify',
	signals: 'signals',
	simulate: 'random'
};

/** A test's own template, by the test screen it ran on. */
const BY_TEST: Record<string, Template> = {
	random: 'random',
	mimo_random: 'mimo',
	shock: 'shock',
	srs: 'shock',
	sine: 'sine',
	twr: 'transient'
};

/** The template Generate Report starts from, after `module` (and, from Acquire & Control, its `test`). */
export function defaultTemplate(module: Workspace | null, test: string | null = null): Template {
	if (module === 'acquire') return (test && BY_TEST[test]) || 'modal';
	return (module && BY_MODULE[module]) || 'modal';
}
