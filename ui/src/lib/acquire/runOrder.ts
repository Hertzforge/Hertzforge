/**
 * Whether `next` would take a run's screen back from "finished" to an earlier state.
 *
 * A verb's reply carries the run's state when the verb was taken (a stop answers "stopping"),
 * while the run goes on reporting its state as events. When the run finishes before the reply
 * lands, the reply is older than what the screen holds, and applying it would leave the screen
 * on "stopping" with no later event to correct it. Within one run "finished" is final; a new run
 * has a new id.
 */
export function regresses(
	was: { run_id?: string | null; state?: string } | null | undefined,
	next: { run_id?: string | null; state?: string } | null | undefined
): boolean {
	return !!was && !!next && !!was.run_id && was.run_id === next.run_id && was.state === 'finished' && next.state !== 'finished';
}
