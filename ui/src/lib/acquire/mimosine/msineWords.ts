/**
 * Plain words for a MIMO Sine run: how a run ended and what tripped it, the service's refusals of
 * a setup by name, the identification and the sweep-rate warning per resonance, the readings, and
 * each built-in law's stated limit. Display only: every number is the service's, and nothing here
 * computes one beyond placing two of them side by side.
 */

import type { MimoSineIdentificationFacts, MimoSineLawFaultReason, MimoSineOutcome, MimoSineReadings, MimoSineResonance, MimoSineRunState, MimoSineTrip } from '$lib/protocol/mimoSine';
import type { Status } from '$lib/protocol/types';
import { formatNumber } from '$lib/viz/format';
import { SINE_PHASE_WORDS, hz } from '../sine/sineWords';
import { db } from '../words';

export { hz };

export const MSINE_OUTCOME_WORDS: Record<MimoSineOutcome, string> = {
	completed: 'Completed',
	operator_stop: 'Stopped by the Operator',
	operator_abort: 'Aborted by the Operator',
	safe_stop: 'Stopped: Nobody Held Control',
	aborted: 'Aborted by a Protection',
	held: 'Held at the Coherence Check',
	law_fault: 'Stopped: No Usable Drive From the Control Law',
	fault: 'Stopped on a Fault'
};

export function msineOutcomeTone(outcome: MimoSineOutcome): Status {
	if (outcome === 'completed') return 'pass';
	if (outcome === 'operator_stop') return 'info';
	return 'fail';
}

export const MSINE_PHASE_WORDS: Record<string, string> = { ...SINE_PHASE_WORDS };

export const MSINE_STATE_WORDS: Record<MimoSineRunState, string> = {
	unconfigured: 'Not set up',
	configured: 'Set up',
	blocked: 'Exceeds the shaker ratings',
	armed: 'Armed',
	running: 'Running',
	review: 'Waiting for review',
	stopping: 'Stopping',
	finished: 'Finished'
};

export const MSINE_TRIP_WORDS: Record<MimoSineTrip['kind'], string> = {
	abort_band: 'Abort Band',
	kinematic: 'Kinematic Abort',
	limit_channel: 'Limit Channel Abort',
	drive_vpk: 'Drive Abort',
	watchdog: 'Input Watchdog',
	oscillator_range: 'Frequency Out of Range',
	divergence: 'A Guard Bound Without a Break',
	law_fault: 'Control Law Stop'
};

export const MSINE_LAW_FAULT_WORDS: Record<MimoSineLawFaultReason, string> = {
	error: 'The control law reported an error, or could not be built',
	panicked: 'The control law failed while computing',
	over_budget: 'The control law took longer than its time budget',
	over_memory: 'The control law asked for more memory than its cap',
	wrong_drive_count: 'The control law gave a drive count other than the run has',
	non_finite: 'A drive or a value was not a number',
	too_many_values: 'The control law reported more values than allowed',
	undeclared_value: 'The control law reported a value its manifest does not declare',
	bad_request: 'The control law asked for a slow-down that is not between 0 and 1',
	wrong_reference_count: 'The control law returned a notched reference that is not one value per control'
};

const CAP_WORDS = { step: 'the change in one tick', rise: 'the rise in a second', rotation: 'the turn in one tick' } as const;

const num = (v: number, sig = 4) => formatNumber(v, sig);

/** The trip's facts as labeled values. */
export function msineTripFacts(trip: MimoSineTrip): { label: string; value: string }[] {
	const at = [
		{ label: 'At', value: hz(trip.at_hz) },
		{ label: 'Level', value: db(trip.at_level_db) }
	];
	switch (trip.kind) {
		case 'abort_band':
			return [...at, { label: 'Control', value: String(trip.channel) }, { label: 'Measured', value: num(trip.measured) }, { label: trip.side === 'high' ? 'Upper abort' : 'Lower abort', value: num(trip.limit) }];
		case 'kinematic': {
			const unit = trip.axis === 'velocity' ? 'in/s' : 'in pk-pk';
			return [...at, { label: 'Drive', value: String(trip.drive) }, { label: trip.axis === 'velocity' ? 'Velocity' : 'Displacement', value: `${num(trip.value, 3)} ${unit}` }, { label: 'Limit', value: `${num(trip.limit, 3)} ${unit}` }];
		}
		case 'limit_channel':
			return [...at, { label: 'Channel', value: String(trip.channel) }, { label: 'Measured', value: num(trip.measured) }, { label: 'Bound', value: num(trip.limit) }];
		case 'drive_vpk':
			return [...at, { label: 'Drive', value: String(trip.channel) }, { label: 'Played', value: `${num(trip.vpk)} V pk` }, { label: 'Drive abort', value: `${num(trip.abort_vpk)} V pk` }];
		case 'watchdog':
			return [...at, { label: 'Channel', value: String(trip.channel) }, { label: 'Read', value: `${num(trip.value)} ${trip.axis}`.trim() }, { label: 'Limit', value: `${num(trip.limit)} ${trip.axis}`.trim() }];
		case 'oscillator_range':
			return [...at, { label: 'Asked for', value: hz(trip.requested_hz) }, { label: 'Rate', value: `${num(trip.fs_hz, 6)} Hz` }];
		case 'divergence':
			return [...at, { label: 'Guard', value: CAP_WORDS[trip.cap] }, { label: 'Bound for', value: `${num(trip.capped_s, 3)} s` }];
		case 'law_fault':
			return [...at, { label: 'Why', value: MSINE_LAW_FAULT_WORDS[trip.reason] ?? trip.reason }, { label: 'In a row', value: String(trip.consecutive) }];
	}
}

/**
 * The service's refusals of a setup by name, as the operator reads them before arming. The
 * service's own message says what is wrong in this setup; this names the kind of refusal.
 */
export const MSINE_REASON_WORDS: Record<string, string> = {
	controls_out_of_range: 'Too few or too many controls',
	drives_out_of_range: 'Too few or too many drives',
	limits_out_of_range: 'Too many limit channels',
	duplicate_channel: 'A channel named twice',
	control_unknown_channel: 'A control that is not an enabled control row',
	drive_unknown_channel: 'A drive the table does not have, or out of its order',
	limit_unknown_channel: 'A limit channel that is not an enabled input',
	limit_on_control_channel: 'A limit channel on a control',
	abort_below_ceiling: 'A drive abort at or under its ceiling',
	profile_too_short: 'A reference or bound with too few rows',
	frequency_not_positive: 'A frequency at or under zero',
	frequency_not_ascending: 'Frequencies out of order',
	value_not_positive: 'A value at or under zero',
	profile_not_covering: "A reference or bound that does not cover the test's band",
	phase_missing: 'A control with no reference phase',
	phase_invalid: 'A reference phase out of order or out of range',
	steps_empty: 'A stepped sine with no steps',
	band_above_grid: 'A band above what the rate can play',
	levels_empty: 'No levels in the schedule',
	levels_not_ascending: 'Levels that do not rise',
	start_not_a_level: 'A start level that is not in the schedule',
	end_before_full_level: 'A test that ends before full level',
	tick_out_of_range: 'A control tick out of range',
	safe_stop_budget: 'A stop that takes too long',
	tracking_lag: 'A sweep too fast for the tracking filter',
	structure_unknown: 'No simulator structure by that name',
	structure_wrong_size: "A simulator structure of another size than the run's",
	shaker_record_invalid: 'A shaker record that does not read',
	law_unknown: 'A control law the catalog does not hold',
	law_wrong_world: 'A control law of another test type',
	law_shape_not_accepted: "A control law that does not accept the run's numbers of controls and drives",
	law_test_type_not_accepted: 'A control law that does not accept this kind of test',
	law_amplitude_only_not_accepted: 'A control law that needs a phase on every reference',
	law_param_unknown: 'A parameter the law does not declare',
	law_param_invalid: "A law's parameter out of its range",
	out_of_range: 'A value out of range',
	join_discontinuity: 'A step in a reference between rows',
	alarm_wider_than_abort: 'An alarm band wider than its abort band'
};

export function reasonWords(reason: string): string {
	return MSINE_REASON_WORDS[reason] ?? reason.replace(/_/g, ' ');
}

/** The identification in a line: its level, its coherence and its trust. */
export function identificationWords(id: MimoSineIdentificationFacts | null): string | null {
	if (!id) return null;
	const worst = id.coherence_mean.length ? Math.min(...id.coherence_mean) : null;
	const measured = id.measured_level_db === null ? '' : ` (measured ${db(id.measured_level_db)})`;
	const coh = worst === null ? '' : `, lowest control coherence ${num(worst, 3)}`;
	return `At ${db(id.level_db)}${measured}, ${id.realizations} records${coh}, ${num(id.trusted_frac * 100, 3)} % of the band trusted, ${id.passed ? 'passed' : 'held'} at ${num(id.gate_coherence, 2)}.`;
}

/** The sweep-rate warning for one resonance, or null when it is not flagged. */
export function resonanceWords(r: MimoSineResonance): string | null {
	if (!r.flagged) return null;
	return `At ${hz(r.freq_hz)} the sweep spends ${num(r.time_constants, 2)} of the resonance's own time constants inside its half-power bandwidth (${num(r.bandwidth_hz, 3)} Hz). Under two, the law's error through it is expected to be large; a slower sweep is the remedy.`;
}

/** The readings in words: shown and recorded, never judged. */
export function readingsWords(r: MimoSineReadings | null | undefined): { label: string; value: string; help: string }[] {
	if (!r) return [];
	return [
		{ label: 'Opposition', value: r.opposition_ratio === null ? '-' : num(r.opposition_ratio, 3), help: 'The drive vector over the least drive that would give the same control response by the identification: 1 when the shakers do not work against each other.' },
		{ label: 'Largest share', value: r.largest_share === null ? '-' : `${num(r.largest_share * 100, 3)} %`, help: "The largest share of the drive vector carried by one shaker." },
		{ label: 'Largest turn', value: r.rotation_deg === null ? '-' : `${num(r.rotation_deg, 3)} deg`, help: "The largest turn of a drive's phase in the last tick." }
	];
}

/**
 * Each built-in law's limit, as the guide states it. A law the catalog lists that is not here has
 * its own entry's summary and nothing more.
 */
export const LAW_LIMITS: Record<string, string> = {
	matrix:
		'This law assumes the test article responds at test level as it did during the low-level identification. Where a resonance has moved by less than a quarter of its half-power bandwidth, expect the same control as on a linear article. Between a quarter and one bandwidth, expect errors over 3 dB at that resonance. Past about one bandwidth the loop can be lost and the test will abort. A Q of 50 puts one bandwidth at 2 percent of frequency.',
	paired: 'No model of the test article: each control is paired with one drive. Expect it to fail where the shakers are strongly coupled: it held none of the dwells on the coupled structure of the research testbed. It is the simplest law and the template for writing your own.',
	steepest_descent: 'Moves the drive down the gradient of the control error through the identification. Shown to hold a dwell; it did not hold a stepped sine reliably or any sweep in the research testbed.',
	secant: 'Finds a gain per control from two successive drives. Shown to hold a dwell only; it did not hold a stepped sine or a sweep in the research testbed.',
	ratio: 'Multiplies each drive by a compressed ratio of wanted to measured amplitude on its paired control. Shown to hold a dwell and a stepped sine with the reference pairing; it did not hold a sweep in the research testbed.'
};
