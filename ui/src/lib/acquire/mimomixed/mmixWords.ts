/**
 * Plain words for a MIMO Mixed Mode run: the states, phases and outcomes as the service names
 * them, the controller's name for how a run ended, every refusal of a verb and the facts its detail
 * carries, a drive's room against its ceiling, and how the run ended by the part, control or drive
 * it names (the service names controls and drives by channel). Display only: every number is the
 * service's.
 */

import type { MimoMixedEnd, MimoMixedIdentificationView, MimoMixedOutcome, MimoMixedPhase, MimoMixedRefusalReason } from '$lib/protocol/mimoMixed';
import { formatNumber } from '$lib/viz/format';
import { errDb, hz, partValue, PART_KIND_WORDS } from '../mixed/mixedWords';

export { errDb, hz, partValue, PART_KIND_WORDS };

export const MMIX_OUTCOME_WORDS: Record<MimoMixedOutcome, string> = {
	completed: 'Completed',
	operator_stop: 'Stopped by the Operator',
	operator_abort: 'Aborted by the Operator',
	safe_stop: 'Stopped: Nobody Held Control',
	aborted: 'Aborted by a Protection',
	law_fault: 'Stopped: No Usable Drive From a Control Law',
	fault: 'Stopped on a Fault',
	sysid_level_unreachable: 'Identification Could Not Reach Its Level'
};

export const MMIX_PHASE_WORDS: Record<MimoMixedPhase, string> = {
	idle: 'Idle',
	armed: 'Station open, nothing driven',
	sysid: 'System identification',
	coherence_gate: 'Coherence gate',
	gate_failed: 'Coherence gate not passed, waiting for you',
	prediction_exceeded: 'The prediction is over a drive ceiling, waiting for you',
	awaiting_review: 'Identification done, waiting for you',
	floor_only: 'Floor alone, equalizing',
	engaged: 'Level schedule, every part',
	full_level: 'Full level',
	ramping_down: 'Ramping down',
	stopped: 'Stopped',
	aborted: 'Aborted'
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

/** The controller's name for an end, in words. */
export const MMIX_END_WORDS: Record<string, string> = {
	completed: 'Completed',
	stopped: 'Stopped',
	operator_abort: 'Aborted by the Operator',
	drive_vpk: 'Drive Peak Abort',
	drive_vrms: 'Drive rms Abort',
	drive_not_finite: 'Drive Not a Number',
	random_underrun: 'Floor Drive Ran Short',
	input_overload: 'Input Overload',
	kinematic: 'Shaker Rating Abort',
	divergence: 'Divergence Abort',
	floor_envelope: 'Floor Envelope Abort',
	floor_not_equalized: 'The Floor Did Not Equalize in Time',
	tone_abort: 'Tone Abort',
	tone_abort_at_ceiling: 'Tone Abort With Its Drive at the Ceiling',
	tone_unreadable: 'A Tone Could Not Be Read',
	band_abort: 'Narrowband Abort',
	limit_abort: 'Limit Channel Abort',
	station_stop: 'The Station Stopped',
	law_fault: 'Control Law Stop',
	ident_unreachable: 'Identification Could Not Reach Its Level',
	run_error: 'Run Error',
	run_thread_failure: 'The Run Stopped Unexpectedly'
};

/** A verb's refusal by name, as a heading. */
export const MMIX_REFUSAL_WORDS: Record<MimoMixedRefusalReason, string> = {
	not_configured: 'No Setup Yet',
	wrong_state: 'Not Now',
	run_finished: 'The Run Has Finished',
	run_active: 'A Run Is Live',
	shutting_down: 'The Service Is Shutting Down',
	config_invalid: 'The Setup Has Errors',
	config_unreadable: 'The Setup Could Not Be Read',
	table_invalid: 'The Channel Table Has Errors',
	law_not_checked: 'A Law Is Not Cleared for Hardware',
	arm_refused: 'The Station Could Not Arm',
	record_unwritable: "The Run's Record Could Not Be Written",
	hardware_busy: 'The Hardware Is Busy',
	gate_failed: 'Coherence Gate Not Passed',
	prediction_exceeded: 'Prediction Over a Drive Ceiling',
	override_not_needed: 'No Override Needed',
	budget_cannot_fit: 'The Drive Budget Cannot Fit',
	tone_model_not_finite: "The Model Is Not Usable at a Tone's Frequency",
	manual_mode_required: 'Switch to Manual First',
	not_a_rung: 'Not a Level of the Schedule',
	part_refused: 'The Part Cannot Be Switched',
	bad_params: 'Not Understood',
	adjust_not_built: 'No Live Parameters',
	exclude_not_built: 'Controls Cannot Be Excluded',
	identification_save_not_built: 'Identification Sets Are Not Saved',
	identification_load_not_built: 'Identification Sets Are Not Loaded'
};

/** `detail.why` of a part refusal, in words. */
export const PART_REFUSED_WORDS: Record<string, string> = {
	unknown_part: 'The run has no part by that id.',
	floor: 'The floor plays for the whole run; it cannot be switched off.',
	band_folded: "The narrowband is folded into the floor's reference, so it is not switched on its own."
};

const phaseWords = (p: unknown) => MMIX_PHASE_WORDS[p as MimoMixedPhase] ?? String(p);

/** The facts a refusal's detail carries, in words; `names.channel` gives a channel's name. */
export function mmixRefusalFacts(reason: string | null, detail: Record<string, unknown> | undefined, names: { channel: (c: number) => string; part: (id: string) => string }): string[] {
	if (!detail || !reason) return [];
	const has = (k: string) => detail[k] !== undefined && detail[k] !== null;
	const out: string[] = [];
	switch (reason) {
		case 'wrong_state':
			if (has('state')) out.push(`The run is ${MMIX_STATE_WORDS[String(detail.state)]?.toLowerCase() ?? String(detail.state)}${has('phase') ? `: ${phaseWords(detail.phase).toLowerCase()}` : ''}`);
			break;
		case 'override_not_needed':
			if (has('holds')) out.push(`What holds the run: ${phaseWords(detail.holds).toLowerCase()}`);
			break;
		case 'not_a_rung':
			if (Array.isArray(detail.levels_db)) out.push(`The levels of the schedule: ${(detail.levels_db as number[]).map((d) => `${d} dB`).join(', ')}`);
			break;
		case 'manual_mode_required':
			if (has('mode')) out.push(`The level schedule is in ${String(detail.mode)} mode`);
			break;
		case 'budget_cannot_fit':
			if (has('drive')) out.push(`Drive ${names.channel(Number(detail.drive))} needs ${volts(Number(detail.need_v))} against its ${volts(Number(detail.ceiling_v))} ceiling${has('time_s') ? ` at ${String(detail.time_s)} s` : ''}`);
			break;
		case 'tone_model_not_finite':
			if (has('part')) out.push(`Part ${names.part(String(detail.part))}${has('f_hz') ? ` at ${hz(Number(detail.f_hz))}` : ''}`);
			break;
		case 'part_refused':
			if (has('id')) out.push(`Part ${names.part(String(detail.id))}`);
			if (has('why')) out.push(PART_REFUSED_WORDS[String(detail.why)] ?? String(detail.why));
			break;
		case 'bad_params':
			if (has('param')) out.push(`Parameter: ${String(detail.param)}`);
			break;
		case 'record_unwritable':
			if (has('path')) out.push(`Path: ${String(detail.path)}`);
			break;
	}
	return out;
}

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

/** How a run ended: its name in words, the part, control or drive it names (controls and drives by channel), then the service's words. */
export function endWords(end: MimoMixedEnd, names: { part: (id: string) => string; channel: (c: number) => string }): string {
	const where = [end.part ? `part ${names.part(end.part)}` : '', end.control !== null ? `control ${names.channel(end.control)}` : '', end.drive !== null ? `drive ${names.channel(end.drive)}` : ''].filter(Boolean).join(', ');
	const named = end.name ? (MMIX_END_WORDS[end.name] ?? end.name) : MMIX_OUTCOME_WORDS[end.outcome] ?? end.outcome;
	return [`${named}${where ? ` on ${where}` : ''}.`, end.message].filter(Boolean).join(' ');
}

/** The ceiling rule's scale on the tones in words. */
export function scaleWords(scale: number | null, rule: string | null): string {
	if (rule === 'in_turn') return 'Each tone is scaled in turn.';
	if (scale === null) return 'No tone is playing.';
	return scale >= 0.9995 ? 'No drive is short of room: every tone at its target.' : `Every tone at ${pct(scale)} of its target: a drive is out of room.`;
}

/**
 * A coherence to set beside the gate: at least two decimals and as many as the gate has, cut
 * (not rounded) so a coherence under the gate never reads as at or over it.
 */
export function coherenceAgainst(c: number, gate: number | null): string {
	const places = Math.min(6, Math.max(2, gate === null ? 0 : (String(gate).split('.')[1] ?? '').length));
	const f = 10 ** places;
	const cut = (Math.floor(c * f + 1e-9) / f).toFixed(places);
	return cut.replace(/(\.\d\d\d*?)0+$/, '$1');
}

/** The identification's coherence gate in words: passed or not, the worst band coherence against the gate, where it was identified. */
export function gateWords(ident: MimoMixedIdentificationView): string {
	const c = ident.band_coherence.filter((x): x is number => x !== null);
	const worst = c.length ? coherenceAgainst(Math.min(...c), ident.gate_coherence) : '-';
	const gate = ident.gate_coherence === null ? '-' : String(ident.gate_coherence);
	return `${ident.passed ? 'The coherence gate passed' : 'The coherence gate did not pass'}: the worst band coherence is ${worst} against the gate's ${gate}, identified at ${ident.level_db} dB in ${ident.steps} ${ident.steps === 1 ? 'step' : 'steps'} of ${ident.frames} frames.`;
}

/**
 * The log's kinds the service names that the shared list has no word for, each short enough for
 * the log card's kind column.
 */
export const MMIX_LOG_WORDS: Record<string, string> = {
	identification: 'Sys ID',
	engage: 'Engage',
	floor: 'Floor',
	hold: 'Hold',
	limit: 'Limit',
	stop: 'Stop',
	part: 'Part'
};
