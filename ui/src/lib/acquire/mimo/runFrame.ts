/**
 * What the MIMO run views draw against, from the run's status and the operator's draft: each
 * controlled quantity's tolerance bands (its control row's, or under virtual control its degree of
 * freedom's), and where the sensors are for the geometry page.
 */

import type { MimoStatus } from '$lib/protocol/mimo';
import type { MimoGeometry } from '$lib/viz/geometry';
import { DEFAULT_BAND, type RowBand } from '$lib/viz/scales';
import type { MimoDraft } from './mimoSetup';

/** One band pair per controlled quantity, in the run's order; the service's defaults where none. */
export function runBandsOf(status: MimoStatus | null): RowBand[] {
	const labels = status?.shape?.labels ?? [];
	const own = status?.shape?.bands ?? [];
	return labels.map((_, i) => {
		const b = own[i];
		return b ? { alarm_db: [b.alarm_db[0], b.alarm_db[1]], abort_db: [b.abort_db[0], b.abort_db[1]] } : DEFAULT_BAND;
	});
}

/**
 * The sensors placed by the rigid-body geometry the operator entered, when every controlled
 * channel has a place; null otherwise (the geometry page then says what it needs).
 */
export function runGeometryOf(d: MimoDraft, status: MimoStatus | null): MimoGeometry | null {
	const rigid = d.reference.rigid;
	const names = Object.keys(rigid.sensors);
	if (!names.length) return null;
	const sensors = names.map((label) => ({ label, position: rigid.sensors[label].position, direction: rigid.sensors[label].direction }));
	if (status?.shape && status.shape.space === 'physical' && !status.shape.labels.every((l) => names.includes(l))) return null;
	return { sensors, drives: [], reference_point: rigid.reference_point, length_unit: rigid.length_unit };
}
