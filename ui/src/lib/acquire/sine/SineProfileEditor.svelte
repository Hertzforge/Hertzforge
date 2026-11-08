<script lang="ts">
	/**
	 * The test's title and its control profile: rows of a frequency and a value in the profile's own
	 * units (g peak, in/s peak, in peak to peak), with alarm and abort bands for the profile and, where
	 * a row needs them, its own. Rows are typed here, pasted, or read from a file on the computer that
	 * runs the test; the control service reads pasted or file rows and says what it found. Beside the
	 * table the service draws the profile as acceleration, velocity and displacement at full level,
	 * with its kinematics: a computation, never a measurement. The shell computes none of it.
	 */
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import SineKinematics from './SineKinematics.svelte';
	import SineProfileTable from './SineProfileTable.svelte';
	import { hasProfileText } from './sineSetup';
	import type { SineStore } from './sineStore.svelte';

	interface Props {
		sine: SineStore;
	}
	let { sine }: Props = $props();

	const d = $derived(sine.setup);
	const p = $derived(d.profile);
	const locked = $derived(sine.locked);
	const E = $derived(sine.fieldErrors);
	const W = $derived(sine.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const check = $derived(sine.profileCheck);
	// The dry run of the whole setup draws the profile over the test's band; the profile check draws it on its own.
	const dryViews = $derived(sine.check && sine.checkedKey === sine.key ? sine.check.views : []);
	const views = $derived((dryViews.length ? dryViews : (check?.views ?? [])).filter((v) => v.view === 'spectrum'));
	const findings = $derived(check ? [...check.errors.map((e) => ({ level: 'error' as const, message: e.message })), ...check.warnings.map((e) => ({ level: 'warning' as const, message: e.message }))] : []);
	const note = $derived(sine.imported);
	let pasted = $state('');
	let path = $state('');
	let reading = $state(false);

	async function readText() {
		if (!hasProfileText(pasted)) return;
		reading = true;
		if (await sine.importProfile({ text: pasted })) pasted = '';
		reading = false;
	}

	async function readFile() {
		if (!path.trim()) return;
		reading = true;
		if (await sine.importProfile({ path })) path = '';
		reading = false;
	}

	function setPair(pair: [number, number], i: 0 | 1, v: number | null) {
		pair[i] = v ?? NaN;
	}
</script>

<div class="cols">
	<div class="stack">
		<section class="aq-panel" aria-labelledby="np-title">
			<div class="aq-head"><h2 id="np-title">Profile</h2><span class="aq-sub">The amplitude the control point should see at each frequency, at full level.</span></div>
			<div class="aq-fields">
				<div class="sel wide">
					<label for="np-name" class="lbl">Test title</label>
					<input id="np-name" class="field" bind:value={d.title} disabled={locked} aria-invalid={errs('title').length > 0} data-testid="sine-title" />
					<Findings items={errs('title').map((message) => ({ level: 'error' as const, message }))} />
				</div>
				<div class="sel">
					<label for="np-label" class="lbl">Profile name</label>
					<input id="np-label" class="field" bind:value={p.label} disabled={locked} placeholder="Optional" data-testid="sine-profile-label" />
				</div>
			</div>
			<p class="aq-lead aq-small spaced">Each row holds from its frequency to the next row's. Neighbours of one quantity are joined on log-log axes; where the quantity changes, the left row's value holds up to the next row. Values are in the row's own unit: g peak, in/s peak, or in peak to peak.</p>
			<SineProfileTable rows={p.rows} prefix="profile.rows" errors={E} warnings={W} {locked} testid="sine-profile-rows" label="Profile rows" />

			<div class="aq-group">
				<h3>Bands</h3>
				<p class="aq-lead aq-small">How far below and above the profile the control point may read: past the alarm band the run warns, past the abort band it stops. A row with bands of its own uses them from that row to the next.</p>
				<div class="aq-fields">
					<NumberField id="np-al" label="Alarm below" unit="dB" step={0.5} min={0} value={p.alarm_db[0]} onchange={(v) => setPair(p.alarm_db, 0, v)} errors={errs('profile.alarm_db')} disabled={locked} testid="sine-alarm-below" />
					<NumberField id="np-ah" label="Alarm above" unit="dB" step={0.5} min={0} value={p.alarm_db[1]} onchange={(v) => setPair(p.alarm_db, 1, v)} disabled={locked} testid="sine-alarm-above" />
					<NumberField id="np-bl" label="Abort below" unit="dB" step={0.5} min={0} value={p.abort_db[0]} onchange={(v) => setPair(p.abort_db, 0, v)} errors={errs('profile.abort_db')} warnings={W['profile.abort_db'] ?? []} disabled={locked} testid="sine-abort-below" />
					<NumberField id="np-bh" label="Abort above" unit="dB" step={0.5} min={0} value={p.abort_db[1]} onchange={(v) => setPair(p.abort_db, 1, v)} disabled={locked} testid="sine-abort-above" />
				</div>
				<p class="aq-small aq-muted">Usual: alarm 1.5 dB and abort 3 dB either side.</p>
			</div>

			<div class="aq-group">
				<h3>Paste or Read Rows</h3>
				<p class="aq-lead aq-small">A header row naming the frequency column and the quantity columns (accel_g, velocity_ips, disp_pp_in), then one row a line with one value in each, separated by tabs, commas, semicolons or spaces. Lines starting with # are comments. The rows replace the table; the bands stay.</p>
				<textarea class="field mono rows" rows="4" bind:value={pasted} disabled={locked} aria-label="Pasted profile rows" placeholder={'freq_hz, disp_pp_in, accel_g\n20, 0.0097,\n45, , 1\n2000, , 1'} data-testid="sine-profile-paste"></textarea>
				<div class="row">
					<button class="btn sm" disabled={locked || !hasProfileText(pasted) || reading || !sine.connected} onclick={readText} data-testid="sine-profile-read-text">Read Pasted Rows</button>
				</div>
				<div class="row">
					<input class="field mono grow" aria-label="Profile file path" placeholder="/full/path/to/profile.csv" bind:value={path} disabled={locked} data-testid="sine-profile-path" onkeydown={(e) => e.key === 'Enter' && void readFile()} />
					<button class="btn sm" disabled={locked || !path.trim() || reading || !sine.connected} onclick={readFile} data-testid="sine-profile-read-file">Read File</button>
				</div>
				<p class="aq-small aq-muted">A file is read by the control service from its full path on the computer that runs the test, at most 4 MiB.</p>
				{#if note}
					<Findings items={[...note.errors.map((e) => ({ level: 'error' as const, message: e.message })), ...note.warnings.map((e) => ({ level: 'warning' as const, message: e.message }))]} testid="sine-profile-import-findings" />
					{#if !note.errors.length}<p class="aq-small aq-muted" data-testid="sine-profile-imported">Read {note.rows} {note.rows === 1 ? 'row' : 'rows'}{note.path ? ` from ${note.path}` : ''}.</p>{/if}
				{/if}
			</div>
		</section>
	</div>

	<section class="aq-panel preview" aria-labelledby="np-prev" data-testid="sine-profile-preview" data-state={sine.validating ? 'busy' : views.length ? 'ok' : 'none'}>
		<div class="aq-head"><h2 id="np-prev">The Profile at Full Level</h2><span class="aq-sub">A computation by the control service, never a measurement.</span></div>
		{#if views.length}
			<div class="plots">
				{#each views as v, i (i)}
					<div class="plot" data-testid="sine-profile-view-{i}"><ViewRenderer view={v} minHeight={i === 0 ? 240 : 150} /></div>
				{/each}
			</div>
		{:else if sine.validating}
			<p class="aq-muted aq-small">Reading the profile.</p>
		{:else if sine.validationError}
			<p class="aq-notice warn aq-small" data-testid="sine-validate-error">{sine.validationError}</p>
		{:else}
			<p class="aq-muted aq-small">{sine.connected ? 'Give at least two rows with numbers to draw the profile.' : 'The profile is drawn by the control service once it is connected.'}</p>
		{/if}
		<Findings items={findings} testid="sine-profile-findings" />
		<SineKinematics kinematics={check?.kinematics ?? null} crossovers={check?.crossovers ?? []} />
	</section>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.sel.wide {
		grid-column: span 2;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.spaced {
		margin: var(--sp-3) 0 var(--sp-2);
	}

	.mono {
		font-family: var(--font-mono);
	}

	.rows {
		width: 100%;
		resize: vertical;
	}

	.row {
		display: flex;
		gap: var(--sp-2);
		margin-top: var(--sp-2);
	}

	.grow {
		flex: 1;
		min-width: 0;
	}

	.preview {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.plots {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.plot {
		min-width: 0;
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
