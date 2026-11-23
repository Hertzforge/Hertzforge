<script lang="ts">
	/**
	 * A 3-D animation recorded frame by frame with no window (`hertzforge record`): the shell's own
	 * 3-D view, alone on the page, its frames handed to the recorder through
	 * `window.__hertzforgeRecorder` one at a time. The page is the report viewer with a recording in
	 * place of a report, so it draws exactly as the app does.
	 */
	import { onMount, tick } from 'svelte';
	import type { Shape3DView } from '$lib/protocol/types';
	import Shape3DPane from '$lib/viz/Shape3DPane.svelte';

	export interface RecordSetup {
		width: number;
		height: number;
		scale?: number;
		mode?: number;
		/** Draw each DOF arrow's label into the frames. */
		labels?: boolean;
	}

	interface Props {
		view: Shape3DView;
		setup: RecordSetup;
	}
	let { view, setup }: Props = $props();
	let pane = $state<ReturnType<typeof Shape3DPane> | null>(null);

	onMount(async () => {
		const w = window as unknown as { __hertzforgeRecorder?: unknown };
		// the pane draws on mount; wait for its renderer before setting it up
		let last: unknown = null;
		for (let k = 0; k < 200; k++) {
			await tick();
			try {
				pane?.recordSetup({ width: setup.width, height: setup.height, scale: setup.scale, modeAt: setup.mode, labels: setup.labels });
				last = null;
				break;
			} catch (err) {
				last = err;
				await new Promise((r) => setTimeout(r, 25));
			}
		}
		if (last !== null) {
			// the recorder says why rather than leaving it to wait out its time
			(window as unknown as { __hertzforgeRecorderFailed?: string }).__hertzforgeRecorderFailed = String((last as Error)?.message ?? last);
			return;
		}
		await tick();
		w.__hertzforgeRecorder = {
			info: () => pane!.recordInfo(),
			frame: (at: number) => pane!.recordFrame(at)
		};
	});
</script>

<div class="stage" style:width="{setup.width}px" style:height="{setup.height}px" data-testid="record-stage">
	<Shape3DPane bind:this={pane} {view} />
</div>

<style>
	.stage {
		position: fixed;
		inset: 0 auto auto 0;
		display: flex;
		flex-direction: column;
		background: var(--bg-1);
	}
</style>
