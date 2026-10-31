<script lang="ts">
	/**
	 * Mode shape view, drawn with three.js.
	 *
	 * Deformation x0 + a * shape * sin(2 pi t / T) runs in the vertex shader, so a frame costs one
	 * uniform update however large the mesh is. Normals come from screen-space derivatives (flat
	 * shading), which keeps lighting correct on the deformed surface. Color is the mode shape
	 * magnitude on a colorblind-safe sequential map with a numeric legend. GPU resources are
	 * released on unmount and the WebGL context is dropped explicitly (WebKit caps live contexts).
	 */
	import { onDestroy, onMount } from 'svelte';
	import {
		AmbientLight,
		BufferAttribute,
		BufferGeometry,
		Color,
		DirectionalLight,
		DoubleSide,
		LineBasicMaterial,
		LineSegments,
		Mesh,
		MeshBasicMaterial,
		MeshLambertMaterial,
		PerspectiveCamera,
		Quaternion,
		Scene,
		Vector3,
		WebGLRenderer,
		type Material
	} from 'three';
	import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
	import type { Shape3DView } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { colormap, colormapGradient } from './colormap';
	import { formatCount, formatFixed, formatNumber } from './format';

	interface Props {
		view: Shape3DView;
		/** Index into `view.modes`. */
		mode?: number;
	}

	let { view, mode = $bindable(0) }: Props = $props();

	/** Seconds per animation cycle. Visual only; not the mode's period. */
	const CYCLE_S = 1.6;
	/** Amplitude slider full scale, as a fraction of the model's diagonal. */
	const AMP_FULL_SCALE = 0.15;

	let host: HTMLDivElement;
	let canvasHost: HTMLDivElement;
	let triad: SVGSVGElement;

	let playing = $state(true);
	let amplitude = $state(0.5);
	let wireframe = $state(false);
	let ghost = $state(false);
	let fps = $state<number | null>(null);
	let failure = $state<string | null>(null);

	const nodeCount = $derived(view.nodes.length / 3);
	const triCount = $derived((view.triangles?.length ?? 0) / 3);
	const current = $derived(view.modes[Math.min(Math.max(0, mode), view.modes.length - 1)]);

	/** Model diagonal, from the undeformed nodes. */
	const diagonal = $derived.by(() => {
		const n = view.nodes;
		const lo = [Infinity, Infinity, Infinity];
		const hi = [-Infinity, -Infinity, -Infinity];
		for (let i = 0; i < n.length; i += 3) {
			for (let k = 0; k < 3; k++) {
				if (n[i + k] < lo[k]) lo[k] = n[i + k];
				if (n[i + k] > hi[k]) hi[k] = n[i + k];
			}
		}
		return Math.hypot(hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]) || 1;
	});

	const peak = $derived(amplitude * AMP_FULL_SCALE * diagonal);

	/** Display scaling of the model unit: meters shown as millimeters below one meter. */
	const display = $derived.by(() => {
		if (view.unit === 'm' && peak < 1) return { factor: 1000, unit: 'mm' };
		return { factor: 1, unit: view.unit };
	});

	// three.js objects (not reactive)
	let renderer: WebGLRenderer | null = null;
	let scene: Scene;
	let camera: PerspectiveCamera;
	let controls: OrbitControls;
	let geometry: BufferGeometry;
	let lineGeometry: BufferGeometry | null = null;
	let shapeAttr: BufferAttribute;
	let colorAttr: BufferAttribute;
	let surface: Mesh;
	let outline: LineSegments | null = null;
	let ghostSurface: Mesh;
	let ghostOutline: LineSegments | null = null;
	const materials: Material[] = [];
	const uniforms = { uScale: { value: 0 } };
	let maxMag = 1;
	let raf = 0;
	let phase = 0;
	let lastTime = 0;
	let dirty = true;
	let frames = 0;
	let fpsWindowStart = 0;
	let resizeObs: ResizeObserver | null = null;
	let homeDistance = 1;

	/** Inject the deformation into a built-in material's vertex shader. */
	function displaced<M extends Material>(material: M): M {
		material.onBeforeCompile = (shader) => {
			shader.uniforms.uScale = uniforms.uScale;
			shader.vertexShader = shader.vertexShader
				.replace('#include <common>', '#include <common>\nattribute vec3 shape;\nuniform float uScale;')
				.replace('#include <begin_vertex>', '#include <begin_vertex>\ntransformed += shape * uScale;');
		};
		material.customProgramCacheKey = () => 'sds-displaced';
		materials.push(material);
		return material;
	}

	function plain<M extends Material>(material: M): M {
		materials.push(material);
		return material;
	}

	function srgbToLinear(c: number) {
		return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
	}

	function loadMode() {
		if (!renderer || !current) return;
		const src = current.shape;
		const dst = shapeAttr.array as Float32Array;
		const colors = colorAttr.array as Float32Array;
		let m = 0;
		for (let i = 0; i < src.length; i += 3) {
			const mag = Math.hypot(src[i], src[i + 1], src[i + 2]);
			if (mag > m) m = mag;
		}
		maxMag = m || 1;
		const rgb: [number, number, number] = [0, 0, 0];
		for (let i = 0; i < src.length; i += 3) {
			dst[i] = src[i];
			dst[i + 1] = src[i + 1];
			dst[i + 2] = src[i + 2];
			colormap(Math.hypot(src[i], src[i + 1], src[i + 2]) / maxMag, rgb);
			colors[i] = srgbToLinear(rgb[0]);
			colors[i + 1] = srgbToLinear(rgb[1]);
			colors[i + 2] = srgbToLinear(rgb[2]);
		}
		shapeAttr.needsUpdate = true;
		colorAttr.needsUpdate = true;
		dirty = true;
	}

	function applyTheme() {
		if (!renderer) return;
		const ghostColor = new Color(cssVar('--text-3'));
		const lineColor = new Color(cssVar('--text-2'));
		(ghostSurface.material as MeshBasicMaterial).color = ghostColor;
		if (ghostOutline) (ghostOutline.material as LineBasicMaterial).color = ghostColor;
		if (outline) (outline.material as LineBasicMaterial).color = lineColor;
		dirty = true;
	}

	function resetView() {
		if (!renderer) return;
		const dir = new Vector3(0.55, -1.2, 0.58).normalize();
		controls.target.set(centre.x, centre.y, centre.z);
		camera.position.copy(centre).addScaledVector(dir, homeDistance);
		camera.up.set(0, 0, 1);
		controls.update();
		dirty = true;
	}

	const centre = new Vector3();

	function init() {
		try {
			renderer = new WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'high-performance' });
		} catch {
			failure = 'The 3-D view needs WebGL, which is not available in this browser.';
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
		camera = new PerspectiveCamera(32, 1, 0.001, 1000);
		camera.up.set(0, 0, 1);

		// Geometry: one position buffer shared by the surface, outline and ghosts.
		geometry = new BufferGeometry();
		const position = new BufferAttribute(view.nodes, 3);
		shapeAttr = new BufferAttribute(new Float32Array(view.nodes.length), 3);
		colorAttr = new BufferAttribute(new Float32Array(view.nodes.length), 3);
		geometry.setAttribute('position', position);
		geometry.setAttribute('shape', shapeAttr);
		geometry.setAttribute('color', colorAttr);
		if (view.triangles) geometry.setIndex(new BufferAttribute(view.triangles, 1));
		geometry.computeBoundingSphere();
		const sphere = geometry.boundingSphere!;
		centre.copy(sphere.center);
		const radius = Math.max(sphere.radius, 1e-6);
		homeDistance = (radius / Math.sin((camera.fov * Math.PI) / 360)) * 0.82;
		camera.near = radius / 100;
		camera.far = radius * 100;
		camera.updateProjectionMatrix();

		surface = new Mesh(
			geometry,
			displaced(new MeshLambertMaterial({ vertexColors: true, side: DoubleSide, flatShading: true }))
		);
		surface.frustumCulled = false;
		scene.add(surface);

		ghostSurface = new Mesh(
			geometry,
			plain(new MeshBasicMaterial({ transparent: true, opacity: 0.1, depthWrite: false, side: DoubleSide }))
		);
		ghostSurface.visible = false;
		ghostSurface.renderOrder = 2;
		scene.add(ghostSurface);

		if (view.lines && view.lines.length) {
			lineGeometry = new BufferGeometry();
			lineGeometry.setAttribute('position', position);
			lineGeometry.setAttribute('shape', shapeAttr);
			lineGeometry.setIndex(new BufferAttribute(view.lines, 1));
			outline = new LineSegments(lineGeometry, displaced(new LineBasicMaterial({ transparent: true, opacity: 0.9 })));
			outline.frustumCulled = false;
			scene.add(outline);
			ghostOutline = new LineSegments(
				lineGeometry,
				plain(new LineBasicMaterial({ transparent: true, opacity: 0.55, depthWrite: false }))
			);
			ghostOutline.visible = false;
			scene.add(ghostOutline);
		}

		// A headlight that follows the camera keeps shading even while orbiting.
		scene.add(new AmbientLight(0xffffff, 1.05));
		const head = new DirectionalLight(0xffffff, 2.1);
		head.position.set(0.45, 0.9, 1);
		camera.add(head);
		scene.add(camera);

		controls = new OrbitControls(camera, renderer.domElement);
		controls.enableDamping = true;
		controls.dampingFactor = 0.12;
		controls.rotateSpeed = 0.8;
		controls.zoomSpeed = 0.9;
		controls.addEventListener('change', () => (dirty = true));

		resetView();
		applyTheme();
		loadMode();

		resizeObs = new ResizeObserver(resize);
		resizeObs.observe(canvasHost);
		resize();

		lastTime = performance.now();
		fpsWindowStart = lastTime;
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

	const tmp = new Vector3();
	const inv = new Quaternion();
	function updateTriad() {
		if (!triad) return;
		inv.copy(camera.quaternion).invert();
		const axes: [string, Vector3][] = [
			['x', new Vector3(1, 0, 0)],
			['y', new Vector3(0, 1, 0)],
			['z', new Vector3(0, 0, 1)]
		];
		for (const [name, v] of axes) {
			tmp.copy(v).applyQuaternion(inv);
			const line = triad.querySelector<SVGLineElement>(`line.${name}`);
			const label = triad.querySelector<SVGTextElement>(`text.${name}`);
			const x = 24 + tmp.x * 15;
			const y = 24 - tmp.y * 15;
			line?.setAttribute('x2', x.toFixed(1));
			line?.setAttribute('y2', y.toFixed(1));
			label?.setAttribute('x', (24 + tmp.x * 21).toFixed(1));
			label?.setAttribute('y', (24 - tmp.y * 21 + 3.5).toFixed(1));
		}
	}

	function loop(now: number) {
		raf = requestAnimationFrame(loop);
		if (!renderer) return;
		const dt = Math.min(0.1, (now - lastTime) / 1000);
		lastTime = now;
		const moved = controls.update();
		if (playing) {
			phase = (phase + dt / CYCLE_S) % 1;
			dirty = true;
		}
		const scale = (peak / maxMag) * Math.sin(2 * Math.PI * phase);
		if (uniforms.uScale.value !== scale) {
			uniforms.uScale.value = scale;
			dirty = true;
		}
		if (!dirty && !moved) {
			fpsWindowStart = now;
			frames = 0;
			return;
		}
		dirty = false;
		renderer.render(scene, camera);
		updateTriad();
		frames++;
		if (now - fpsWindowStart >= 500) {
			fps = playing ? Math.round((frames * 1000) / (now - fpsWindowStart)) : fps;
			frames = 0;
			fpsWindowStart = now;
		}
	}

	onMount(init);

	// Mode or view changes: reload the shape buffer.
	$effect(() => {
		void current;
		loadMode();
	});

	$effect(() => {
		void theme.version;
		applyTheme();
	});

	$effect(() => {
		if (!renderer) return;
		for (const m of [surface.material as MeshLambertMaterial]) m.wireframe = wireframe;
		ghostSurface.visible = ghost;
		if (ghostOutline) ghostOutline.visible = ghost;
		dirty = true;
	});

	$effect(() => {
		void peak;
		dirty = true;
	});

	onDestroy(() => {
		cancelAnimationFrame(raf);
		resizeObs?.disconnect();
		if (!renderer) return;
		controls.dispose();
		geometry.dispose();
		lineGeometry?.dispose();
		for (const m of materials) m.dispose();
		renderer.dispose();
		renderer.forceContextLoss();
		renderer.domElement.remove();
		renderer = null;
	});

	function onKey(e: KeyboardEvent) {
		if (e.target instanceof HTMLInputElement || e.metaKey || e.ctrlKey || e.altKey) return;
		const k = e.key.toLowerCase();
		if (k === ' ') playing = !playing;
		else if (k === 'w') wireframe = !wireframe;
		else if (k === 'g') ghost = !ghost;
		else if (k === 'r') resetView();
		else return;
		e.preventDefault();
	}

	const legendTicks = $derived([0, 0.5, 1].map((t) => formatNumber(t * peak * display.factor, 3)));
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
	class="shape3d"
	data-testid="view-shape3d"
	bind:this={host}
	tabindex="0"
	role="group"
	aria-label="{view.title}. Space plays or pauses, W toggles wireframe, G toggles the undeformed ghost, R resets the view."
	onkeydown={onKey}
>
	<div class="canvas" data-testid="shape3d-canvas" bind:this={canvasHost}></div>

	{#if failure}
		<div class="failure" role="alert">{failure}</div>
	{/if}

	<div class="top">
		<div class="toolbar" role="toolbar" aria-label="Mode shape display">
			<button
				class="btn icon sm"
				data-testid="shape3d-play"
				onclick={() => (playing = !playing)}
				aria-pressed={playing}
				title={playing ? 'Pause (Space)' : 'Play (Space)'}
				aria-label={playing ? 'Pause' : 'Play'}
			>
				{#if playing}
					<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M5 3.5v9M11 3.5v9" stroke="currentColor" stroke-width="2" stroke-linecap="round" /></svg>
				{:else}
					<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M5 3.2v9.6L12.6 8z" fill="currentColor" /></svg>
				{/if}
			</button>
			<label class="amp">
				<span class="lbl">Amplitude</span>
				<input type="range" data-testid="shape3d-amplitude" min="0" max="1" step="0.01" bind:value={amplitude} aria-valuetext="Peak {formatFixed(peak * display.factor, 3)} {display.unit}" />
				<span class="ampval num">{formatFixed(peak * display.factor, 3)} {display.unit}</span>
			</label>
			<div class="seg" role="radiogroup" aria-label="Surface style">
				<button role="radio" data-testid="shape3d-surface" aria-checked={!wireframe} class:on={!wireframe} onclick={() => (wireframe = false)} title="Surface (W)">Surface</button>
				<button role="radio" data-testid="shape3d-wireframe" aria-checked={wireframe} class:on={wireframe} onclick={() => (wireframe = true)} title="Wireframe (W)">Wireframe</button>
			</div>
			<button class="btn sm toggle" data-testid="shape3d-ghost" class:on={ghost} aria-pressed={ghost} onclick={() => (ghost = !ghost)} title="Undeformed ghost (G)">
				<svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"><rect x="2" y="5" width="12" height="6" rx="1" fill="none" stroke="currentColor" stroke-width="1.3" stroke-dasharray="2 1.6" /></svg>
				Ghost
			</button>
			<button class="btn icon sm" data-testid="shape3d-reset" onclick={resetView} title="Reset view (R)" aria-label="Reset view">
				<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M3 8a5 5 0 105-5H5.5M5.5 1.2V5H9.3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
		</div>

		{#if current}
			<div class="modetag">
				<span class="m">Mode {current.index}</span>
				<span class="num">{formatFixed(current.frequency_hz, 4)} Hz</span>
				{#if current.damping != null}<span class="num dim">{formatFixed(current.damping * 100, 3)} % damping</span>{/if}
			</div>
		{/if}
	</div>

	<div class="bottom">
		<div class="legend" aria-label="Color legend">
			<div class="ltitle">Peak displacement ({display.unit})</div>
			<div class="bar" style:background={colormapGradient()}></div>
			<div class="ticks num">
				{#each legendTicks as t, i (i)}<span>{t}</span>{/each}
			</div>
		</div>

		<div class="corner">
			<svg class="triad" bind:this={triad} viewBox="0 0 48 48" width="48" height="48" aria-hidden="true">
				<line class="x" x1="24" y1="24" x2="39" y2="24" />
				<line class="y" x1="24" y1="24" x2="24" y2="9" />
				<line class="z" x1="24" y1="24" x2="30" y2="30" />
				<text class="x" x="45" y="27">X</text>
				<text class="y" x="24" y="6">Y</text>
				<text class="z" x="33" y="36">Z</text>
			</svg>

			<div class="stats num" aria-live="off">
				<span class="fps" data-testid="shape3d-fps" class:paused={!playing}>{playing ? (fps === null ? '-- fps' : `${fps} fps`) : 'Paused'}</span>
				<span>{formatCount(nodeCount)} nodes</span>
				<span>{formatCount(triCount)} triangles</span>
			</div>
		</div>
	</div>
</div>

<style>
	.shape3d {
		position: relative;
		height: 100%;
		min-height: 280px;
		background:
			radial-gradient(ellipse at 50% 35%, var(--bg-3), transparent 70%),
			var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	.shape3d:focus-visible {
		outline-offset: -2px;
	}

	.canvas {
		position: absolute;
		inset: 0;
	}

	.canvas :global(canvas) {
		display: block;
		touch-action: none;
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

	.amp {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		padding: 0 var(--sp-1);
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.amp input {
		width: 110px;
	}

	.ampval {
		min-width: 7.5ch;
		color: var(--text-1);
		text-align: right;
	}

	.seg {
		display: inline-flex;
		padding: 2px;
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
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

	.toggle.on {
		color: var(--text-1);
		background: var(--blue-weak);
		border-color: var(--blue-line);
	}

	.modetag {
		padding-top: 6px;
		display: flex;
		align-items: baseline;
		gap: var(--sp-3);
		font-size: var(--text-sm);
		color: var(--text-1);
		pointer-events: none;
	}

	.modetag .m {
		font-weight: var(--weight-strong);
	}

	.dim {
		color: var(--text-3);
	}

	.legend {
		width: 200px;
		padding-bottom: 2px;
	}

	.corner {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: 2px;
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

	.triad {
		margin-right: -6px;
	}

	.triad line {
		stroke-width: 1.6;
		stroke-linecap: round;
	}

	.triad line.x {
		stroke: var(--blue-2);
	}

	.triad line.y {
		stroke: var(--amber-2);
		stroke-dasharray: 3 2;
	}

	.triad line.z {
		stroke: var(--text-2);
		stroke-dasharray: 1 2;
	}

	.triad text {
		font-family: var(--font-mono);
		font-size: 8.5px;
		fill: var(--text-2);
		text-anchor: middle;
	}

	.stats {
		display: flex;
		gap: var(--sp-3);
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.fps {
		min-width: 6.5ch;
		color: var(--text-1);
		text-align: right;
	}

	.fps.paused {
		color: var(--text-3);
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
