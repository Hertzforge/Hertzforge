<script lang="ts">
	/**
	 * A panel built from an operation descriptor: number, integer, enum, boolean and string
	 * parameters, with units, ranges and dataset-driven options. Values bind out; the parent
	 * decides when to run. Errors are words beside the field, never a color alone.
	 */
	import type { ParamDescriptor, ParamOption } from '$lib/protocol/types';
	import { optionsFor, parseNumber, rangeHint, validate, type ParamValues } from './params';

	interface Props {
		params: ParamDescriptor[];
		values: ParamValues;
		channels?: ParamOption[];
		/** Show errors only after the user has touched a field or tried to run. */
		showAllErrors?: boolean;
		layout?: 'stack' | 'row';
		disabled?: boolean;
		idPrefix?: string;
		/** data-testid on each field is `<testidPrefix>-<param name>`. */
		testidPrefix?: string;
	}

	let {
		params,
		values = $bindable(),
		channels = [],
		showAllErrors = false,
		layout = 'stack',
		disabled = false,
		idPrefix = 'pf',
		testidPrefix = 'param'
	}: Props = $props();

	/** Text being typed into number fields, so "1." or "-" can sit there while editing. */
	let drafts = $state<Record<string, string>>({});
	let touched = $state<Record<string, boolean>>({});

	function errorFor(p: ParamDescriptor): string | null {
		if (!showAllErrors && !touched[p.name]) return null;
		return validate(p, values[p.name], channels);
	}

	function numberText(p: ParamDescriptor): string {
		if (p.name in drafts) return drafts[p.name];
		const v = values[p.name];
		return typeof v === 'number' && Number.isFinite(v) ? String(v) : '';
	}

	function onNumberInput(p: ParamDescriptor, text: string) {
		drafts[p.name] = text;
		values[p.name] = parseNumber(text);
	}

	function onNumberBlur(p: ParamDescriptor) {
		touched[p.name] = true;
		delete drafts[p.name];
	}

	function step(p: ParamDescriptor, dir: 1 | -1) {
		const s = p.step ?? (p.type === 'integer' ? 1 : 0.1);
		const cur = typeof values[p.name] === 'number' && Number.isFinite(values[p.name] as number) ? (values[p.name] as number) : (p.min ?? 0);
		let next = Number((cur + dir * s).toPrecision(12));
		if (p.min !== undefined) next = Math.max(p.min, next);
		if (p.max !== undefined) next = Math.min(p.max, next);
		if (p.type === 'integer') next = Math.round(next);
		values[p.name] = next;
		delete drafts[p.name];
		touched[p.name] = true;
	}

	function onNumberKey(p: ParamDescriptor, e: KeyboardEvent) {
		if (e.key === 'ArrowUp') {
			e.preventDefault();
			step(p, 1);
		} else if (e.key === 'ArrowDown') {
			e.preventDefault();
			step(p, -1);
		}
	}

	/** Short option lists read better as a segmented control than a menu. */
	function segmented(p: ParamDescriptor): boolean {
		const opts = optionsFor(p, channels);
		return opts.length > 0 && opts.length <= 3 && opts.every((o) => o.label.length <= 12);
	}
</script>

<div class="form {layout}">
	{#each params as p (p.name)}
		{@const id = `${idPrefix}-${p.name}`}
		{@const err = errorFor(p)}
		{@const hint = rangeHint(p)}
		{@const tid = `${testidPrefix}-${p.name}`}
		<div class="param" class:invalid={!!err} class:boolean={p.type === 'boolean'}>
			{#if p.type === 'boolean'}
				<label class="switch" for={id}>
					<input
						{id}
						type="checkbox"
						data-testid={tid}
						role="switch"
						checked={values[p.name] === true}
						{disabled}
						onchange={(e) => ((values[p.name] = e.currentTarget.checked), (touched[p.name] = true))}
					/>
					<span class="track" aria-hidden="true"><span class="thumb"></span></span>
					<span class="label">{p.label}</span>
					<span class="state">{values[p.name] === true ? 'On' : 'Off'}</span>
				</label>
			{:else}
				<label class="label" for={id}>
					{p.label}
					{#if hint}<span class="hint num">{hint}</span>{/if}
				</label>
				{#if p.type === 'number' || p.type === 'integer'}
					<div class="numeric">
						<input
							{id}
							class="field num"
							data-testid={tid}
							type="text"
							inputmode={p.type === 'integer' ? 'numeric' : 'decimal'}
							autocomplete="off"
							spellcheck="false"
							value={numberText(p)}
							{disabled}
							aria-invalid={!!err}
							aria-describedby={err ? `${id}-err` : undefined}
							oninput={(e) => onNumberInput(p, e.currentTarget.value)}
							onblur={() => onNumberBlur(p)}
							onkeydown={(e) => onNumberKey(p, e)}
						/>
						{#if p.unit}<span class="unit">{p.unit}</span>{/if}
						<div class="steppers">
							<button type="button" tabindex="-1" aria-label="Increase {p.label}" {disabled} onclick={() => step(p, 1)}>
								<svg viewBox="0 0 10 6" width="8" height="5" aria-hidden="true"><path d="M1 5l4-4 4 4" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
							</button>
							<button type="button" tabindex="-1" aria-label="Decrease {p.label}" {disabled} onclick={() => step(p, -1)}>
								<svg viewBox="0 0 10 6" width="8" height="5" aria-hidden="true"><path d="M1 1l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
							</button>
						</div>
					</div>
				{:else if p.type === 'enum'}
					{@const opts = optionsFor(p, channels)}
					{#if segmented(p)}
						<div class="seg" role="radiogroup" aria-labelledby={id} {id} data-testid={tid}>
							{#each opts as o (o.value)}
								<button
									type="button"
									role="radio"
									data-testid="{tid}-{o.value}"
									aria-checked={values[p.name] === o.value}
									class:on={values[p.name] === o.value}
									{disabled}
									onclick={() => ((values[p.name] = o.value), (touched[p.name] = true))}>{o.label}</button
								>
							{/each}
						</div>
					{:else}
						<select
							{id}
							data-testid={tid}
							class="field"
							{disabled}
							aria-invalid={!!err}
							value={values[p.name]}
							onchange={(e) => {
								const raw = e.currentTarget.value;
								const match = opts.find((o) => String(o.value) === raw);
								values[p.name] = match ? match.value : raw;
								touched[p.name] = true;
							}}
						>
							{#if opts.length === 0}<option value="">No options</option>{/if}
							{#each opts as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
						</select>
					{/if}
				{:else}
					<input
						{id}
						data-testid={tid}
						class="field"
						type="text"
						autocomplete="off"
						value={String(values[p.name] ?? '')}
						{disabled}
						aria-invalid={!!err}
						oninput={(e) => (values[p.name] = e.currentTarget.value)}
						onblur={() => (touched[p.name] = true)}
					/>
				{/if}
			{/if}
			<div class="err" id="{id}-err" aria-live="polite">
				{#if err}
					<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /><path d="M7 5.4v3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
					{err}
				{/if}
			</div>
		</div>
	{/each}
</div>

<style>
	.form.stack {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.form.row {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-start;
		gap: 0 var(--sp-4);
	}

	.form.row .param {
		min-width: 150px;
	}

	.param {
		display: flex;
		flex-direction: column;
		gap: 5px;
	}

	.label {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--sp-2);
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.hint {
		font-size: var(--text-2xs);
		font-weight: var(--weight-regular);
		color: var(--text-4);
	}

	.numeric {
		position: relative;
		display: flex;
		align-items: center;
	}

	.numeric .field {
		padding-right: 52px;
		text-align: right;
	}

	.numeric .unit {
		position: absolute;
		right: 24px;
		font-size: var(--text-xs);
		color: var(--text-3);
		pointer-events: none;
	}

	.steppers {
		position: absolute;
		right: 3px;
		top: 3px;
		bottom: 3px;
		display: flex;
		flex-direction: column;
		width: 16px;
	}

	.steppers button {
		flex: 1;
		display: grid;
		place-items: center;
		padding: 0;
		color: var(--text-3);
		background: transparent;
		border: 0;
		border-radius: 3px;
	}

	.steppers button:hover:not(:disabled) {
		color: var(--text-1);
		background: var(--bg-4);
	}

	.seg {
		display: flex;
		padding: 2px;
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.seg button {
		flex: 1;
		height: 24px;
		padding: 0 var(--sp-2);
		font-size: var(--text-xs);
		color: var(--text-2);
		background: transparent;
		border: 0;
		border-radius: 4px;
	}

	.seg button.on {
		color: var(--text-1);
		background: var(--bg-4);
		box-shadow: inset 0 0 0 1px var(--line-3);
	}

	.switch {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		height: var(--control-height);
		margin-top: 18px;
		cursor: pointer;
		font-size: var(--text-sm);
	}

	.form.stack .switch {
		margin-top: 4px;
	}

	.switch input {
		position: absolute;
		opacity: 0;
		width: 1px;
		height: 1px;
	}

	.track {
		position: relative;
		flex: none;
		width: 28px;
		height: 16px;
		background: var(--bg-4);
		border: 1px solid var(--line-3);
		border-radius: 9px;
		transition: background var(--dur-fast) var(--ease);
	}

	.thumb {
		position: absolute;
		top: 2px;
		left: 2px;
		width: 10px;
		height: 10px;
		background: var(--text-2);
		border-radius: 50%;
		transition: transform var(--dur-fast) var(--ease);
	}

	.switch input:checked + .track {
		background: var(--blue-2);
		border-color: var(--blue-2);
	}

	.switch input:checked + .track .thumb {
		transform: translateX(12px);
		background: var(--text-on-accent);
	}

	.switch input:focus-visible + .track {
		outline: 2px solid var(--focus);
		outline-offset: 2px;
	}

	.switch .label {
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.switch .state {
		margin-left: auto;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.err {
		display: flex;
		align-items: center;
		gap: 4px;
		min-height: 16px;
		font-size: var(--text-2xs);
		color: var(--amber-2);
	}
</style>
