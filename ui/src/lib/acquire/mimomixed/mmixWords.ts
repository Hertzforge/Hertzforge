/**
 * Plain words for a MIMO Mixed Mode run: the states, phases and outcomes (Mixed Mode's, with MIMO
 * Random's gate and prediction holds), a drive's room against its ceiling, a part's numbers per
 * control, and how the run ended, by the part, control or drive it names. Display only: every
 * number is the service's.
 */

import type { MimoMixedEnd, MimoMixedOutcome, MimoMixedPhase } from '$lib/protocol/mimoMixed';
import { formatNumber } from '$lib/viz/format';
import { MIXED_OUTCOME_WORDS, MIXED_PHASE_WORDS, errDb, hz, partValue, PART_KIND_WORDS, PART_REFUSED_WORDS } from '../mixed/mixedWords';

export { errDb, hz, partValue, PART_KIND_WORDS, PART_REFUSED_WORDS };

export const MMIX_OUTCOME_WORDS: Record<MimoMixedOutcome, string> = MIXED_OUTCOME_WORDS;

export const MMIX_PHASE_WORDS: Record<MimoMixedPhase, string> = {
	...MIXED_PHASE_WORDS,
	gate_failed: 'Identification gate not passed, waiting for you',
	prediction_exceeded: 'The prediction is over a ceiling, waiting for you'
};

export const MMIX_STATE_WORDS: Record<string, string> = {
	unconfigured: 'No Setup',
	configured: 'Ready to Arm',
	blocked: 'Blocked',
	armed: 'Armed',
	running: 'Running',
	review: 'Waiting for Review',
	stopping: 'Stopping',
	finished: 'Finished'
};

/** A trip's name in words; the name itself where there are none. */
export const MMIX_TRIP_WORDS: Record<string, string> = {
	tone_abort: 'Tone Abort',
	band_abort: 'Narrowband Abort',
	floor_abort: 'Floor Envelope Abort',
	drive_vpk: 'Drive Peak Abort',
	drive_vrms: 'Drive rms Abort',
	tracker_lost: 'Tone Tracker Lost the Tone',
	divergence: 'Divergence Abort',
	law_fault: 'Control Law Stop',
	limit_abort: 'Limit Channel Abort'
};

const n3 = (v: number) => formatNumber(v, 3);

/** Volts: "2.41 V"; "-" when there is none. */
export function volts(v: number | null | undefined): string {
	return v === null || v === undefined || !Number.isFinite(v) ? '-' : `${n3(v)} V`;
}

/** A share in percent: "84 %". */
export function pct(v: number | null | undefined): string {
	return v === null || v === undefined || !Number.isFinite(v) ? '-' : `${Math.round(v * 100)} %`;
}

/** A need against its ceiling in words: "2.10 V of 3.00 V (70 %)". */
export function needWords(need: number, ceiling: number): string {
	return `${volts(need)} of ${volts(ceiling)} (${pct(ceiling > 0 ? need / ceiling : NaN)})`;
}

/** How a run ended, with the part, control or drive the trip names. */
export function endWords(end: MimoMixedEnd, names: { part: (id: string) => string; control: (i: number) => string; drive: (i: number) => string }): string {
	const where = [end.part ? `part ${names.part(end.part)}` : '', end.control !== null ? `control ${names.control(end.control)}` : '', end.drive !== null ? `drive ${names.drive(end.drive)}` : ''].filter(Boolean).join(', ');
	const trip = end.name ? (MMIX_TRIP_WORDS[end.name] ?? end.name) : null;
	return [trip ? `${trip}${where ? ` on ${where}` : ''}.` : '', end.message].filter(Boolean).join(' ');
}

/** The ceiling rule's scale on the tones in words. */
export function scaleWords(scale: number | null, rule: string | null): string {
	if (rule === 'in_turn') return 'Each tone is scaled in turn.';
	if (scale === null) return 'No tone is playing.';
	return scale >= 0.9995 ? 'No drive is short of room: every tone at its target.' : `Every tone at ${pct(scale)} of its target: a drive is out of room.`;
}
