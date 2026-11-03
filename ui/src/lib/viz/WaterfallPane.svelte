<script lang="ts">
	import { copyCanvas, plotBackground } from './copyImage';
	/**
	 * Waterfall view, drawn with three.js: every record a polyline at its own depth, colored by
	 * level on the sequential colormap (viridis), the way Visual Dynamics' 3-D data view draws time
	 * histories, FRFs and PSDs.
	 *
	 * All records are one geometry and one draw call, whatever their number. The raw values go to
	 * the GPU once as attributes (abscissa, level, record); the vertex shader maps them onto the
	 * stage and takes log10 of a log axis, and the fragment shader looks the level up in the
	 * colormap, so orbiting and switching an axis between linear and log move no data. Only the
	 * segment index is rebuilt on a scale change, because a level at or below zero is a gap on a
	 * log axis. Frames are drawn on demand: a still scene costs nothing.
	 *
	 * Hovering reads the point under the cursor by drawing the curves once more, into a few pixels
	 * around it, with each vertex writing its own index as its color; one small draw however many
	 * points there are, where a search on the CPU would visit every segment.
	 */
	import { onDestroy, onMount } from 'svelte';
	import {
		BufferAttribute,
		BufferGeometry,
		Color,
		DataTexture,
		GLSL3,
		LinearFilter,
		LineBasicMaterial,
		LineSegments,
		NoBlending,
		PerspectiveCamera,
		RawShaderMaterial,
		RGBAFormat,
		Scene,
		Vector3,
		WebGLRenderer,
		WebGLRenderTarget,
		type Material
	} from 'three';
	import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
	import type { ParamValues } from '$lib/forms/params';
	import type { WaterfallView } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { colormap, colormapGradient } from './colormap';
	import { formatCount, formatNumber, formatTick, withUnit } from './format';
	import { STAGE, axisTicks, fraction, levelRange, namedRecords, pickedPoint, recordOf, segmentIndex } from './waterfall';

	interface Props {
		view: WaterfallView;
		/** Run the view's operation again with these params: a quantity chosen here. */
		onparams?: (patch: ParamValues) => void;
	}

	let { view, onparams }: Props = $props();

	/** Side of the square read back around the cursor, in pixels: lines are one pixel wide. */
	const PICK = 9;

	type Facing = 'perspective' | 'top' | 'front';

	let canvasHost: HTMLDivElement;
	let overlay: HTMLDivElement;

	let logX = $state(false);
	let logZ = $state(false);
	let facing = $state<Facing>('perspective');
	let failure = $state<string | null>(null);
	/** The point under the cursor and where it stands on screen, or null. */
	let hover = $state<{ point: number; record: number; sx: number; sy: number } | null>(null);

	// The service says how each axis reads; the viewer may change it afterward.
	$effect.pre(() => {
		logX = view.x.scale === 'log';
		logZ = view.z.scale === 'log';
	});

	const recordCount = $derived(view.records.length);
	const pointCount = $derived(view.x_values.length);
	const xRange = $derived.by((): [number, number] => {
		const [a, b] = view.x_range;
		if (!logX) return a < b ? [a, b] : [a, a + 1];
		// a log abscissa starts at the first positive value drawn
		let lo = Infinity;
		const x = view.x_values;
		for (let i = 0; i < x.length; i++) if (x[i] > 0 && x[i] < lo) lo = x[i];
		const top = b > 0 ? b : 1;
		return [Math.min(Number.isFinite(lo) ? lo : top / 10, top / 1.0001), top];
	});
	const zRange = $derived(levelRange(view.z_values, logZ, view.z_range));
	const xTicks = $derived(axisTicks(xRange, logX).filter((t) => t >= xRange[0] && t <= xRange[1]));
	const zTicks = $derived(axisTicks(zRange, logZ).filter((t) => t >= zRange[0] * (1 - 1e-9) && t <= zRange[1] * (1 + 1e-9)));
	const named = $derived(namedRecords(recordCount));

	// three.js objects (not reactive)
	let renderer: WebGLRenderer | null = null;
	let scene: Scene;
	let camera: PerspectiveCamera;
	let controls: OrbitControls;
	let geometry: BufferGeometry;
	let curves: LineSegments;
	let frame: LineSegments | null = null;
	let colorTexture: DataTexture;
	const materials: Material[] = [];
	const uniforms = {
		uX: { value: [0, 1] },
		uZ: { value: [0, 1] },
		uXLog: { value: 0 },
		uZLog: { value: 0 },
		uDepth: { value: 0 },
		uMap: { value: null as DataTexture | null }
	};
	let raf = 0;
	let dirty = true;
	let curveMaterial: RawShaderMaterial;
	let pickMaterial: RawShaderMaterial;
	let pickTarget: WebGLRenderTarget;
	const pickPixels = new Uint8Array(4 * PICK * PICK);
	/** A cursor position waiting to be picked at the next frame, in canvas pixels. */
	let pendingPick: [number, number] | null = null;
	let records = new Float32Array(0);
	let resizeObs: ResizeObserver | null = null;
	const centre = new Vector3(STAGE.x / 2, STAGE.y / 2, STAGE.z / 2);

	const vertexShader = /* glsl */ `
		#ifdef PICKING
		flat out uint vId;
		#endif
		in float ax;
		in float az;
		in float arec;
		uniform mat4 modelViewMatrix;
		uniform mat4 projectionMatrix;
		uniform vec2 uX;
		uniform vec2 uZ;
		uniform float uXLog;
		uniform float uZLog;
		uniform float uDepth;
		out float vLevel;
		const vec3 S = vec3(${STAGE.x}, ${STAGE.y}, ${STAGE.z});
		void main() {
			float x = uXLog > 0.5 ? log2(ax) * 0.30102999566 : ax;
			float z = uZLog > 0.5 ? log2(az) * 0.30102999566 : az;
			float tx = (x - uX.x) / (uX.y - uX.x);
			float tz = clamp((z - uZ.x) / (uZ.y - uZ.x), 0.0, 1.0);
			float depth = uDepth > 0.0 ? arec / uDepth : 0.0;
			vLevel = tz;
			#ifdef PICKING
			vId = uint(gl_VertexID) + 1u;
			#endif
			gl_Position = projectionMatrix * modelViewMatrix * vec4(tx * S.x, depth * S.y, tz * S.z, 1.0);
		}`;

	// The colormap's stops are sRGB and the canvas is sRGB, so the looked-up color is written as is.
	const fragmentShader = /* glsl */ `
		precision highp float;
		uniform sampler2D uMap;
		in float vLevel;
		out vec4 outColor;
		void main() {
			outColor = vec4(texture(uMap, vec2(vLevel, 0.5)).rgb, 1.0);
		}`;

	// a vertex's index, plus one so that zero is "nothing here", in four bytes
	const pickFragmentShader = /* glsl */ `
		precision highp float;
		flat in uint vId;
		out vec4 outColor;
		void main() {
			outColor = vec4(uvec4(vId, vId >> 8u, vId >> 16u, vId >> 24u) & 255u) / 255.0;
		}`;

	function mapTexture(): DataTexture {
		const n = 256;
		const data = new Uint8Array(4 * n);
		const rgb: [number, number, number] = [0, 0, 0];
		for (let i = 0; i < n; i++) {
			colormap(i / (n - 1), rgb);
			data[4 * i] = Math.round(rgb[0] * 255);
			data[4 * i + 1] = Math.round(rgb[1] * 255);
			data[4 * i + 2] = Math.round(rgb[2] * 255);
			data[4 * i + 3] = 255;
		}
		const texture = new DataTexture(data, n, 1, RGBAFormat);
		texture.magFilter = LinearFilter;
		texture.minFilter = LinearFilter;
		texture.needsUpdate = true;
		return texture;
	}

	function span(range: [number, number], log: boolean): [number, number] {
		return log ? [Math.log10(range[0]), Math.log10(range[1])] : range;
	}

	function applyScales() {
		if (!renderer) return;
		uniforms.uX.value = span(xRange, logX);
		uniforms.uZ.value = span(zRange, logZ);
		uniforms.uXLog.value = logX ? 1 : 0;
		uniforms.uZLog.value = logZ ? 1 : 0;
		geometry.setIndex(new BufferAttribute(segmentIndex(view.offsets, view.x_values, view.z_values, logX, logZ), 1));
		buildFrame();
		dirty = true;
	}

	/** The floor, the back wall and the left wall, with a grid line at every tick. */
	function buildFrame() {
		if (frame) {
			scene.remove(frame);
			frame.geometry.dispose();
		}
		const p: number[] = [];
		const seg = (a: number[], b: number[]) => p.push(...a, ...b);
		const { x: X, y: Y, z: Z } = STAGE;
		seg([0, 0, 0], [X, 0, 0]);
		seg([X, 0, 0], [X, Y, 0]);
		seg([X, Y, 0], [0, Y, 0]);
		seg([0, Y, 0], [0, 0, 0]);
		seg([0, Y, 0], [0, Y, Z]);
		seg([X, Y, 0], [X, Y, Z]);
		seg([0, Y, Z], [X, Y, Z]);
		for (const t of xTicks) {
			const f = fraction(t, xRange, logX) * X;
			seg([f, 0, 0], [f, Y, 0]);
			seg([f, Y, 0], [f, Y, Z]);
		}
		for (const t of zTicks) {
			const f = fraction(t, zRange, logZ) * Z;
			seg([0, Y, f], [X, Y, f]);
			seg([X, Y, f], [X + 0.012, Y, f]);
		}
		const g = new BufferGeometry();
		g.setAttribute('position', new BufferAttribute(new Float32Array(p), 3));
		frame = new LineSegments(g, frameMaterial);
		frame.renderOrder = -1;
		scene.add(frame);
	}

	let frameMaterial: LineBasicMaterial;

	function applyTheme() {
		if (!renderer) return;
		frameMaterial.color = new Color(cssVar('--line-2'));
		dirty = true;
	}

	/** The distance at which the whole stage fits the pane, in whichever direction is tighter. */
	function fitDistance(): number {
		const radius = 0.5 * Math.hypot(STAGE.x, STAGE.y, STAGE.z);
		const vertical = (camera.fov * Math.PI) / 360;
		const horizontal = Math.atan(Math.tan(vertical) * camera.aspect);
		return (radius / Math.sin(Math.min(vertical, horizontal))) * 1.12;
	}

	function face(which: Facing) {
		facing = which;
		if (!renderer) return;
		const d = fitDistance();
		controls.target.copy(centre);
		if (which === 'top') {
			camera.position.set(centre.x, centre.y - 0.0001, centre.z + d);
			camera.up.set(0, 1, 0);
		} else if (which === 'front') {
			camera.position.set(centre.x, centre.y - d, centre.z);
			camera.up.set(0, 0, 1);
		} else {
			camera.position.copy(centre).addScaledVector(new Vector3(0.62, -1.25, 0.72).normalize(), d);
			camera.up.set(0, 0, 1);
		}
		controls.update();
		dirty = true;
	}

	function init() {
		try {
			renderer = new WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'high-performance' });
		} catch {
			failure = 'The waterfall needs WebGL, which is not available in this browser.';
			return;
		}
		renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
		renderer.setClearColor(0x000000, 0);
		canvasHost.appendChild(renderer.domElement);
		renderer.domElement.addEventListener('webglcontextlost', (e) => {
			e.preventDefault();
			failure = 'The graphics context was lost. Switch workspaces and back to rebuild the view.';
		});

		scene = new Scene();
		camera = new PerspectiveCamera(30, 1, 0.01, 100);

		geometry = new BufferGeometry();
		geometry.setAttribute('ax', new BufferAttribute(view.x_values, 1));
		geometry.setAttribute('az', new BufferAttribute(view.z_values, 1));
		records = recordOf(view.offsets, view.x_values.length);
		geometry.setAttribute('arec', new BufferAttribute(records, 1));
		colorTexture = mapTexture();
		uniforms.uMap.value = colorTexture;
		uniforms.uDepth.value = Math.max(0, recordCount - 1);
		curveMaterial = new RawShaderMaterial({ glslVersion: GLSL3, vertexShader, fragmentShader, uniforms });
		materials.push(curveMaterial);
		curves = new LineSegments(geometry, curveMaterial);
		curves.frustumCulled = false;
		scene.add(curves);
		pickMaterial = new RawShaderMaterial({
			glslVersion: GLSL3,
			vertexShader,
			fragmentShader: pickFragmentShader,
			uniforms,
			defines: { PICKING: 1 },
			blending: NoBlending
		});
		materials.push(pickMaterial);
		pickTarget = new WebGLRenderTarget(PICK, PICK);

		frameMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.8 });
		materials.push(frameMaterial);

		controls = new OrbitControls(camera, renderer.domElement);
		controls.enableDamping = true;
		controls.dampingFactor = 0.12;
		controls.rotateSpeed = 0.8;
		controls.zoomSpeed = 0.9;
		controls.addEventListener('change', () => (dirty = true));

		applyTheme();
		applyScales();

		resizeObs = new ResizeObserver(resize);
		resizeObs.observe(canvasHost);
		resize();
		face(facing);
		raf = requestAnimationFrame(loop);
	}

	function resize() {
		if (!renderer) return;
		const w = Math.max(1, canvasHost.clientWidth);
		const h = Math.max(1, canvasHost.clientHeight);
		renderer.setSize(w, h, false);
		renderer.domElement.style.width = '100%';
		renderer.domElement.style.height = '100%';
		camera.aspect = w / h;
		camera.updateProjectionMatrix();
		dirty = true;
	}

	const probe = new Vector3();
	/** Closest two record labels may stand on screen, in pixels; nearer ones are skipped. */
	const LABEL_GAP_PX = 12;

	/**
	 * Pin each HTML label over its point on the stage. Record and level labels are thinned by where
	 * they land on screen, not only by count: forty names along a depth axis seen end-on, or a
	 * decade every few pixels on a short level axis, would overprint.
	 */
	function placeLabels() {
		if (!overlay) return;
		const w = canvasHost.clientWidth;
		const h = canvasHost.clientHeight;
		const last: Record<string, [number, number]> = {};
		for (const el of overlay.querySelectorAll<HTMLElement>('[data-at]')) {
			const [x, y, z] = el.dataset.at!.split(',').map(Number);
			probe.set(x, y, z).project(camera);
			const sx = ((probe.x + 1) / 2) * w;
			const sy = ((1 - probe.y) / 2) * h;
			let visible = probe.z < 1;
			const thinned = el.classList.contains('rec') ? 'rec' : el.classList.contains('z') ? 'z' : null;
			if (visible && thinned) {
				const [lx, ly] = last[thinned] ?? [-Infinity, -Infinity];
				if (Math.hypot(sx - lx, sy - ly) < LABEL_GAP_PX) visible = false;
				else last[thinned] = [sx, sy];
			}
			el.style.visibility = visible ? 'visible' : 'hidden';
			el.style.transform = `translate(${sx}px, ${sy}px)`;
		}
	}

	/** Draw the curves alone around (px, py) with the picking material and read back what is there. */
	function pick(px: number, py: number) {
		if (!renderer) return;
		const w = canvasHost.clientWidth;
		const h = canvasHost.clientHeight;
		const half = (PICK - 1) / 2;
		camera.setViewOffset(w, h, px - half, py - half, PICK, PICK);
		curves.material = pickMaterial;
		if (frame) frame.visible = false;
		renderer.setRenderTarget(pickTarget);
		renderer.clear();
		renderer.render(scene, camera);
		renderer.setRenderTarget(null);
		renderer.readRenderTargetPixels(pickTarget, 0, 0, PICK, PICK, pickPixels);
		curves.material = curveMaterial;
		if (frame) frame.visible = true;
		camera.clearViewOffset();
		const point = pickedPoint(pickPixels, PICK);
		if (point === null || point >= records.length) {
			hover = null;
			return;
		}
		const record = records[point];
		probe
			.set(
				fraction(view.x_values[point], xRange, logX) * STAGE.x,
				recordCount > 1 ? (record / (recordCount - 1)) * STAGE.y : 0,
				Math.min(1, Math.max(0, fraction(view.z_values[point], zRange, logZ))) * STAGE.z
			)
			.project(camera);
		hover = { point, record, sx: ((probe.x + 1) / 2) * w, sy: ((1 - probe.y) / 2) * h };
	}

	function onPointerMove(e: PointerEvent) {
		// not while orbiting, when the point under the cursor changes every frame anyway
		if (e.buttons) {
			hover = null;
			return;
		}
		const box = canvasHost.getBoundingClientRect();
		pendingPick = [e.clientX - box.left, e.clientY - box.top];
	}

	function loop() {
		raf = requestAnimationFrame(loop);
		if (!renderer) return;
		const moved = controls.update();
		if (moved && hover) hover = null;
		if (pendingPick && !moved) {
			pick(...pendingPick);
			pendingPick = null;
		}
		if (!dirty && !moved) return;
		dirty = false;
		renderer.render(scene, camera);
		placeLabels();
	}

	onMount(init);

	$effect(() => {
		void logX;
		void logZ;
		void xRange;
		void zRange;
		applyScales();
	});

	$effect(() => {
		void theme.version;
		applyTheme();
	});

	// Labels are rendered by Svelte; place them once the DOM has them.
	$effect(() => {
		void xTicks;
		void zTicks;
		void named;
		queueMicrotask(() => {
			placeLabels();
		});
	});

	onDestroy(() => {
		cancelAnimationFrame(raf);
		resizeObs?.disconnect();
		if (!renderer) return;
		controls.dispose();
		geometry.dispose();
		frame?.geometry.dispose();
		colorTexture.dispose();
		pickTarget.dispose();
		for (const m of materials) m.dispose();
		renderer.dispose();
		renderer.forceContextLoss();
		renderer.domElement.remove();
		renderer = null;
	});

	function onKey(e: KeyboardEvent) {
		if (e.target instanceof HTMLInputElement || e.metaKey || e.ctrlKey || e.altKey) return;
		const k = e.key.toLowerCase();
		if (k === 'r') face('perspective');
		else if (k === 't') face('top');
		else if (k === 'f') face('front');
		else if (k === 'x') logX = !logX;
		else if (k === 'l') logZ = !logZ;
		else return;
		e.preventDefault();
	}

	function tickText(v: number): string {
		return formatTick(v);
	}

	const legendTicks = $derived.by(() => {
		const [lo, hi] = zRange;
		const mid = logZ ? Math.sqrt(lo * hi) : (lo + hi) / 2;
		return [lo, mid, hi].map((v) => formatNumber(v, 3));
	});

	const xAt = (t: number) => `${fraction(t, xRange, logX) * STAGE.x},${-0.035},0`;
	// the level axis stands at the back right, where a spectrum's levels are usually lowest, so
	// the curves rarely pass in front of its labels
	const zAt = (t: number) => `${STAGE.x + 0.02},${STAGE.y},${fraction(t, zRange, logZ) * STAGE.z}`;
	const recordAt = (r: number) => `${STAGE.x + 0.03},${recordCount > 1 ? (r / (recordCount - 1)) * STAGE.y : 0},0`;

	let copiedImage = $state<'copied' | 'saved' | null>(null);
	/** The view as it stands, as an image on the clipboard: drawn and copied in one task. */
	async function copyImage() {
		const drawer = renderer;
		if (!drawer) return;
		copiedImage = await copyCanvas(drawer.domElement, plotBackground(drawer.domElement), () => drawer.render(scene, camera), view.title);
		if (copiedImage) setTimeout(() => (copiedImage = null), 1500);
	}
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
	class="waterfall"
	data-testid="view-waterfall"
	tabindex="0"
	role="group"
	aria-label="{view.title}. {recordCount} records. R resets the view, T looks from the top, F from the front, X and L switch the abscissa and the level between linear and log."
	onkeydown={onKey}
>
	<!-- svelte-ignore a11y_no_static_element_interactions (hovering only reads; the keyboard has the view's own keys) -->
	<div
		class="canvas"
		data-testid="waterfall-canvas"
		bind:this={canvasHost}
		onpointermove={onPointerMove}
		onpointerleave={() => ((pendingPick = null), (hover = null))}
	></div>

	{#if hover}
		<div class="marker" style:transform="translate({hover.sx}px, {hover.sy}px)" aria-hidden="true"></div>
		<div class="readout num" data-testid="waterfall-readout" style:transform="translate({hover.sx + 12}px, {hover.sy - 30}px)">
			<span class="rname">{view.records[hover.record]}</span>
			<span>{formatNumber(view.x_values[hover.point], 5)}{view.x.unit ? ` ${view.x.unit}` : ''}</span>
			<span>{formatNumber(view.z_values[hover.point], 4)}{view.z.unit ? ` ${view.z.unit}` : ''}</span>
		</div>
	{/if}

	<div class="overlay" bind:this={overlay} aria-hidden="true">
		{#each xTicks as t (t)}
			<span class="tick x num" data-at={xAt(t)}>{tickText(t)}</span>
		{/each}
		{#if facing !== 'top'}
			<!-- seen from above, every level lands on one point -->
			{#each zTicks as t (t)}
				<span class="tick z num" data-at={zAt(t)}>{tickText(t)}</span>
			{/each}
		{/if}
		{#each named as r (r)}
			<span class="tick rec" data-at={recordAt(r)}>{view.records[r]}</span>
		{/each}
		<span class="title" data-at="{STAGE.x / 2},-0.11,0">{withUnit(view.x.label, view.x.unit)}</span>
	</div>

	{#if failure}
		<div class="failure" role="alert">{failure}</div>
	{/if}

	<div class="top">
		<div class="toolbar" role="toolbar" aria-label="Waterfall display">
			<button class="btn ghost sm copyimg" data-testid="waterfall-copy-image" onclick={copyImage} title="Copy the 3-D view as an image">{copiedImage === 'copied' ? 'Copied' : copiedImage === 'saved' ? 'Saved as PNG' : 'Copy Image'}</button>
			<div class="seg" role="radiogroup" aria-label="Viewpoint">
				<button role="radio" data-testid="waterfall-perspective" aria-checked={facing === 'perspective'} class:on={facing === 'perspective'} onclick={() => face('perspective')} title="Perspective (R)">3-D</button>
				<button role="radio" data-testid="waterfall-top" aria-checked={facing === 'top'} class:on={facing === 'top'} onclick={() => face('top')} title="From the top: a map of level over abscissa and record (T)">Top</button>
				<button role="radio" data-testid="waterfall-front" aria-checked={facing === 'front'} class:on={facing === 'front'} onclick={() => face('front')} title="From the front: every record on one axis (F)">Front</button>
			</div>
			<div class="seg" role="radiogroup" aria-label="{view.x.label} scale">
				<span class="lbl">{view.x.label}</span>
				<button role="radio" data-testid="waterfall-x-linear" aria-checked={!logX} class:on={!logX} onclick={() => (logX = false)} title="Linear (X)">Lin</button>
				<button role="radio" data-testid="waterfall-x-log" aria-checked={logX} class:on={logX} onclick={() => (logX = true)} title="Log (X)">Log</button>
			</div>
			<div class="seg" role="radiogroup" aria-label="{view.z.label} scale">
				<span class="lbl">{view.z.label}</span>
				<button role="radio" data-testid="waterfall-z-linear" aria-checked={!logZ} class:on={!logZ} onclick={() => (logZ = false)} title="Linear (L)">Lin</button>
				<button role="radio" data-testid="waterfall-z-log" aria-checked={logZ} class:on={logZ} onclick={() => (logZ = true)} title="Log (L)">Log</button>
			</div>
			{#if view.quantities && onparams}
				<div class="seg" role="radiogroup" aria-label="Quantity">
					<span class="lbl">Quantity</span>
					{#each view.quantities as q (q.value)}
						<button
							role="radio"
							data-testid="waterfall-quantity"
							aria-checked={q.value === view.quantity}
							class:on={q.value === view.quantity}
							onclick={() => q.value !== view.quantity && onparams({ quantity: q.value, page: 1 })}
							title="{formatCount(q.records)} records">{q.label}</button
						>
					{/each}
				</div>
			{/if}
		</div>
		{#if view.note}<div class="note">{view.note}</div>{/if}
	</div>

	<div class="bottom">
		<div class="legend" aria-label="Color legend">
			<div class="ltitle">{withUnit(view.z.label, view.z.unit)}{logZ ? ', log' : ''}</div>
			<div class="bar" style:background={colormapGradient()}></div>
			<div class="ticks num">
				{#each legendTicks as t, i (i)}<span>{t}</span>{/each}
			</div>
		</div>
		<div class="stats num" aria-live="off">
			{#if view.page}<span data-testid="waterfall-page">Page {view.page.number} of {view.page.count}, {formatCount(view.page.records)} records</span>{/if}
			<span>{formatCount(recordCount)} records</span>
			<span>{formatCount(pointCount)} points</span>
		</div>
	</div>
</div>

<style>
	.copyimg {
		flex: none;
		font-size: var(--text-2xs, 11px);
	}

	.waterfall {
		position: relative;
		height: 100%;
		min-height: 320px;
		background:
			radial-gradient(ellipse at 50% 35%, var(--bg-3), transparent 70%),
			var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	.waterfall:focus-visible {
		outline-offset: -2px;
	}

	.canvas,
	.overlay {
		position: absolute;
		inset: 0;
	}

	.canvas :global(canvas) {
		display: block;
		touch-action: none;
	}

	.overlay {
		pointer-events: none;
		overflow: hidden;
	}

	.tick,
	.title {
		position: absolute;
		left: 0;
		top: 0;
		white-space: nowrap;
		font-size: var(--text-2xs);
		color: var(--text-2);
		text-shadow:
			0 0 3px var(--bg-plot),
			0 0 2px var(--bg-plot);
		will-change: transform;
	}

	.tick.x {
		translate: -50% 2px;
	}

	.tick.z {
		translate: 4px -50%;
	}

	.tick.rec {
		translate: 4px -50%;
		color: var(--text-3);
	}

	.title {
		translate: -50% 0;
		color: var(--text-1);
		font-size: var(--text-xs);
	}

	.top,
	.bottom {
		position: absolute;
		left: var(--sp-2);
		right: var(--sp-3);
		display: flex;
		flex-wrap: wrap;
		justify-content: space-between;
		gap: var(--sp-2) var(--sp-4);
		pointer-events: none;
	}

	.top {
		top: var(--sp-2);
		align-items: flex-start;
	}

	.bottom {
		bottom: var(--sp-2);
		left: var(--sp-3);
		align-items: flex-end;
	}

	.toolbar {
		pointer-events: auto;
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		padding: 4px;
		background: color-mix(in srgb, var(--bg-1) 88%, transparent);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
		backdrop-filter: blur(6px);
	}

	.seg {
		display: inline-flex;
		align-items: center;
		padding: 2px;
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.seg .lbl {
		padding: 0 var(--sp-2) 0 var(--sp-1);
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.seg button {
		height: 20px;
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

	.note {
		padding-top: 6px;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.legend {
		width: 200px;
		padding-bottom: 2px;
	}

	.ltitle {
		margin-bottom: 4px;
		font-size: var(--text-2xs);
		color: var(--text-2);
	}

	.bar {
		height: 9px;
		border-radius: 2px;
		box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.25);
	}

	.ticks {
		display: flex;
		justify-content: space-between;
		margin-top: 3px;
		font-size: var(--text-2xs);
		color: var(--text-2);
	}

	.stats {
		display: flex;
		gap: var(--sp-3);
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.marker {
		position: absolute;
		top: -4px;
		left: -4px;
		width: 8px;
		height: 8px;
		border: 1.5px solid var(--text-1);
		border-radius: 50%;
		pointer-events: none;
	}

	.readout {
		position: absolute;
		top: 0;
		left: 0;
		display: flex;
		flex-direction: column;
		gap: 1px;
		padding: 4px 6px;
		font-size: var(--text-2xs);
		color: var(--text-1);
		white-space: nowrap;
		background: color-mix(in srgb, var(--bg-1) 90%, transparent);
		border: 1px solid var(--line-2);
		border-radius: 4px;
		pointer-events: none;
	}

	.rname {
		color: var(--text-2);
	}

	.failure {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		padding: var(--sp-6);
		color: var(--text-2);
		text-align: center;
	}
</style>
