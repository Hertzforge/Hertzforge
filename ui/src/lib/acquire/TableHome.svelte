<script lang="ts">
	/**
	 * The project is the channel table's home. Use the Project's Channel Table reads a project
	 * table, shows every row that would change and the control service's verdict, and only then
	 * takes it; it arrives restored and is applied by the operator. Send Table to Project shows
	 * what would change in the project's table and only then sends the service's table, with every
	 * analysis column. A refusal is shown in the service's own words.
	 */
	import type { AcquireState } from './store.svelte';

	interface Props {
		acq: AcquireState;
	}
	let { acq }: Props = $props();

	let name = $state('');
	const review = $derived(acq.homeReview);
	const noProject = $derived(acq.tableFiles ? null : 'No analysis service is connected, so there is no project.');
	const useWhy = $derived(
		noProject ?? (!acq.connected ? 'The control service is not connected.' : !acq.isController ? 'Take control to take a table.' : acq.locked ? 'The table is locked while a run is live.' : null)
	);
	const sendWhy = $derived(noProject ?? (!acq.server ? 'The control service has sent no table yet.' : null));
	const refused = $derived(review?.report?.errors ?? []);
	const target = () => name.trim() || null;
</script>

<div class="home" data-testid="table-home">
	<input class="field sm" aria-label="Project table name" placeholder="Project table" bind:value={name} data-testid="table-home-name" />
	<button class="btn sm" data-testid="table-home-use" disabled={!!useWhy || acq.busy !== null} title={useWhy ?? "Read the project's channel table and show what would change."} onclick={() => acq.reviewProjectTable(target())}>Use the Project's Channel Table</button>
	<button class="btn sm" data-testid="table-home-send" disabled={!!sendWhy} title={sendWhy ?? "Show what sending the service's table to the project would change."} onclick={() => acq.reviewSendToProject(target())}>Send Table to Project</button>
</div>

{#if acq.homeProblem}
	<div class="aq-notice warn" role="alert" data-testid="table-home-problem">
		<p class="grow"><strong>Not done:</strong> {acq.homeProblem}</p>
		<button class="btn ghost sm" onclick={() => acq.cancelHome()}>Dismiss</button>
	</div>
{/if}

{#if review}
	<div class="aq-notice info" role="dialog" aria-label="Review the table" data-testid="table-home-review" data-direction={review.direction}>
		<div class="grow">
			<strong>
				{#if review.direction === 'use'}
					Take the project table "{review.name}" as the control service's table?
				{:else if review.replaces}
					Replace the project table "{review.name}" with the control service's table?
				{:else}
					Add the control service's table to the project as "{review.name}"?
				{/if}
			</strong>
			<ul data-testid="table-home-changes">
				{#each review.changes as line (line)}<li>{line}</li>{/each}
				{#if review.changes.length === 0}<li>No row would change.</li>{/if}
			</ul>
			{#each review.warnings as w (w)}<p class="aq-small">{w}</p>{/each}
			{#if refused.length}
				<p class="aq-small" data-testid="table-home-refused"><strong>The control service refuses this table:</strong></p>
				<ul>
					{#each refused as e, i (i)}<li>{e.message}</li>{/each}
				</ul>
			{:else if review.direction === 'use'}
				<p class="aq-small">It arrives restored: review and apply it before any run arms on it.</p>
			{/if}
		</div>
		<div class="aq-actions">
			{#if review.direction === 'use'}
				<button class="btn primary sm" data-testid="table-home-take" disabled={refused.length > 0 || !!useWhy || acq.busy !== null} onclick={() => acq.takeProjectTable()}>Take This Table</button>
			{:else}
				<button class="btn primary sm" data-testid="table-home-put" onclick={() => acq.sendToProject()}>{review.replaces ? 'Replace in Project' : 'Add to Project'}</button>
			{/if}
			<button class="btn ghost sm" data-testid="table-home-cancel" onclick={() => acq.cancelHome()}>Cancel</button>
		</div>
	</div>
{/if}

<style>
	.home {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		align-items: center;
	}
	.home input {
		width: 140px;
	}
</style>
