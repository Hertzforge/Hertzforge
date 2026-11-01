<script lang="ts" generics="V extends number | null">
	/**
	 * A number with its unit, for setup forms and table cells. Typing keeps the text as typed
	 * ("1.", "-"); the value is the parsed number, NaN while the text is not one. A nullable field
	 * carries an Off switch with its own words ("No clip"). Errors and warnings are words with a
	 * symbol beside the field, never a color alone.
	 */
	import { parseNumber } from '$lib/forms/params';

	interface Props {
		id: string;
		label: string;
		/** A number, or null when `nullable` is given. */
		value: V;
		unit?: string;
		/** Words for null ("No clip", "None"). Absent: the field always holds a number. */
		nullable?: string;
		step?: number;
		min?: number;
		max?: number;
		integer?: boolean;
		errors?: string[];
		warnings?: string[];
		help?: string;
		disabled?: boolean;
		/** Table cell: the label is for assistive technology only, and findings show as a mark. */
		compact?: boolean;
		testid?: string;
		/** Value to put back when an Off switch is turned on again. */
		fallback?: number;
		onchange?: (value: V) => void;
	}

	let {
		id,
		label,
		value = $bindable(),
		unit,
		nullable,
		step,
		min,
		max,
		integer = false,
		errors = [],
		warnings = [],
		help,
		disabled = false,
		compact = false,
		testid,
		fallback,
		onchange
	}: Props = $props();

	let draft = $state<string | null>(null);
	let remembered = $state<number | null>(null);

	const off = $derived(nullable !== undefined && value === null);
	const text = $derived(draft ?? (typeof value === 'number' && Number.isFinite(value) ? String(value) : ''));
	const invalid = $derived(errors.length > 0);
	const describedBy = $derived([errors.length || warnings.length ? `${id}-msg` : null, help && !compact ? `${id}-help` : null].filter(Boolean).join(' ') || undefined);

	function set(v: number | null) {
		value = v as V;
		onchange?.(v as V);
	}

	function onInput(raw: string) {
		draft = raw;
		set(parseNumber(raw));
	}

	function bump(dir: 1 | -1) {
		const s = step ?? (integer ? 1 : 0.1);
		const cur = typeof value === 'number' && Number.isFinite(value) ? value : (min ?? 0);
		let next = Number((cur + dir * s).toPrecision(12));
		if (min !== undefined) next = Math.max(min, next);
		if (max !== undefined) next = Math.min(max, next);
		if (integer) next = Math.round(next);
		draft = null;
		set(next);
	}

	function onKey(e: KeyboardEvent) {
		if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
			e.preventDefault();
			bump(e.key === 'ArrowUp' ? 1 : -1);
		}
	}

	function toggleOff(on: boolean) {
		if (on) {
			remembered = typeof value === 'number' && Number.isFinite(value) ? value : remembered;
			draft = null;
			set(null);
		} else {
			set(remembered ?? fallback ?? min ?? 0);
		}
	}
</script>

<div class="nf" class:compact class:invalid class:warned={!invalid && warnings.length > 0}>
	{#if !compact}
		<div class="top">
			<label for={id} class="lbl">{label}</label>
			{#if nullable !== undefined}
				<label class="off">
					<input type="checkbox" checked={off} {disabled} onchange={(e) => toggleOff(e.currentTarget.checked)} data-testid={testid ? `${testid}-off` : undefined} />
					{nullable}
				</label>
			{/if}
		</div>
	{/if}
	<div class="box" style:--unit-w={unit ? `${Math.max(1.6, unit.length * 0.62 + 0.5)}em` : undefined}>
		<input
			{id}
			class="field num"
			type="text"
			inputmode={integer ? 'numeric' : 'decimal'}
			autocomplete="off"
			spellcheck="false"
			value={off ? '' : text}
			placeholder={off ? nullable : undefined}
			disabled={disabled || off}
			aria-label={compact ? label : undefined}
			aria-invalid={invalid}
			aria-describedby={describedBy}
			data-testid={testid}
			oninput={(e) => onInput(e.currentTarget.value)}
			onblur={() => (draft = null)}
			onkeydown={onKey}
		/>
		{#if unit}<span class="unit">{unit}</span>{/if}
		{#if compact && (invalid || warnings.length)}
			<span class="cellmark" title={[...errors, ...warnings].join('\n')} aria-hidden="true">
				{#if invalid}
					<svg viewBox="0 0 14 14" width="12" height="12"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.7" stroke-linecap="round" /></svg>
				{:else}
					<svg viewBox="0 0 14 14" width="12" height="12"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /></svg>
				{/if}
			</span>
		{/if}
	</div>
	{#if errors.length || warnings.length}
		<ul class="msg" id="{id}-msg" class:visually-hidden={compact} aria-live="polite">
			{#each errors as m, i (i)}
				<li class="err">
					<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.7" stroke-linecap="round" /></svg>
					<span class="visually-hidden">Error:</span>{m}
				</li>
			{/each}
			{#each warnings as m, i (i)}
				<li class="warn">
					<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /></svg>
					<span class="visually-hidden">Warning:</span>{m}
				</li>
			{/each}
		</ul>
	{/if}
	{#if help && !compact}<p class="help" id="{id}-help">{help}</p>{/if}
</div>

<style>
	.nf {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}

	.top {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
		min-height: 18px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.off {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		font-size: var(--text-2xs);
		color: var(--text-3);
		cursor: pointer;
	}

	.off input {
		margin: 0;
		accent-color: var(--blue-2);
	}

	.box {
		position: relative;
		display: flex;
		align-items: center;
	}

	.field {
		text-align: right;
		padding-right: 10px;
	}

	.box:has(.unit) .field {
		padding-right: calc(12px + var(--unit-w, 3em));
	}

	.unit {
		position: absolute;
		right: 9px;
		max-width: 6em;
		overflow: hidden;
		font-size: var(--text-xs);
		color: var(--text-3);
		white-space: nowrap;
		pointer-events: none;
	}

	.compact .field {
		height: 26px;
		font-size: var(--text-xs);
	}

	.compact .box:has(.unit) .field {
		padding-right: calc(10px + var(--unit-w, 3em));
	}

	.cellmark {
		position: absolute;
		left: 5px;
		display: inline-flex;
		color: var(--amber-2);
	}

	.msg {
		margin: 0;
		padding: 0;
		list-style: none;
		font-size: var(--text-xs);
	}

	.msg li {
		display: flex;
		align-items: baseline;
		gap: 5px;
		line-height: 1.35;
	}

	.msg svg {
		flex: none;
		transform: translateY(1px);
	}

	.err {
		color: var(--amber-1);
	}

	.warn {
		color: var(--text-2);
	}

	.warn svg {
		color: var(--amber-2);
	}

	.help {
		font-size: var(--text-2xs);
		line-height: 1.4;
		color: var(--text-3);
	}

	.invalid .field {
		border-color: var(--amber-2);
		box-shadow: 0 0 0 1px var(--amber-line);
	}

	.compact.invalid .field,
	.compact.warned .field {
		padding-left: 22px;
	}

	.field:disabled {
		color: var(--text-3);
		background: var(--bg-1);
	}
</style>
