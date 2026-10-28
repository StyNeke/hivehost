<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../api';

  type Release = { tag: string; name: string | null; published_at: string | null; prerelease: boolean; url: string; assets: string[] };
  type Commit = { sha: string; message: string; date: string | null; url: string };
  type Milestone = { title: string; open_issues: number; closed_issues: number; due_on: string | null; url: string };
  type Upstream = { description: string | null; url: string; pushed_at: string | null; releases: Release[]; commits: Commit[]; milestones: Milestone[] };
  type Entry = { key: string; name: string; repository: string; website: string; summary: string; upstream: Upstream | null; fetched_at: number | null; stale: boolean; error: string | null; egg_id: string | null };

  let { onAdded }: { onAdded: () => void } = $props();
  let entries = $state<Entry[]>([]);
  let error = $state('');
  let busy = $state(false);

  const date = (value: string | null) => value ? new Date(value).toLocaleDateString(undefined, { dateStyle: 'medium' }) : 'unknown';
  const when = (at: number) => new Date(at * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  async function load() {
    try { entries = await api<Entry[]>('/api/experimental'); } catch (reason) { error = reason instanceof Error ? reason.message : 'Could not load upstream status.'; }
  }
  async function refresh() {
    busy = true; error = '';
    try { entries = await api<Entry[]>('/api/experimental/refresh', { method: 'POST' }); } catch (reason) { error = reason instanceof Error ? reason.message : 'Refresh failed.'; } finally { busy = false; }
  }
  async function add(entry: Entry) {
    busy = true; error = '';
    try { await api(`/api/experimental/${entry.key}/egg`, { method: 'POST' }); await load(); onAdded(); } catch (reason) { error = reason instanceof Error ? reason.message : 'Could not add the egg.'; } finally { busy = false; }
  }
  onMount(() => { void load(); });
</script>

<section class="panel experimental-panel" aria-label="Experimental servers">
  <div class="backup-heading">
    <div>
      <div class="eyebrow">Experimental</div>
      <h2>Alternative server implementations</h2>
      <p>New implementations of the Minecraft server, for test servers only. They are incomplete, support no plugins or mod loaders, and are never used to convert an existing world. Status below is copied from GitHub; it is not a measure of how complete they are.</p>
    </div>
    <button class="secondary" disabled={busy} onclick={refresh}>Refresh status</button>
  </div>
  {#if error}<div class="feedback error inline" role="alert">{error}</div>{/if}
  <div class="experimental-grid">
    {#each entries as entry (entry.key)}
      <article class="experimental-card" aria-label={entry.name}>
        <header><h3>{entry.name}</h3><span class="tag warning-tag">Experimental</span></header>
        <p class="muted">{entry.upstream?.description ?? entry.summary}</p>
        <p class="links"><a href={`https://github.com/${entry.repository}`} target="_blank" rel="noreferrer noopener">{entry.repository}</a> · <a href={entry.website} target="_blank" rel="noreferrer noopener">Website</a></p>
        {#if entry.upstream}
          <dl class="upstream">
            <dt>Last code change</dt><dd>{date(entry.upstream.pushed_at)}</dd>
            <dt>Published builds</dt>
            <dd>{#if entry.upstream.releases.length}{#each entry.upstream.releases.slice(0, 3) as release (release.tag)}<a href={release.url} target="_blank" rel="noreferrer noopener">{release.name || release.tag}</a>{release.prerelease ? ' (pre-release)' : ''}, {date(release.published_at)}<br />{/each}{:else}None published{/if}</dd>
            <dt>Recent commits</dt>
            <dd>{#each entry.upstream.commits.slice(0, 3) as commit (commit.sha)}<a href={commit.url} target="_blank" rel="noreferrer noopener">{commit.message}</a> ({date(commit.date)})<br />{/each}</dd>
            <dt>Open milestones</dt>
            <dd>{#if entry.upstream.milestones.length}{#each entry.upstream.milestones as milestone (milestone.url)}<a href={milestone.url} target="_blank" rel="noreferrer noopener">{milestone.title}</a>: {milestone.closed_issues} of {milestone.open_issues + milestone.closed_issues} issues in this milestone closed{milestone.due_on ? `, due ${date(milestone.due_on)}` : ''}<br />{/each}{:else}None published{/if}</dd>
          </dl>
        {:else}
          <p class="muted">Upstream status is not available yet.</p>
        {/if}
        <p class="freshness" class:stale={entry.stale}>
          {#if entry.fetched_at}{entry.stale ? 'Last known status' : 'Status'} from {when(entry.fetched_at)}{:else}Never loaded{/if}{entry.error ? ` · ${entry.error}` : ''}
        </p>
        <button class="secondary" disabled={busy || !!entry.egg_id} onclick={() => add(entry)}>{entry.egg_id ? 'In the egg library' : 'Add to egg library'}</button>
      </article>
    {/each}
  </div>
</section>
