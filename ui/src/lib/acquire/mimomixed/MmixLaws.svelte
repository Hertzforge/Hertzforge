<script lang="ts">
	/**
	 * The five choices that decide how MIMO Mixed Mode controls. Two are laws from the service's
	 * catalog, each with its parameter form: the floor's law (from MIMO Random's laws) and the tones'
	 * law (from MIMO Sine's). Three are rules built into the service: how a narrowband is servoed,
	 * what happens when the drives' shared budget runs out, and what the floor's law is shown on the
	 * lines a tone or a band occupies. How best to control this test is an open question; the module
	 * is built so that laws and rules can be tried against each other.
	 */
	import LawPanel from '../law/LawPanel.svelte';
	import Findings from '../Findings.svelte';
	import { MIMO_MIXED_BAND_LAWS, MIMO_MIXED_CEILING_RULES, MIMO_MIXED_FILL_RULES } from '$lib/protocol/mimoMixed';
	import { BAND_LAW_WORDS, CEILING_RULE_WORDS, FILL_RULE_WORDS } from './mmixSetup';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const d = $derived(mm.setup);
	const locked = $derived(mm.locked);
	const E = $derived(mm.fieldErrors);
	const dry = $derived(mm.findings.errors.length === 0 ? (mm.check?.derived ?? null) : null);
	const errs = (k: string) => (E[k] ?? []).map((message) => ({ level: 'error' as const, message }));
</script>

<div class="stack" data-testid="mmix-laws">
	<p class="aq-small aq-muted lead">How best to control sine tones and narrowbands on a random floor across several shakers is still an open topic. This module is built for trying laws: choose each part's law and rule here, and compare runs.</p>

	<LawPanel
		heading="Floor Law"
		laws={mm.lawsOf('floor')}
		law={mm.floorLaw}
		chosenId={d.laws.floor.id}
		values={d.laws.floor.params}
		subtitle="From MIMO Random's laws. Called each loop on the floor's spectral matrix, on the lines no tone or servoed band occupies."
		{locked}
		connected={mm.connected}
		busy={mm.busy}
		problems={mm.lawProblems}
		errors={mm.floorLawErrors}
		channelLabels={mm.controlLabels}
		onchoose={(id) => mm.chooseLaw('floor', id)}
		onimport={(path) => mm.importLaw('floor', path)}
	>
		{#snippet facts()}
			{#if dry}<div><dt>On hardware</dt><dd data-testid="mmix-floor-law-hw">{dry.law.floor.hardware ? 'May drive hardware in this role' : 'Runs on the simulator only in this role'}</dd></div>{/if}
		{/snippet}
	</LawPanel>

	<LawPanel
		heading="Tone Law"
		laws={mm.lawsOf('tones')}
		law={mm.toneLaw}
		chosenId={d.laws.tones.id}
		values={d.laws.tones.params}
		subtitle="From MIMO Sine's laws. One instance per tone, called each tick on that tone's amplitudes and phases at every control."
		{locked}
		connected={mm.connected}
		busy={mm.busy}
		problems={mm.lawProblems}
		errors={mm.toneLawErrors}
		channelLabels={mm.controlLabels}
		onchoose={(id) => mm.chooseLaw('tones', id)}
		onimport={(path) => mm.importLaw('tones', path)}
		live={dry?.law.tones.live}
	>
		{#snippet facts()}
			{#if dry}<div><dt>On hardware</dt><dd data-testid="mmix-tone-law-hw">{dry.law.tones.hardware ? 'May drive hardware in this role' : 'Runs on the simulator only in this role'}</dd></div>{/if}
		{/snippet}
	</LawPanel>

	{#snippet rule(id: string, title: string, sub: string, list: readonly string[], words: Record<string, { label: string; help: string }>, value: string, set: (v: string) => void, key: string)}
		<section class="aq-panel" aria-labelledby="mmix-{id}" data-testid="mmix-{id}">
			<div class="aq-head"><h2 id="mmix-{id}">{title}</h2><span class="aq-sub">{sub}</span></div>
			<div class="choices" role="radiogroup" aria-labelledby="mmix-{id}">
				{#each list as v (v)}
					<label class="choice" class:on={value === v}>
						<input type="radio" name="mmix-{id}" checked={value === v} disabled={locked} onchange={() => set(v)} data-testid="mmix-{id}-{v}" />
						<span><strong>{words[v].label}</strong><span class="aq-small aq-muted help">{words[v].help}</span></span>
					</label>
				{/each}
			</div>
			<Findings items={errs(key)} />
		</section>
	{/snippet}

	{@render rule('band-law', 'Band Law', 'How each narrowband is held to its statement.', MIMO_MIXED_BAND_LAWS, BAND_LAW_WORDS, d.laws.bands, (v) => (d.laws.bands = v as typeof d.laws.bands), 'laws.bands')}
	{@render rule('ceiling-rule', 'Ceiling Rule', "What happens when a drive's shared budget runs out.", MIMO_MIXED_CEILING_RULES, CEILING_RULE_WORDS, d.laws.ceiling_rule, (v) => (d.laws.ceiling_rule = v as typeof d.laws.ceiling_rule), 'laws.ceiling_rule')}
	{@render rule('fill-rule', 'Fill Rule', "What the floor's law is shown on the lines a tone or a servoed band occupies.", MIMO_MIXED_FILL_RULES, FILL_RULE_WORDS, d.laws.fill_rule, (v) => (d.laws.fill_rule = v as typeof d.laws.fill_rule), 'laws.fill_rule')}
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.lead {
		margin: 0;
	}

	.choices {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.choice {
		display: flex;
		gap: var(--sp-2);
		align-items: flex-start;
		cursor: pointer;
	}

	.choice > span {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.help {
		max-width: 70ch;
	}
</style>
