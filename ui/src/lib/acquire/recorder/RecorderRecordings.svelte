<script lang="ts">
	/**
	 * The run's recordings: each one's folder under the run folder and its facts, as the service
	 * lists them, and the one being written last with its frames on disk and its backlog from the
	 * run's live numbers.
	 */
	import type { RecorderRecording, RecorderRecordingNow } from '$lib/protocol/recorder';
	import { recordingFacts, savedWords, secs } from './recorderWords';

	interface Props {
		/** The saved recordings, in the order they began. */
		saved: RecorderRecording[];
		/** The recording in progress, from the live numbers. */
		now: RecorderRecordingNow | null;
		folder: string | null;
		fs: number | null;
	}
	let { saved, now, folder, fs }: Props = $props();

	const where = (r: RecorderRecording) => (folder ? `${folder}/${r.folder}` : r.folder);
</script>

<div class="recs" data-testid="recorder-recordings" data-count={saved.length}>
	{#if folder}<p class="aq-small aq-muted">In {folder}</p>{/if}
	{#if now}
		<article class="rec live" data-testid="recorder-recording-{now.name}" data-complete="false">
			<h3>{now.name} <span class="tag">Being written</span></h3>
			<p class="aq-small">{secs(now.seconds)} after its trigger{now.length_s !== null ? ` of ${secs(now.length_s)}` : ', until cut'}; {now.frames_on_disk} of {now.frames_fed} frames on disk, {secs(now.backlog_s)} behind.</p>
			<p class="aq-small aq-muted">Trigger at station frame {now.trigger_station_frame}; first frame {now.first_station_frame}; pre-window {now.pre_frames}{now.pre_frames < now.pre_frames_asked ? ` of ${now.pre_frames_asked}` : ''} frames{fs ? ` (${secs(now.pre_frames / fs)})` : ''}.</p>
		</article>
	{/if}
	{#each [...saved].reverse() as r (r.name)}
		<article class="rec" data-testid="recorder-recording-{r.name}" data-complete={r.complete} data-frames={r.frames}>
			<h3>{r.name}</h3>
			<p class="aq-small">{savedWords(r)}</p>
			<p class="aq-small mono where">{where(r)}</p>
			<dl>
				{#each recordingFacts(r) as f (f.label)}<div><dt>{f.label}</dt><dd class="num">{f.value}</dd></div>{/each}
			</dl>
		</article>
	{:else}
		{#if !now}<p class="aq-muted aq-small empty">No recording yet. Monitoring saves nothing; a recording is listed here as it is saved.</p>{/if}
	{/each}
</div>

<style>
	.recs {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.recs > p {
		margin: 0;
	}

	.rec {
		padding: var(--sp-2) var(--sp-3);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.rec.live {
		border-color: var(--blue-line);
	}

	h3 {
		margin: 0 0 4px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.tag {
		margin-left: 6px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-medium);
		color: var(--text-3);
	}

	p {
		margin: 0 0 4px;
	}

	.mono {
		font-family: var(--font-mono);
	}

	.where {
		color: var(--text-2);
		word-break: break-all;
	}

	dl {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 4px var(--sp-3);
		margin: 4px 0 0;
	}

	dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	dd {
		margin: 0;
		font-size: var(--text-xs);
	}

	.empty {
		margin: var(--sp-4) 0;
	}

	@media (max-width: 900px) {
		dl {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
</style>
