<script lang="ts">
	/**
	 * The control law of a test: the laws of the test's world from the service's catalog (built in
	 * first, then imported), the chosen law's identity and check record, and its parameter form,
	 * drawn from the law's own manifest so nothing here is specific to a law or a test type. A law
	 * file is imported by its path on the computer that runs the test. The test type's screen says
	 * what its law is fixed for, and adds facts of its own (what shapes a MIMO law supports). A test
	 * type whose laws may declare parameters changeable during a run (Sine) passes them as `live`,
	 * and the form says which they are.
	 */
	import type { Snippet } from 'svelte';
	import ParamForm from '$lib/forms/ParamForm.svelte';
	import type { ParamValues } from '$lib/forms/params';
	import type { LawEntry, LawParam } from '$lib/protocol/mimo';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { CHECK_WORDS, lawParamDescriptor, lawParamError, liveParamLabels, shortHash, type LawErrors, type LawValue } from './lawForm';

	interface Props {
		/** The laws offered, in the catalog's order. */
		laws: LawEntry[];
		/** The chosen law's entry, or null while the catalog has not said. */
		law: LawEntry | null;
		/** The chosen law's id. */
		chosenId: string;
		/** The chosen law's values, by key; the form edits them in place. */
		values: Record<string, LawValue>;
		/** What the law is fixed for, in a line. */
		subtitle: string;
		locked: boolean;
		connected: boolean;
		/** A request in flight (its verb), or null. */
		busy: string | null;
		problems: { file: string; reason: string }[];
		/** Findings by key: `law.id`, `law.params.<key>`. */
		errors: LawErrors;
		/** The names of the controls a per-channel parameter has one value for. */
		channelLabels?: string[];
		onchoose: (id: string) => void;
		onimport: (path: string) => Promise<boolean>;
		/** More facts about the chosen law, as `<div><dt></dt><dd></dd></div>` rows. */
		facts?: Snippet<[LawEntry]>;
		/** A notice under the law's details (the setup does not admit it). */
		notice?: Snippet<[LawEntry]>;
		/** The keys of the chosen law's parameters that may change during a run; absent, none is marked. */
		live?: string[];
		/** The panel's heading; a test type with two laws names each by its role. */
		heading?: string;
	}
	let { laws, law, chosenId, values, subtitle, locked, connected, busy, problems, errors, channelLabels = [], onchoose, onimport, facts, notice, live, heading = 'Control Law' }: Props = $props();

	let importPath = $state('');
	let importOpen = $state(false);

	const descriptors = $derived((law?.params ?? []).map(lawParamDescriptor).filter((p) => p !== null));
	const liveLabels = $derived(liveParamLabels(law, live));
	const extras = $derived((law?.params ?? []).filter((p) => p.kind === 'per_channel' || p.kind === 'curve'));
	const errs = (k: string) => errors[k] ?? [];

	async function doImport() {
		if (!importPath.trim()) return;
		if (await onimport(importPath)) {
			importPath = '';
			importOpen = false;
		}
	}

	function perChannel(p: LawParam): number[] {
		const v = values[p.key];
		return Array.isArray(v) ? (v as number[]) : [];
	}

	function curve(p: LawParam): [number, number][] {
		const v = values[p.key];
		return Array.isArray(v) ? (v as [number, number][]) : [];
	}
</script>

<section class="aq-panel" aria-labelledby="law-title" data-testid="law-panel">
	<div class="aq-head">
		<h2 id="law-title">{heading}</h2>
		<span class="aq-sub">{subtitle}</span>
		<span class="aq-spacer"></span>
		<button class="btn sm" disabled={locked} aria-expanded={importOpen} onclick={() => (importOpen = !importOpen)} data-testid="law-import-open">Import Law</button>
	</div>

	{#if importOpen}
		<div class="import" data-testid="law-import">
			<label for="law-path" class="lbl">Law file (.wasm)</label>
			<input id="law-path" class="field mono" bind:value={importPath} placeholder="Full path on the computer that runs the test" disabled={locked} data-testid="law-path" />
			<button class="btn sm primary" disabled={locked || !importPath.trim() || busy !== null} onclick={doImport} data-testid="law-import-go">{busy === 'Import Law' ? 'Importing' : 'Import'}</button>
			<p class="aq-muted aq-small">The law is checked when it is imported and copied into the program's data folder under its hash. An imported law runs on the simulator at once; it drives hardware only once its check has passed.</p>
		</div>
	{/if}

	{#if laws.length}
		<div class="laws" role="radiogroup" aria-label="Control law" data-testid="law-list">
			{#each laws as l (l.id)}
				{@const on = chosenId === l.id}
				<button type="button" role="radio" class="law" class:on aria-checked={on} disabled={locked} onclick={() => onchoose(l.id)} data-testid="law-{l.builtin ? l.id : 'imported'}">
					<span class="dot" aria-hidden="true">{#if on}<svg viewBox="0 0 12 12" width="12" height="12"><circle cx="6" cy="6" r="5" fill="none" stroke="currentColor" stroke-width="1.4" /><circle cx="6" cy="6" r="2.6" fill="currentColor" /></svg>{:else}<svg viewBox="0 0 12 12" width="12" height="12"><circle cx="6" cy="6" r="5" fill="none" stroke="currentColor" stroke-width="1.2" /></svg>{/if}</span>
					<span class="name">{l.name}</span>
					<span class="ver num">{l.version}</span>
					<span class="kind">{l.builtin ? 'Built in' : 'Imported'}</span>
					<span class="sum">{l.summary}</span>
				</button>
			{/each}
		</div>
	{:else}
		<p class="aq-muted aq-small">{connected ? 'Reading the law catalog.' : 'The law catalog is read from the control service once it is connected.'}</p>
	{/if}
	{#if problems.length}
		<Findings items={problems.map((p) => ({ level: 'warning' as const, message: `${p.file}: ${p.reason}` }))} />
	{/if}
	<Findings items={errs('law.id').map((message) => ({ level: 'error' as const, message }))} testid="law-findings" />

	{#if law}
		<div class="details" data-testid="law-details">
			<h3>{law.name} <span class="num aq-muted">{law.version}</span></h3>
			<p class="aq-small summary">{law.summary}</p>
			<dl class="facts">
				<div><dt>Author</dt><dd>{law.author}</dd></div>
				<div><dt>File hash</dt><dd class="num mono" title={law.sha256 ?? undefined}>{shortHash(law.sha256)}</dd></div>
				<div>
					<dt>Check</dt>
					<dd class="check {law.check.status}" data-testid="law-check">
						{#if law.check.status === 'failed' || law.check.status === 'unreadable'}
							<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /></svg>
						{:else if law.check.status === 'unchecked'}
							<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /></svg>
						{:else}
							<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
						{/if}
						{CHECK_WORDS[law.check.status]}
					</dd>
				</div>
				<div><dt>Hardware</dt><dd>{law.check.hardware ? 'May drive hardware' : 'Simulator only'}</dd></div>
				{#if facts}{@render facts(law)}{/if}
			</dl>
			<p class="aq-small aq-muted">{law.check.message}</p>
			{#if notice}{@render notice(law)}{/if}
		</div>

		{#if law.params.length}
			<div class="aq-group params" data-testid="law-params">
				<h3>Parameters</h3>
				<ParamForm params={descriptors} values={values as ParamValues} showAllErrors layout="stack" disabled={locked} idPrefix="law" testidPrefix="law-param" />
				{#if liveLabels}
					<p class="aq-small live" data-testid="law-live" data-live={(live ?? []).join(' ')}>
						{#if liveLabels.length}
							<span class="tag">Live</span>
							{liveLabels.join(', ')} {liveLabels.length === 1 ? 'can' : 'can each'} be changed while the run runs; every other parameter is fixed once it starts.
						{:else}
							Every parameter is fixed once the run starts.
						{/if}
					</p>
				{/if}
				{#each extras as p (p.key)}
					<div class="extra">
						<span class="lbl">{p.label}{#if 'unit' in p && p.unit} ({p.unit}){/if}</span>
						<p class="aq-muted aq-small">{p.help}</p>
						{#if p.kind === 'per_channel'}
							<div class="aq-fields">
								{#each channelLabels as l, i (i)}
									<NumberField id="law-{p.key}-{i}" label="{p.label}, {l}" unit={p.unit} step={p.step} min={p.min} max={p.max} value={perChannel(p)[i]} onchange={(v) => (perChannel(p)[i] = v ?? NaN)} disabled={locked} />
								{/each}
							</div>
						{:else if p.kind === 'curve'}
							<div class="pts">
								{#each curve(p) as pt, k (k)}
									<div class="pt">
										<NumberField id="law-{p.key}-{k}-f" label="Point {k + 1} frequency" compact unit="Hz" bind:value={pt[0]} disabled={locked} />
										<NumberField id="law-{p.key}-{k}-v" label="Point {k + 1} value" compact unit={p.unit} bind:value={pt[1]} disabled={locked} />
										<button class="aq-icon-btn" aria-label="Remove point {k + 1}" disabled={locked || curve(p).length <= 1} onclick={() => curve(p).splice(k, 1)}>
											<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
										</button>
									</div>
								{/each}
								<button class="btn sm ghost" disabled={locked} onclick={() => curve(p).push([(curve(p).at(-1)?.[0] ?? 100) * 2, curve(p).at(-1)?.[1] ?? 0])}>Add Point</button>
							</div>
						{/if}
						{#if lawParamError(p, values[p.key])}<Findings items={[{ level: 'error', message: lawParamError(p, values[p.key]) ?? '' }]} />{/if}
					</div>
				{/each}
				<Findings items={Object.entries(errors).filter(([k]) => k.startsWith('law.params.')).flatMap(([, v]) => v).map((message) => ({ level: 'error' as const, message }))} />
			</div>
		{/if}
	{/if}
</section>

<style>
	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.mono {
		font-family: var(--font-mono);
	}

	.import {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		margin-bottom: var(--sp-3);
		padding: var(--sp-3);
		background: var(--bg-2);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.import p {
		grid-column: 1 / -1;
		margin: 0;
	}

	.laws {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.law {
		display: grid;
		grid-template-columns: 16px auto auto 1fr;
		grid-template-areas: 'dot name ver kind' '. sum sum sum';
		align-items: baseline;
		gap: 2px var(--sp-2);
		padding: 8px 10px;
		text-align: left;
		color: var(--text-1);
		background: var(--bg-0);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.law:hover:not(:disabled) {
		background: var(--bg-2);
	}

	.law.on {
		background: var(--blue-weak);
		border: 1.5px solid var(--blue-line);
	}

	.law .dot {
		grid-area: dot;
		align-self: center;
		display: inline-flex;
		color: var(--blue-2);
	}

	.law .name {
		grid-area: name;
		font-weight: var(--weight-strong);
	}

	.law .ver {
		grid-area: ver;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.law .kind {
		grid-area: kind;
		justify-self: end;
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.law .sum {
		grid-area: sum;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.details {
		margin-top: var(--sp-4);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--line-1);
	}

	.details h3 {
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.summary {
		margin: 4px 0 var(--sp-2);
		color: var(--text-2);
	}

	/* The rows a test type adds come from its own component, so the rules reach them globally. */
	.facts {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2) var(--sp-3);
		margin: 0 0 var(--sp-2);
	}

	.facts :global(.wide) {
		grid-column: 1 / -1;
	}

	.facts :global(dt) {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.facts :global(dd) {
		margin: 0;
		font-size: var(--text-sm);
	}

	.check {
		display: inline-flex;
		align-items: center;
		gap: 5px;
	}

	.check.failed,
	.check.unreadable {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.params {
		margin-top: var(--sp-4);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--line-1);
	}

	.extra {
		margin-top: var(--sp-3);
	}

	.live {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		margin: var(--sp-2) 0 0;
		color: var(--text-2);
	}

	.live .tag {
		padding: 1px 6px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		color: var(--blue-1);
		background: var(--blue-weak);
		border: 1px solid var(--blue-line);
		border-radius: 8px;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.pts {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}

	.pt {
		display: flex;
		align-items: center;
		gap: 4px;
	}
</style>
