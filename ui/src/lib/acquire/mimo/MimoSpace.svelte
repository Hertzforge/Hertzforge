<script lang="ts">
	/**
	 * Where control happens: on the control channels themselves (physical), or on named degrees
	 * of freedom through the response transformation (virtual), with an optional drive
	 * transformation for ganged or differential shakers. Both matrices are edited here, with paste
	 * from a spreadsheet and CSV import, and read back with the service's rank and condition. A
	 * rigid-body or imported reference brings its own transformation, shown read-only.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { dofBandsOf, ensureDofBands, rowName, type MatrixDraft } from './mimoSetup';
	import MatrixGrid from './MatrixGrid.svelte';
	import type { MimoState } from './mimoStore.svelte';

	interface Props {
		mimo: MimoState;
	}
	let { mimo }: Props = $props();

	const d = $derived(mimo.setup);
	const s = $derived(mimo.space);
	const locked = $derived(mimo.locked);
	const E = $derived(mimo.fieldErrors);
	const W = $derived(mimo.fieldWarnings);
	const controls = $derived(mimo.channels.controls.map(rowName));
	const drives = $derived(mimo.channels.drives.map(rowName));
	const check = $derived(mimo.check && mimo.checkedKey === mimo.key ? mimo.check.transforms : null);
	const fixed = $derived(s.from !== 'operator');
	let dofText = $state('');

	$effect(() => {
		// The names field follows the draft unless it is being typed in.
		if (document.activeElement?.id !== 'ms-dofs') dofText = d.space.dofs.join(', ');
	});

	function setKind(kind: 'physical' | 'virtual') {
		if (locked || fixed) return;
		d.space.kind = kind;
		if (kind === 'virtual' && !d.space.dofs.length) {
			d.space.dofs = controls.slice(0, Math.min(controls.length, 6)).map((_, i) => `DOF ${i + 1}`);
		}
	}

	function setDofs(text: string) {
		dofText = text;
		d.space.dofs = text
			.split(/[,\n]/)
			.map((x) => x.trim())
			.filter((x, i, all) => x !== '' || i < all.length - 1);
	}

	function setVirtualDrives(n: number) {
		if (!Number.isInteger(n) || n < 1) return;
		const m = d.space.drive;
		const values = new Array(m.rows * n).fill(0);
		for (let r = 0; r < m.rows; r++) for (let c = 0; c < Math.min(n, m.cols); c++) values[r * n + c] = m.values[r * m.cols + c];
		d.space.drive = { rows: m.rows, cols: n, values, source: m.source };
	}

	const at = (k: string) => E[k] ?? [];

	// Under virtual control the reference and its tolerances are in the control space, so each
	// degree of freedom carries its own alarm and abort bands.
	$effect(() => {
		if (s.kind === 'virtual' && !locked) ensureDofBands(d, s.dofs);
	});
	const virtualDrives = $derived(Array.from({ length: d.space.drive.cols }, (_, i) => `U${i + 1}`));
</script>

<section class="aq-panel" aria-labelledby="ms-title" data-testid="mimo-space">
	<div class="aq-head">
		<h2 id="ms-title">Control Space</h2>
		<span class="aq-spacer"></span>
		<div class="aq-seg" role="radiogroup" aria-label="Control space">
			<button type="button" role="radio" aria-checked={s.kind === 'physical'} disabled={locked || fixed} onclick={() => setKind('physical')} data-testid="space-physical">Physical</button>
			<button type="button" role="radio" aria-checked={s.kind === 'virtual'} disabled={locked || fixed} onclick={() => setKind('virtual')} data-testid="space-virtual">Virtual</button>
		</div>
	</div>
	<p class="aq-muted aq-small lead">
		{#if s.from === 'rigid'}
			The rigid-body reference controls the six rigid-body degrees of freedom; its response transformation comes from the sensor geometry, under Reference.
		{:else if s.from === 'import'}
			The imported reference is written on degrees of freedom and brings its own transformation.
		{:else if s.kind === 'physical'}
			The reference is written on the control channels themselves: one row and one column per control.
		{:else}
			The control responses are combined into the degrees of freedom you name, and the reference is written on those. The response transformation has one row per degree of freedom and one column per control channel, and needs full row rank.
		{/if}
	</p>

	{#if s.kind === 'virtual'}
		{#if !fixed}
			<div class="dofs">
				<label class="lbl" for="ms-dofs">Degrees of freedom</label>
				<input id="ms-dofs" class="field" value={dofText} disabled={locked} oninput={(e) => setDofs(e.currentTarget.value)} placeholder="Names, separated by commas" data-testid="space-dofs" />
				<span class="aq-muted aq-small num">{d.space.dofs.length} named</span>
			</div>
			<Findings items={at('space.dofs').map((message) => ({ level: 'error' as const, message }))} />
		{/if}
		{#if s.response}
			<MatrixGrid
				id="tr"
				title="Response Transformation"
				sub="Degrees of freedom by control channels"
				matrix={s.response}
				rowLabels={s.dofs}
				colLabels={controls}
				check={check?.response ?? null}
				needs={{ rank: Math.min(s.dofs.length, controls.length), words: `${s.dofs.length} degrees of freedom by ${controls.length} control channels` }}
				errors={[...at('space.response_transform'), ...at('space')]}
				warnings={W['space.response_transform'] ?? []}
				readonly={fixed}
				disabled={locked}
				onchange={(m: MatrixDraft) => (d.space.response = m)}
			/>
		{:else}
			<p class="aq-muted aq-small">The transformation appears once the sensor geometry fits, under Reference.</p>
		{/if}

		{#if s.dofs.length}
			<div class="bands" data-testid="space-dof-bands">
				<h3>Tolerance per Degree of Freedom</h3>
				<p class="aq-muted aq-small">The reference is written on the degrees of freedom, so its alarm and abort bands are too: below and above the reference, in dB.</p>
				<table class="aq-table">
					<thead><tr><th>Degree of freedom</th><th class="num">Alarm below</th><th class="num">Alarm above</th><th class="num">Abort below</th><th class="num">Abort above</th></tr></thead>
					<tbody>
						{#each s.dofs as name, i (name + i)}
							{@const b = d.space.dofBands?.[name] ?? dofBandsOf(d, name)}
							<tr data-testid="space-dof-band-{i}">
								<td>{name}</td>
								<td class="num"><NumberField id="ms-b{i}-al" label="{name} alarm below" compact unit="dB" bind:value={b.alarm_db[0]} disabled={locked} /></td>
								<td class="num"><NumberField id="ms-b{i}-ah" label="{name} alarm above" compact unit="dB" bind:value={b.alarm_db[1]} disabled={locked} /></td>
								<td class="num"><NumberField id="ms-b{i}-bl" label="{name} abort below" compact unit="dB" bind:value={b.abort_db[0]} disabled={locked} /></td>
								<td class="num"><NumberField id="ms-b{i}-bh" label="{name} abort above" compact unit="dB" bind:value={b.abort_db[1]} disabled={locked} /></td>
							</tr>
						{/each}
					</tbody>
				</table>
				<Findings items={at('space.dof_bands').map((message) => ({ level: 'error' as const, message }))} />
			</div>
		{/if}

		<div class="drive-head">
			<label class="aq-switch" for="ms-drive-on">
				<input id="ms-drive-on" type="checkbox" role="switch" checked={d.space.driveOn} disabled={locked} onchange={(e) => (d.space.driveOn = e.currentTarget.checked)} data-testid="space-drive-on" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Transform the drives too</span>
				<span class="word">{d.space.driveOn ? 'On' : 'Off'}</span>
			</label>
			<span class="aq-muted aq-small">For ganged or differential shakers: the law commands virtual drives, which the transformation spreads to the outputs.</span>
		</div>
		{#if d.space.driveOn && s.drive}
			<div class="vd">
				<label class="lbl" for="ms-vd">Virtual drives</label>
				<input id="ms-vd" class="field num" type="number" min="1" max={drives.length} step="1" value={d.space.drive.cols} disabled={locked || s.from === 'import'} onchange={(e) => setVirtualDrives(Number(e.currentTarget.value))} />
			</div>
			<MatrixGrid
				id="td"
				title="Drive Transformation"
				sub="Drives by virtual drives"
				matrix={s.drive}
				rowLabels={drives}
				colLabels={virtualDrives}
				check={check?.drive ?? null}
				needs={{ rank: Math.min(drives.length, d.space.drive.cols), words: `${drives.length} drives by ${d.space.drive.cols} virtual drives` }}
				errors={at('space.drive_transform')}
				warnings={W['space.drive_transform'] ?? []}
				readonly={s.from === 'import'}
				disabled={locked}
				onchange={(m: MatrixDraft) => (d.space.drive = m)}
			/>
		{/if}
	{/if}
</section>

<style>
	.bands {
		margin: var(--sp-3) 0;
	}

	.bands h3 {
		margin-bottom: 4px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.lead {
		margin: -6px 0 var(--sp-3);
		max-width: 90ch;
	}

	.dofs,
	.vd {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px var(--sp-3);
		margin-bottom: var(--sp-3);
	}

	.dofs .field {
		flex: 1;
		max-width: 520px;
	}

	.vd .field {
		width: 90px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.drive-head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px var(--sp-3);
		margin: var(--sp-4) 0 var(--sp-3);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--line-1);
	}
</style>
