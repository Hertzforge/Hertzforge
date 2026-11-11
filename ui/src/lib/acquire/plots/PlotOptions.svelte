<script lang="ts" module>
	/** The section last open, so the menu opens where it was left. */
	let lastSection = 'data';
</script>

<script lang="ts">
	/**
	 * A plot panel's options menu, in sections: Data, Axes, Display, Cursors and Marks, Hold,
	 * Export. A section, or a choice, that does not apply to the plot's type is not shown. Every
	 * change applies at once. Escape closes; focus stays in the menu while it is open and goes
	 * back where it was when it closes. It draws no frame of its own: it sits in the popover that
	 * opens it, which scrolls it when it is taller than the window.
	 */
	import { onDestroy, onMount, untrack } from 'svelte';
	import { MARKER_WORDS, TRACE_COLORS } from '$lib/viz/roles';
	import { withType } from './display';
	import { stepIndex } from './keys';
	import type { AxisDisplay, CursorMode, Dash, HoldMode, LineFamily, Marker, PartChoice, PlotDisplay, PlotPanelSpec, PlotType, TraceStyle } from './types';

	interface Props {
		spec: PlotPanelSpec;
		types: PlotType[];
		/** The test's channels, for the channel choice. */
		channels: { index: number; name: string }[];
		/** The test's parts, for a per-part plot's part choice. */
		parts?: PartChoice[];
		onchange: (spec: PlotPanelSpec) => void;
		onexport?: (what: 'copy' | 'image' | 'csv') => void;
		onclose: () => void;
		/** The lines the plot draws now (the plot body's `traces()`), for the trace list. */
		traces?: { name: string; role: string }[];
	}
	let { spec, types, channels, parts = [], onchange, onexport, onclose, traces = [] }: Props = $props();

	const d = $derived(spec.display);
	const type = $derived(types.find((t) => t.id === spec.quantity) ?? null);
	const time = $derived(type?.time ?? false);
	const numbers = $derived(type?.group === 'numbers');
	const uid = `po-${Math.random().toString(36).slice(2, 8)}`;

	type Section = 'data' | 'axes' | 'display' | 'cursors' | 'hold' | 'export';
	const sections = $derived(
		(
			[
				['data', 'Data', true],
				['axes', 'Axes', !numbers],
				['display', 'Display', !numbers],
				['cursors', 'Cursors', !numbers],
				['hold', 'Hold', !numbers && !time],
				['export', 'Export', true]
			] as [Section, string, boolean][]
		).filter(([, , on]) => on)
	);
	let section = $state<Section>(untrack(() => lastSection) as Section);
	const open = $derived(sections.some(([s]) => s === section) ? section : 'data');
	$effect(() => {
		lastSection = open;
	});

	function set(patch: Partial<PlotPanelSpec>) {
		onchange({ ...spec, ...patch });
	}
	function setD(patch: Partial<PlotDisplay>) {
		set({ display: { ...d, ...patch } });
	}
	function setAxis(k: 'x' | 'y', patch: Partial<AxisDisplay>) {
		setD({ [k]: { ...d[k], ...patch } });
	}

	// Data

	const GROUP_WORDS: Record<PlotType['group'], string> = { frequency: 'Frequency', time: 'Time', numbers: 'Numbers', other: 'Other' };
	const typeGroups = $derived((['frequency', 'time', 'numbers', 'other'] as const).map((g) => ({ g, types: types.filter((t) => t.group === g) })).filter((x) => x.types.length));

	function setType(id: string) {
		const t = types.find((x) => x.id === id);
		if (t) onchange(withType(spec, t));
	}

	const CHANNEL_MODES: [string, string][] = [
		['all', 'All'],
		['control', 'Control'],
		['limited', 'Limited'],
		['pick', 'Chosen']
	];
	/** The channel choices the plot's type takes. */
	const channelModes = $derived(CHANNEL_MODES.filter(([m]) => m === 'pick' || !type?.selectors || (type.selectors as string[]).includes(m)));
	const channelMode = $derived(Array.isArray(spec.channels) ? 'pick' : spec.channels);
	const picked = $derived(Array.isArray(spec.channels) ? spec.channels : []);
	function setChannelMode(m: string) {
		if (m === 'pick') set({ channels: picked.length ? picked : channels.slice(0, 1).map((c) => c.index) });
		else set({ channels: m as 'all' | 'control' | 'limited' });
	}
	function toggleChannel(index: number, on: boolean) {
		set({ channels: on ? [...new Set([...picked, index])].sort((a, b) => a - b) : picked.filter((i) => i !== index) });
	}

	/** The part choices the plot's type takes: its own kind of part, or both. */
	const partModes = $derived<[string, string][]>(
		type?.parts === 'tones' ? [['tones', 'Every Tone'], ['pick', 'Chosen']] : type?.parts === 'bands' ? [['bands', 'Every Narrowband'], ['pick', 'Chosen']] : [['all', 'Every Part'], ['tones', 'Tones'], ['bands', 'Narrowbands'], ['pick', 'Chosen']]
	);
	const partOffer = $derived(parts.filter((p) => type?.parts === 'both' || (type?.parts === 'tones' ? p.kind === 'tone' : p.kind === 'narrowband')));
	const partMode = $derived(Array.isArray(spec.parts) ? 'pick' : (spec.parts ?? 'all'));
	const pickedParts = $derived(Array.isArray(spec.parts) ? spec.parts : []);
	function setPartMode(m: string) {
		if (m === 'pick') set({ parts: pickedParts.length ? pickedParts : partOffer.slice(0, 1).map((p) => p.id) });
		else set({ parts: m as 'all' | 'tones' | 'bands' });
	}
	function togglePart(id: string, on: boolean) {
		const order = partOffer.map((p) => p.id);
		set({ parts: on ? order.filter((x) => x === id || pickedParts.includes(x)) : pickedParts.filter((x) => x !== id) });
	}

	function setTitle(text: string) {
		const t = text.trim();
		set({ title: t && t !== type?.words ? t : null });
	}

	// Axes

	// The range fields follow the spec's numbers, except a field being typed into whose number has
	// not changed: a half-typed range is not wiped by another choice.
	let fixedText = $state({ x: ['', ''], y: ['', ''] } as Record<'x' | 'y', [string, string]>);
	const synced: Record<'x' | 'y', [number | null | undefined, number | null | undefined]> = { x: [undefined, undefined], y: [undefined, undefined] };
	$effect(() => {
		const show = (v: number | null) => (v === null ? '' : String(Number(v.toPrecision(6))));
		for (const k of ['x', 'y'] as const) {
			const values = [d[k].min, d[k].max] as const;
			values.forEach((v, j) => {
				if (synced[k][j] === v) return;
				synced[k][j] = v;
				untrack(() => (fixedText[k][j] = show(v)));
			});
		}
	});

	function logAxis(k: 'x' | 'y'): boolean {
		return d[k].scale === 'log';
	}

	function rangeProblem(k: 'x' | 'y'): string | null {
		const [a, b] = fixedText[k];
		if (a.trim() === '' && b.trim() === '') return null;
		if (a.trim() === '' || b.trim() === '') return 'Enter both ends of the range.';
		const lo = Number(a);
		const hi = Number(b);
		if (!Number.isFinite(lo) || !Number.isFinite(hi)) return 'Enter numbers.';
		if (!(hi > lo)) return 'The top must be above the bottom.';
		if (logAxis(k) && lo <= 0) return 'A log axis needs a bottom above zero.';
		return null;
	}

	function commitFixed(k: 'x' | 'y') {
		if (rangeProblem(k) || fixedText[k][0].trim() === '') return;
		setAxis(k, { min: Number(fixedText[k][0]), max: Number(fixedText[k][1]) });
	}

	function setRange(k: 'x' | 'y', range: AxisDisplay['range']) {
		// held takes the range on screen now; the plot fills it in
		setAxis(k, range === 'held' ? { range, min: null, max: null } : { range });
	}

	const WINDOWS: [number | null, string][] = [
		[null, 'As Sent'],
		[0.5, '0.5 s'],
		[1, '1 s'],
		[2, '2 s'],
		[5, '5 s'],
		[10, '10 s'],
		[30, '30 s']
	];

	// Display

	const FAMILY_WORDS: Record<LineFamily, string> = {
		measured: 'Measured',
		reference: 'Reference',
		tolerance: 'Alarm Lines',
		abort: 'Abort Lines',
		limit: 'Limit Lines',
		drive: 'Drive',
		other: 'Other'
	};

	function toggleFamily(f: LineFamily, on: boolean) {
		setD({ hidden: on ? d.hidden.filter((x) => x !== f) : [...d.hidden, f] });
	}

	const NO_STYLE: TraceStyle = { visible: true, width: null, dash: null, color: null, marker: null };
	const traceNames = $derived([...new Set([...traces.map((t) => t.name), ...Object.keys(d.traces)])]);
	const isPlain = (t: TraceStyle) => t.visible && t.width === null && t.dash === null && t.color === null && (t.marker ?? null) === null;

	function setTrace(name: string, patch: Partial<TraceStyle>) {
		const next = { ...NO_STYLE, ...d.traces[name], ...patch };
		const all = { ...d.traces };
		if (isPlain(next)) delete all[name];
		else all[name] = next;
		setD({ traces: all });
	}

	const WIDTHS = [1, 1.5, 2, 3, 4];
	const DASH_WORDS: [Dash, string][] = [
		['solid', 'Solid'],
		['dashed', 'Dashed'],
		['dotted', 'Dotted'],
		['dashdot', 'Dash-Dot']
	];
	const MARKER_CHOICES: [Marker, string][] = [['none', 'None'], ...(Object.entries(MARKER_WORDS) as [Marker, string][]).map(([m, w]): [Marker, string] => [m, w[0].toUpperCase() + w.slice(1)])];

	let noteText = $state('');
	function addNote() {
		const text = noteText.trim();
		if (!text) return;
		const used = new Set(d.notes.map((n) => n.id));
		let k = d.notes.length + 1;
		while (used.has(`n${k}`)) k++;
		setD({ notes: [...d.notes, { id: `n${k}`, text, fx: 0.4, fy: 0.15 }] });
		noteText = '';
	}

	// Cursors and hold

	const CURSOR_WORDS: [CursorMode, string][] = [
		['none', 'None'],
		['one', 'One Cursor'],
		['two', 'Two Cursors'],
		['peak', 'Peak'],
		['peaks', 'Top Peaks'],
		['harmonics', 'Harmonics']
	];
	const HOLD_WORDS: [HoldMode, string][] = [
		['none', 'None'],
		['peak', 'Peak'],
		['min', 'Minimum'],
		['average', 'Average'],
		['exponential', 'Exponential']
	];
	const placed = $derived(d.cursorAt?.[0] != null || d.cursorAt?.[1] != null);

	// Keyboard: the tabs move with the arrows; Tab stays in the menu; Escape closes.

	let root: HTMLDivElement;
	let returnTo: HTMLElement | null = null;

	onMount(() => {
		returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
		root.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus();
	});
	onDestroy(() => {
		if (returnTo?.isConnected) returnTo.focus();
	});

	function focusables(): HTMLElement[] {
		return [...root.querySelectorAll<HTMLElement>('button, input, select, textarea, [tabindex]:not([tabindex="-1"])')].filter((el) => !(el as HTMLButtonElement).disabled && el.offsetParent !== null);
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			// not stopped here: a popover around the menu closes too and gives its button the keyboard
			e.preventDefault();
			onclose();
			return;
		}
		if (e.key !== 'Tab') return;
		const list = focusables();
		if (!list.length) return;
		const at = list.indexOf(document.activeElement as HTMLElement);
		if (e.shiftKey && at <= 0) {
			e.preventDefault();
			list[list.length - 1].focus();
		} else if (!e.shiftKey && at === list.length - 1) {
			e.preventDefault();
			list[0].focus();
		}
	}

	function onTabKey(e: KeyboardEvent, at: number) {
		const next = stepIndex(e.key, at, sections.length, 'horizontal');
		if (next === null) return;
		e.preventDefault();
		section = sections[next][0];
		root.querySelectorAll<HTMLElement>('[role="tab"]')[next]?.focus();
	}
</script>

{#snippet seg<T extends string | number | null>(label: string, options: [T, string][], value: T, choose: (v: T) => void, testid = '')}
	<div class="row">
		<span class="lbl">{label}</span>
		<div class="seg" role="radiogroup" aria-label={label} data-testid={testid || undefined}>
			{#each options as [v, words] (String(v))}
				<button type="button" role="radio" aria-checked={value === v} onclick={() => choose(v)}>{words}</button>
			{/each}
		</div>
	</div>
{/snippet}

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="opts" role="group" aria-label="Plot options" bind:this={root} {onkeydown} data-testid="plot-options">
	<div class="tabs" role="tablist" aria-label="Plot options">
		{#each sections as [id, words], i (id)}
			<button
				type="button"
				role="tab"
				id="{uid}-tab-{id}"
				aria-selected={open === id}
				aria-controls="{uid}-panel"
				tabindex={open === id ? 0 : -1}
				data-testid="plot-options-{id}"
				title={id === 'cursors' ? 'Cursors and Marks' : undefined}
				onclick={() => (section = id)}
				onkeydown={(e) => onTabKey(e, i)}>{words}</button
			>
		{/each}
	</div>

	<div class="panel" role="tabpanel" id="{uid}-panel" aria-labelledby="{uid}-tab-{open}">
		{#if open === 'data'}
			<div class="row">
				<label class="lbl" for="{uid}-type">Shows</label>
				<div class="col">
					<select id="{uid}-type" class="field sm" value={spec.quantity} onchange={(e) => setType(e.currentTarget.value)} data-testid="plot-options-type">
						{#each typeGroups as g (g.g)}
							<optgroup label={GROUP_WORDS[g.g]}>
								{#each g.types as t (t.id)}<option value={t.id}>{t.words}</option>{/each}
							</optgroup>
						{/each}
						{#if !type}<option value={spec.quantity}>{spec.quantity}</option>{/if}
					</select>
					{#if type?.help}<p class="help">{type.help}</p>{/if}
				</div>
			</div>
			{#if type?.parts}
				{@render seg('Parts', partModes, partMode, setPartMode, 'plot-options-parts')}
				{#if partMode === 'pick'}
					<ul class="picks" aria-label="Chosen parts">
						{#each partOffer as p (p.id)}
							<li>
								<label><input type="checkbox" checked={pickedParts.includes(p.id)} onchange={(e) => togglePart(p.id, e.currentTarget.checked)} data-testid="plot-options-part-{p.id}" /> {p.name}</label>
							</li>
						{:else}
							<li class="help">The setup has no parts of this kind to choose.</li>
						{/each}
					</ul>
				{/if}
			{/if}
			{#if type?.channels}
				{@render seg(
					'Channels',
					channelModes,
					channelMode,
					setChannelMode,
					'plot-options-channels'
				)}
				{#if channelMode === 'pick'}
					<ul class="picks" aria-label="Chosen channels">
						{#each channels as c (c.index)}
							<li>
								<label><input type="checkbox" checked={picked.includes(c.index)} onchange={(e) => toggleChannel(c.index, e.currentTarget.checked)} /> <span class="num">{c.index}</span> {c.name}</label>
							</li>
						{:else}
							<li class="help">The setup has no channels to choose.</li>
						{/each}
					</ul>
				{/if}
				{#if !time}
					{@render seg(
						'Panes',
						[
							['split', 'One per Channel'],
							['overlay', 'All on One']
						],
						spec.arrange,
						(v) => set({ arrange: v })
					)}
				{/if}
			{/if}
			{#if time}
				<div class="row">
					<label class="lbl" for="{uid}-points">Points</label>
					<select id="{uid}-points" class="field sm narrow" value={spec.points} onchange={(e) => set({ points: Number(e.currentTarget.value) })}>
						{#each [256, 512, 1024, 2048, 4096, 8192] as n (n)}<option value={n}>{n}</option>{/each}
						{#if ![256, 512, 1024, 2048, 4096, 8192].includes(spec.points)}<option value={spec.points}>{spec.points}</option>{/if}
					</select>
				</div>
			{/if}
			<div class="row">
				<label class="lbl" for="{uid}-title">Title</label>
				<input id="{uid}-title" class="field sm" value={spec.title ?? ''} placeholder={type?.words ?? spec.quantity} onchange={(e) => setTitle(e.currentTarget.value)} data-testid="plot-options-title" />
			</div>
		{:else if open === 'axes'}
			{#each ['x', 'y'] as const as k (k)}
				<h4 class="sub">{k === 'x' ? 'X Axis' : 'Y Axis'}</h4>
				{@render seg(
					'Scale',
					[
						['auto', 'Auto'],
						['log', 'Log'],
						['linear', 'Linear']
					],
					d[k].scale,
					(v) => setAxis(k, { scale: v }),
					`plot-options-${k}-scale`
				)}
				{@render seg(
					'Range',
					[
						['auto', 'Auto'],
						...(k === 'x' && !time ? [['band', 'Band'] as ['band', string]] : []),
						['fixed', 'Fixed'],
						['held', 'Held']
					],
					d[k].range,
					(v) => setRange(k, v),
					`plot-options-${k}-range`
				)}
				{#if d[k].range === 'fixed'}
					{@const problem = rangeProblem(k)}
					<div class="row">
						<span class="lbl"></span>
						<div class="col">
							<div class="pair">
								<input class="field sm num" aria-label="{k === 'x' ? 'X' : 'Y'} minimum" placeholder="Min" bind:value={fixedText[k][0]} onchange={() => commitFixed(k)} aria-invalid={problem !== null} />
								<span class="to">to</span>
								<input class="field sm num" aria-label="{k === 'x' ? 'X' : 'Y'} maximum" placeholder="Max" bind:value={fixedText[k][1]} onchange={() => commitFixed(k)} aria-invalid={problem !== null} />
							</div>
							{#if problem}<p class="help warn">{problem}</p>{/if}
						</div>
					</div>
				{:else if d[k].range === 'held'}
					<div class="row">
						<span class="lbl"></span>
						<div class="col held">
							<span class="num">{d[k].min === null || d[k].max === null ? 'Taking the range on screen' : `${fixedText[k][0]} to ${fixedText[k][1]}`}</span>
							<button type="button" class="btn ghost sm" onclick={() => setRange(k, 'held')}>Hold Here</button>
						</div>
					</div>
				{/if}
				{#if k === 'x' && time}
					<div class="row">
						<label class="lbl" for="{uid}-window">Window</label>
						<select id="{uid}-window" class="field sm narrow" value={String(d.window_s)} onchange={(e) => setD({ window_s: e.currentTarget.value === 'null' ? null : Number(e.currentTarget.value) })} data-testid="plot-options-window">
							{#each WINDOWS as [v, words] (String(v))}<option value={String(v)}>{words}</option>{/each}
						</select>
					</div>
				{/if}
				{#if k === 'y' && type?.units?.length}
					<div class="row">
						<label class="lbl" for="{uid}-unit">Units</label>
						<select id="{uid}-unit" class="field sm narrow" value={d.y.unit ?? ''} onchange={(e) => setAxis('y', { unit: e.currentTarget.value || null })}>
							<option value="">As Sent</option>
							{#each type.units as u (u.id)}<option value={u.id}>{u.words}</option>{/each}
						</select>
					</div>
				{/if}
				<div class="row">
					<label class="lbl" for="{uid}-{k}-label">Label</label>
					<input id="{uid}-{k}-label" class="field sm" value={d[k].label ?? ''} placeholder="As sent" onchange={(e) => setAxis(k, { label: e.currentTarget.value.trim() || null })} />
				</div>
				{@render seg(
					'Ticks',
					[
						['sparse', 'Sparse'],
						['normal', 'Normal'],
						['dense', 'Dense']
					],
					d[k].ticks,
					(v) => setAxis(k, { ticks: v })
				)}
			{/each}
		{:else if open === 'display'}
			{@render seg(
				'Grid',
				[
					['major', 'Major'],
					['minor', 'Major and Minor'],
					['none', 'None']
				],
				d.grid,
				(v) => setD({ grid: v }),
				'plot-options-grid'
			)}
			{@render seg(
				'Legend',
				[
					['inside', 'Inside'],
					['outside', 'Outside'],
					['off', 'Off']
				],
				d.legend,
				(v) => setD({ legend: v }),
				'plot-options-legend'
			)}
			{#if d.legend !== 'outside'}
				<div class="row">
					<label class="lbl" for="{uid}-corner">Corner</label>
					<select id="{uid}-corner" class="field sm narrow" value={d.legendCorner} onchange={(e) => setD({ legendCorner: e.currentTarget.value as PlotDisplay['legendCorner'] })}>
						<option value="top-left">Top Left</option>
						<option value="top-right">Top Right</option>
						<option value="bottom-left">Bottom Left</option>
						<option value="bottom-right">Bottom Right</option>
					</select>
				</div>
			{/if}
			<div class="row">
				<span class="lbl">Readout</span>
				<label class="check"><input type="checkbox" checked={d.readout} onchange={(e) => setD({ readout: e.currentTarget.checked })} data-testid="plot-options-readout" /> Show the cursor's values</label>
			</div>
			{#if type?.families?.length}
				<div class="row">
					<span class="lbl">Lines</span>
					<ul class="checks" aria-label="Lines">
						{#each type.families as f (f)}
							<li><label class="check"><input type="checkbox" checked={!d.hidden.includes(f)} onchange={(e) => toggleFamily(f, e.currentTarget.checked)} /> {FAMILY_WORDS[f]}</label></li>
						{/each}
					</ul>
				</div>
			{/if}
			{#if time}
				{@render seg(
					'Draw',
					[
						['waveform', 'Waveform'],
						['envelope', 'Envelope']
					],
					d.timeRender,
					(v) => setD({ timeRender: v })
				)}
				{#if type?.channels}
					{@render seg(
						'Channels',
						[
							['overlay', 'Overlaid'],
							['split', 'One Lane Each']
						],
						d.lanes,
						(v) => set({ arrange: v, display: { ...d, lanes: v } })
					)}
				{/if}
			{/if}

			<h4 class="sub">Traces</h4>
			{#if traceNames.length}
				<ul class="traces" data-testid="plot-options-traces">
					{#each traceNames as name (name)}
						{@const t = d.traces[name] ?? NO_STYLE}
						{@const colorName = TRACE_COLORS.find((c) => c.hex === t.color)?.words}
						<li>
							<div class="trace-head">
								<button type="button" class="btn ghost icon sm" aria-pressed={t.visible} aria-label="{t.visible ? 'Hide' : 'Show'} {name}" title={t.visible ? 'Shown' : 'Hidden'} onclick={() => setTrace(name, { visible: !t.visible })}>
									{#if t.visible}
										<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M1.5 8s2.4-4.5 6.5-4.5S14.5 8 14.5 8 12.1 12.5 8 12.5 1.5 8 1.5 8z" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="8" cy="8" r="2" fill="currentColor" /></svg>
									{:else}
										<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M1.5 8s2.4-4.5 6.5-4.5S14.5 8 14.5 8 12.1 12.5 8 12.5 1.5 8 1.5 8z" fill="none" stroke="currentColor" stroke-width="1.3" opacity="0.5" /><path d="M2.5 13.5l11-11" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
									{/if}
								</button>
								<span class="tname" class:off={!t.visible} title={name}>{name}</span>
								{#if d.traces[name]}<button type="button" class="btn ghost sm" onclick={() => setTrace(name, NO_STYLE)}>Reset</button>{/if}
							</div>
							<div class="trace-looks">
								<select class="field sm" aria-label="{name} width" value={t.width === null ? '' : String(t.width)} onchange={(e) => setTrace(name, { width: e.currentTarget.value ? Number(e.currentTarget.value) : null })}>
									<option value="">Width</option>
									{#each WIDTHS as w (w)}<option value={String(w)}>{w} px</option>{/each}
								</select>
								<select class="field sm" aria-label="{name} dash" value={t.dash ?? ''} onchange={(e) => setTrace(name, { dash: (e.currentTarget.value || null) as Dash | null })}>
									<option value="">Dash</option>
									{#each DASH_WORDS as [v, w] (v)}<option value={v}>{w}</option>{/each}
								</select>
								<select class="field sm" aria-label="{name} colour" value={t.color ?? ''} onchange={(e) => setTrace(name, { color: e.currentTarget.value || null })}>
									<option value="">Colour</option>
									{#each TRACE_COLORS as c (c.id)}<option value={c.hex}>{c.words}</option>{/each}
								</select>
								<select class="field sm" aria-label="{name} marker" value={t.marker ?? ''} onchange={(e) => setTrace(name, { marker: (e.currentTarget.value || null) as Marker | null })}>
									<option value="">Marker</option>
									{#each MARKER_CHOICES as [v, w] (v)}<option value={v}>{w}</option>{/each}
								</select>
							</div>
							{#if colorName}<span class="visually-hidden">{colorName}</span>{/if}
						</li>
					{/each}
				</ul>
			{:else}
				<p class="help">Appears when the plot draws its lines.</p>
			{/if}

			<h4 class="sub">Notes</h4>
			{#if d.notes.length}
				<ul class="notes">
					{#each d.notes as n (n.id)}
						<li>
							<input class="field sm" aria-label="Note" value={n.text} onchange={(e) => {
								const text = e.currentTarget.value.trim();
								setD({ notes: text ? d.notes.map((x) => (x.id === n.id ? { ...x, text } : x)) : d.notes.filter((x) => x.id !== n.id) });
							}} />
							<button type="button" class="btn ghost sm" onclick={() => setD({ notes: d.notes.filter((x) => x.id !== n.id) })}>Remove</button>
						</li>
					{/each}
				</ul>
			{/if}
			<form class="add-note" onsubmit={(e) => (e.preventDefault(), addNote())}>
				<input class="field sm" aria-label="New note" placeholder="New note" bind:value={noteText} maxlength="200" />
				<button type="submit" class="btn sm" disabled={!noteText.trim()}>Add Note</button>
			</form>
		{:else if open === 'cursors'}
			<div class="row">
				<span class="lbl">Show</span>
				<div class="radios" role="radiogroup" aria-label="Cursors and marks" data-testid="plot-options-cursor">
					{#each CURSOR_WORDS as [v, words] (v)}
						<label class="check"><input type="radio" name="{uid}-cursor" checked={d.cursor === v} onchange={() => setD({ cursor: v })} /> {words}</label>
					{/each}
				</div>
			</div>
			{#if d.cursor === 'peaks' || d.cursor === 'harmonics'}
				<div class="row">
					<label class="lbl" for="{uid}-count">{d.cursor === 'peaks' ? 'Peaks' : 'Harmonics'}</label>
					<select id="{uid}-count" class="field sm narrow" value={d.count} onchange={(e) => setD({ count: Number(e.currentTarget.value) })}>
						{#each [1, 2, 3, 4, 5, 6, 8, 10, 12, 16, 20] as n (n)}<option value={n}>{n}</option>{/each}
					</select>
				</div>
			{/if}
			{#if d.cursor === 'one' || d.cursor === 'two' || d.cursor === 'harmonics'}
				<div class="row">
					<span class="lbl"></span>
					<div class="col">
						<p class="help">Click the plot to place {d.cursor === 'two' ? 'A, then B' : 'the cursor'}.</p>
						{#if placed}<button type="button" class="btn ghost sm start" onclick={() => setD({ cursorAt: [null, null] })}>Clear Cursors</button>{/if}
					</div>
				</div>
			{/if}
		{:else if open === 'hold'}
			<div class="row">
				<span class="lbl">Hold</span>
				<div class="radios" role="radiogroup" aria-label="Hold" data-testid="plot-options-hold">
					{#each HOLD_WORDS as [v, words] (v)}
						<label class="check"><input type="radio" name="{uid}-hold" checked={d.hold === v} onchange={() => setD({ hold: v })} /> {words}</label>
					{/each}
				</div>
			</div>
			{#if d.hold === 'average' || d.hold === 'exponential'}
				<div class="row">
					<label class="lbl" for="{uid}-frames">Frames</label>
					<select id="{uid}-frames" class="field sm narrow" value={d.average} onchange={(e) => setD({ average: Number(e.currentTarget.value) })}>
						{#each [2, 4, 8, 16, 32, 64] as n (n)}<option value={n}>{n}</option>{/each}
						{#if ![2, 4, 8, 16, 32, 64].includes(d.average)}<option value={d.average}>{d.average}</option>{/if}
					</select>
				</div>
			{/if}
			{#if d.hold !== 'none'}
				<div class="row">
					<span class="lbl"></span>
					<button type="button" class="btn sm start" onclick={() => setD({ holdEpoch: (d.holdEpoch ?? 0) + 1 })} data-testid="plot-options-hold-reset">Reset</button>
				</div>
			{/if}
		{:else if open === 'export'}
			<div class="exports">
				<button type="button" class="btn sm" disabled={!onexport} onclick={() => onexport?.('copy')} data-testid="plot-options-copy">Copy Image</button>
				<button type="button" class="btn sm" disabled={!onexport} onclick={() => onexport?.('image')} data-testid="plot-options-save-image">Save Image</button>
				<button type="button" class="btn sm" disabled={!onexport} onclick={() => onexport?.('csv')} data-testid="plot-options-save-csv">Save CSV</button>
			</div>
		{/if}
	</div>

	<div class="foot">
		<button type="button" class="btn sm" onclick={onclose} data-testid="plot-options-done">Done</button>
	</div>
</div>

<style>
	.opts {
		display: flex;
		flex-direction: column;
		width: min(390px, calc(100vw - 40px));
		max-height: min(78vh, 600px);
		font-size: var(--text-xs);
		color: var(--text-1);
	}

	.tabs {
		display: flex;
		flex: none;
		gap: 2px;
		border-bottom: 1px solid var(--line-1);
		overflow-x: auto;
	}

	.tabs button {
		flex: none;
		height: 26px;
		padding: 0 9px;
		font-size: var(--text-xs);
		color: var(--text-2);
		background: transparent;
		border: 0;
		border-bottom: 2px solid transparent;
		cursor: pointer;
	}

	.tabs button:hover {
		color: var(--text-1);
	}

	.tabs button[aria-selected='true'] {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		border-bottom-color: var(--accent);
	}

	.tabs button:focus-visible {
		outline: 1px solid var(--focus);
		outline-offset: -1px;
	}

	.panel {
		flex: 1 1 auto;
		min-height: 0;
		overflow: auto;
		padding: var(--sp-2) 2px;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.foot {
		display: flex;
		flex: none;
		justify-content: flex-end;
		padding-top: var(--sp-2);
		border-top: 1px solid var(--line-1);
	}

	.row {
		display: grid;
		grid-template-columns: 70px minmax(0, 1fr);
		align-items: center;
		gap: var(--sp-2);
		min-height: 24px;
	}

	.lbl {
		color: var(--text-3);
	}

	.col {
		display: flex;
		flex-direction: column;
		gap: 3px;
		min-width: 0;
	}

	.col.held {
		flex-direction: row;
		align-items: center;
		justify-content: space-between;
		color: var(--text-2);
	}

	.sub {
		margin: var(--sp-2) 0 0;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-3);
	}

	.sub:first-child {
		margin-top: 0;
	}

	.help {
		margin: 0;
		color: var(--text-3);
		line-height: 1.35;
	}

	.help.warn {
		color: var(--status-warn);
	}

	.field.sm {
		height: 24px;
		font-size: var(--text-xs);
	}

	.field.narrow {
		width: auto;
		min-width: 110px;
		justify-self: start;
	}

	.seg {
		display: inline-flex;
		flex-wrap: wrap;
		justify-self: start;
		padding: 2px;
		background: var(--bg-1);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.seg button {
		height: 20px;
		padding: 0 7px;
		font-size: var(--text-xs);
		color: var(--text-2);
		background: transparent;
		border: 0;
		border-radius: 4px;
		white-space: nowrap;
		cursor: pointer;
	}

	.seg button[aria-checked='true'] {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		background: var(--bg-4);
		box-shadow: inset 0 0 0 1px var(--line-3);
	}

	.seg button:focus-visible {
		outline: 1px solid var(--focus);
	}

	.pair {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.pair .field {
		flex: 1;
		min-width: 0;
	}

	.to {
		color: var(--text-3);
	}

	.check {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		cursor: pointer;
	}

	.checks,
	.picks,
	.traces,
	.notes {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.checks {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 2px var(--sp-2);
	}

	.picks {
		max-height: 150px;
		overflow: auto;
		margin-left: 78px;
		padding: 4px 6px;
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-sm);
	}

	.radios {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 3px var(--sp-2);
	}

	.traces li {
		padding: 4px 0;
		border-bottom: 1px solid var(--line-1);
	}

	.trace-head {
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.tname {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.tname.off {
		color: var(--text-3);
		text-decoration: line-through;
	}

	.trace-looks {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: 4px;
		margin: 3px 0 0 28px;
	}

	.trace-looks .field {
		padding: 0 18px 0 5px;
		background-position: right 5px center;
	}

	.notes li,
	.add-note {
		display: flex;
		align-items: center;
		gap: 6px;
		margin: 0 0 4px;
	}

	.notes .field,
	.add-note .field {
		flex: 1;
		min-width: 0;
	}

	.exports {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
	}

	.start {
		justify-self: start;
		align-self: flex-start;
	}
</style>
