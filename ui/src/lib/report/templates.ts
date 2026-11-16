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
	['mixed_mode', 'Mixed Mode'],
	['explorer', 'Explorer run'],
	['recorder', 'Recording'],
	['mimo_shock', 'MIMO shock'],
	['run', 'The recorded run\'s test'],
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
	twr: 'transient',
	mixed: 'mixed_mode',
	explorer: 'explorer',
	recorder: 'recorder',
	mimo_shock: 'mimo_shock',
	// a Playback run is its time histories, as the analysis side's Signals reports them
	playback: 'signals'
};

/** The modules whose report is not a test's: a model, a specification, signals. */
const NOT_A_TEST: ReadonlySet<Workspace> = new Set<Workspace>(['geometry', 'specify', 'signals']);

/**
 * The template Generate Report starts from, after `module` (and, from Acquire & Control, its
 * `test`). A project holding a run record (`recorded`) starts from `run`, the template of the
 * test the record is a run of, which the analysis service picks; except from a module whose
 * report is not a test's, which keeps its own.
 */
export function defaultTemplate(module: Workspace | null, test: string | null = null, recorded = false): Template {
	if (recorded && !(module && NOT_A_TEST.has(module))) return 'run';
	if (module === 'acquire') return (test && BY_TEST[test]) || 'modal';
	return (module && BY_MODULE[module]) || 'modal';
}
