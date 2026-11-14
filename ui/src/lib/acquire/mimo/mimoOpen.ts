/**
 * MIMO Random's setup, read back into the editor: a `mimo_random.setup` setup (saved in a project,
 * or the one a run's record holds) as a draft, and the reference it carries. The reference stays
 * as the setup gave it: the draft takes the `field` method, a reference made elsewhere, so nothing
 * rebuilds it from tables the setup does not hold. A virtual control space comes in as the
 * operator's own matrices.
 *
 * What the draft has no field for is not carried; the shared setup bar compares the draft's setup
 * with the one opened and names every difference before the draft is touched.
 */

import type { MimoReference, MimoSetup, MimoTransform } from '$lib/protocol/mimo';
import type { BandDb } from '$lib/protocol/types';
import type { LawValue } from '../law/lawForm';
import { defaultMimoSetup, type MatrixDraft, type MimoDraft } from './mimoSetup';

const band = (b: BandDb | undefined, fallback: BandDb): BandDb => (b ? [b[0], b[1]] : [fallback[0], fallback[1]]);

function matrix(t: MimoTransform | null | undefined, source: string): MatrixDraft {
	return t ? { rows: t.rows, cols: t.cols, values: [...t.values], source } : { rows: 0, cols: 0, values: [], source: null };
}

/** The draft a MIMO Random setup describes, and its reference. */
export function fromMimoSetup(s: MimoSetup): { draft: MimoDraft; reference: MimoReference } {
	const d = defaultMimoSetup();
	d.title = s.title ?? '';
	d.record = {
		operator: s.operator ?? '',
		article: s.article ?? '',
		notes: s.notes ?? '',
		document: s.specification?.document ?? '',
		clause: s.specification?.clause ?? '',
		streams: s.record?.streams ?? true
	};
	const defaults = { alarm_db: d.reference.alarm_db, abort_db: d.reference.abort_db };
	for (const c of s.channels.controls) d.controls[String(c.channel)] = { alarm_db: band(c.alarm_db, defaults.alarm_db), abort_db: band(c.abort_db, defaults.abort_db) };
	for (const r of s.channels.drives) {
		d.drives[String(r.channel)] = {
			ceiling_vrms: r.ceiling_vrms,
			abort_vrms: r.abort_vrms ?? null,
			peak_v: r.peak_v ?? d.drives[String(r.channel)]?.peak_v ?? 6,
			shaker_text: r.shaker ? JSON.stringify(r.shaker) : '',
			moving_mass_lb: r.moving_mass_lb ?? 0
		};
	}
	for (const l of s.channels.limits ?? []) d.limits[String(l.channel)] = { ceiling: l.ceiling, abort_margin_db: l.abort_margin_db ?? 3 };
	d.article_mass_lb = s.channels.article_mass_lb ?? 0;
	const space = s.space;
	if (space?.kind === 'virtual') {
		d.space = {
			kind: 'virtual',
			dofs: [...space.dofs],
			response: matrix(space.response_transform, 'The opened setup'),
			driveOn: !!space.drive_transform,
			drive: matrix(space.drive_transform, 'The opened setup'),
			dofBands: Object.fromEntries(space.dofs.map((n, i) => [n, { alarm_db: band(space.dof_bands?.[i]?.alarm_db, defaults.alarm_db), abort_db: band(space.dof_bands?.[i]?.abort_db, defaults.abort_db) }]))
		};
	}
	d.reference.method = 'field';
	if (s.law) d.law = { id: s.law.id, params: { ...((s.law.params ?? {}) as Record<string, LawValue>) } };
	d.estimation = { ...d.estimation, ...(s.estimation ?? {}) };
	d.identification = { ...d.identification, ...(s.identification ?? {}) };
	const c = s.control ?? {};
	if (c.frf) d.frf = c.frf;
	if (c.frf_blend !== undefined) d.frf_blend = c.frf_blend;
	for (const k of ['level_ramp_s', 'shutdown_ramp_s', 'max_loops_out', 'cap_up_db', 'cap_down_db', 'law_budget', 'law_misses'] as const) {
		if (c[k] !== undefined) d.control[k] = c[k] as number;
	}
	d.schedule = {
		mode: s.schedule.mode ?? 'auto',
		levels: s.schedule.levels.map((l) => ({ db: l.db, dwell_s: l.dwell_s ?? 0 })),
		full_level_s: s.schedule.full_level_s
	};
	return { draft: d, reference: s.reference };
}
