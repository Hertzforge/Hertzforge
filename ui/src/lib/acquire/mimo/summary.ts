/**
 * The setup summary: each section's state in one line, and what must happen before a run can
 * start, in words, each item naming the section it is fixed in. Pure: the screen hands it what
 * the stores hold.
 */

import type { LawEntry, MimoReferenceResult, MimoValidation } from '$lib/protocol/mimo';
import type { SetupStatus } from '../actions';
import type { EffectiveSpace, MimoDraft, MimoSection, TestChannels } from './mimoSetup';

export interface SectionSummary {
	section: Exclude<MimoSection, 'predict'>;
	title: string;
	state: string;
	missing: string[];
	errors: number;
	warnings: number;
}

export interface SummaryInputs {
	draft: MimoDraft;
	channels: TestChannels;
	space: EffectiveSpace;
	labels: string[];
	/** The reference request's answer for the draft as it stands, or null. */
	ref: MimoReferenceResult | null;
	refBusy: boolean;
	law: LawEntry | null;
	lawsLoaded: boolean;
	/** The dry run for the setup as it stands, or null. */
	check: MimoValidation | null;
	counts: Record<MimoSection, { errors: number; warnings: number }>;
	station: { kind: 'sim' | 'ni'; dirty: boolean } | null;
	tableErrors: number;
	setupStatus: SetupStatus;
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
const fixWords = (n: number) => `Fix the ${n === 1 ? 'error' : `${n} errors`} here.`;

const METHOD_WORDS: Record<MimoDraft['reference']['method'], string> = {
	import: 'Imported',
	breakpoints: 'Breakpoint tables',
	rigid: 'Rigid body',
	field: 'From field data'
};

export function summarize(x: SummaryInputs): SectionSummary[] {
	const d = x.draft;
	const c = x.channels;
	const out: SectionSummary[] = [];

	// Channels.
	{
		const missing: string[] = [];
		if (!c.controls.length) missing.push('Give a channel the Control role.');
		if (!c.drives.length) missing.push('Give an output the Drive role.');
		if (x.space.kind === 'virtual' && x.space.from === 'operator' && !d.space.dofs.length) missing.push('Name the degrees of freedom for virtual control.');
		const n = x.counts.channels.errors;
		if (n > 0) missing.push(fixWords(n));
		const space = x.space.kind === 'physical' ? 'physical control' : `virtual control, ${plural(x.space.dofs.length, 'degree of freedom', 'degrees of freedom')}`;
		out.push({
			section: 'channels',
			title: 'Channels',
			state: `${plural(c.controls.length, 'control', 'controls')}, ${plural(c.drives.length, 'drive', 'drives')}${c.limits.length ? `, ${plural(c.limits.length, 'limit', 'limits')}` : ''}; ${space}`,
			missing,
			errors: n,
			warnings: x.counts.channels.warnings
		});
	}

	// Reference.
	{
		const missing: string[] = [];
		const r = x.ref;
		const method = d.reference.method;
		let state = METHOD_WORDS[method];
		if (x.refBusy) state += ', being built';
		else if (r?.ok && r.reference) state += `: ${r.labels.length} by ${r.labels.length} on ${r.lines} lines`;
		else if (r && !r.ok) state += ', not accepted';
		else state += ', not built yet';
		if (method === 'import' && !d.reference.importPath.trim()) missing.push('Give the path of the reference to import.');
		else if (method === 'field') missing.push('Build the reference from the records, then import it.');
		else if (method === 'import' && !r && !x.refBusy) missing.push('Import the reference.');
		else if (r && !r.ok && r.psd.violations > 0 && r.psd.repair && !r.psd.repair_accepted) missing.push('Accept the repair, or change the pairs that cannot hold.');
		if (method === 'rigid' && !x.space.response) missing.push('Give the sensor geometry; the fit sets the six degrees of freedom.');
		if (r?.ok && r.labels.length && r.labels.length !== x.labels.length) missing.push(`The reference has ${r.labels.length} rows and the test controls ${x.labels.length}; they must match.`);
		const n = x.counts.reference.errors;
		if (n > 0 && !missing.length) missing.push(fixWords(n));
		out.push({ section: 'reference', title: 'Reference', state, missing, errors: n, warnings: x.counts.reference.warnings });
	}

	// Control.
	{
		const missing: string[] = [];
		const law = x.law;
		if (!x.lawsLoaded) missing.push('The law catalog is read from the control service once it is connected.');
		else if (!law) missing.push('Choose a control law.');
		if (x.check?.law && !x.check.law.admitted) missing.push('The chosen law does not admit this test; choose another, or change its parameters.');
		const n = x.counts.control.errors;
		if (n > 0) missing.push(fixWords(n));
		out.push({
			section: 'control',
			title: 'Control',
			state: law ? `${law.name} ${law.version}; FRF ${d.frf === 'hold' ? 'held from identification' : 'updated each loop'}` : 'No law chosen',
			missing,
			errors: n,
			warnings: x.counts.control.warnings
		});
	}

	// Schedule.
	{
		const s = d.schedule;
		const n = x.counts.schedule.errors;
		const full = s.levels[s.levels.length - 1];
		out.push({
			section: 'schedule',
			title: 'Schedule',
			state: `${plural(s.levels.length, 'level', 'levels')}, ${s.mode === 'auto' ? 'automatic' : 'manual'}; full level ${full ? `${full.db} dB` : '-'}${s.mode === 'auto' ? ` for ${s.full_level_s} s` : ''}`,
			missing: n > 0 ? [fixWords(n)] : [],
			errors: n,
			warnings: x.counts.schedule.warnings
		});
	}

	// Backend.
	{
		const missing: string[] = [];
		if (!x.station) missing.push('The station is read from the control service once it is connected.');
		else if (x.station.dirty) missing.push('Use the chosen station, or put the choice back.');
		if (x.tableErrors > 0) missing.push(`Fix the ${x.tableErrors === 1 ? 'error' : `${x.tableErrors} errors`} in the channel wiring.`);
		out.push({
			section: 'backend',
			title: 'Backend',
			state: x.station ? (x.station.kind === 'sim' ? 'Simulator' : 'NI hardware') : 'Not connected',
			missing,
			errors: x.counts.backend.errors,
			warnings: x.counts.backend.warnings
		});
	}
	return out;
}

/** Every item that must happen before a run can start, in section order. */
export function missingList(sections: SectionSummary[]): { section: SectionSummary['section']; title: string; text: string }[] {
	return sections.flatMap((s) => s.missing.map((text) => ({ section: s.section, title: s.title, text })));
}
