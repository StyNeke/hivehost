<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import { api, ApiError } from '../api';
  import Icon from '../components/Icon.svelte';

  type Resources = { memory_bytes: number; disk_bytes: number; cpu_millis: number };
  type Operation = { kind: 'install' | 'extensions' | 'delete'; state: 'running' | 'succeeded' | 'failed'; message: string };
  type Runtime = {
    installed: boolean; operation: Operation | null; power: 'offline' | 'starting' | 'running' | 'stopping';
    desired_running: boolean; problem: string | null; started_at: number | null; exit_code: number | null;
    game: { version: string; loader: string | null } | null; disk_used_bytes: number | null; disk_total_bytes: number | null;
  };
  type Detail = {
    id: string; name: string; is_owner: boolean; runtime: Runtime | null; runtime_error: string | null;
    details: null | {
      egg_name: string; limits: Resources; binding: { ip: string; port: number; transport: string };
      install_state: 'pending' | 'installing' | 'installed' | 'failed'; install_message: string | null; deleting: boolean;
    };
  };
  type Extension = { project_id: string; title: string; version_number: string; filename: string; dependency: boolean };
  type Extensions = { game: Runtime['game']; available: boolean; reason: string | null; managed: Extension[]; other_files: string[] };
  type Hit = { project_id: string; title: string; description: string; downloads: number };
  type Activity = { at: number; actor: string | null; action: string; outcome: string; detail: string | null };

  let { serverId, onBack, onDeleted }: { serverId: string; onBack: () => void; onDeleted: (name: string) => void } = $props();

  let detail = $state<Detail | null>(null);
  let tab = $state<'console' | 'mods' | 'activity' | 'settings'>('console');
  let lines = $state<string[]>([]);
  let command = $state('');
  let error = $state('');
  let notice = $state('');
  let busy = $state(false);
  let extensions = $state<Extensions | null>(null);
  let query = $state('');
  let hits = $state<Hit[]>([]);
  let activity = $state<Activity[]>([]);
  let confirmName = $state('');
  let consoleBox = $state<HTMLElement | undefined>();
  let stuckToBottom = true;
  let timer: ReturnType<typeof setInterval> | undefined;
  let gone = false;

  const runtime = $derived(detail?.runtime ?? null);
  const install = $derived(detail?.details?.install_state ?? 'pending');
  const operation = $derived(runtime?.operation?.state === 'running' ? runtime.operation : null);
  const installed = $derived(install === 'installed');
  const power = $derived(runtime?.power ?? 'offline');
  const stopped = $derived(power === 'offline' && !runtime?.desired_running);
  const statusLabel = $derived(
    detail?.details?.deleting ? 'Deleting' :
    install === 'pending' ? 'Waiting to install' : install === 'installing' ? 'Installing' : install === 'failed' ? 'Install failed' :
    operation?.kind === 'extensions' ? 'Updating mods' :
    { offline: 'Offline', starting: 'Starting', running: 'Running', stopping: 'Stopping' }[power]);

  function gib(bytes: number) { return `${(bytes / 1024 ** 3).toFixed(1)} GiB`; }
  function cores(millis: number) { return `${(millis / 1000).toFixed(2).replace(/\.?0+$/, '')} cores`; }
  function when(at: number) { return new Date(at * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }); }
  function report(reason: unknown) {
    error = reason instanceof Error ? reason.message : 'Something went wrong. Please try again.';
    if (reason instanceof ApiError && (reason.status === 403 || reason.status === 404)) { stopPolling(); }
  }

  async function refresh() {
    try {
      detail = await api<Detail>(`/api/servers/${serverId}`);
      if (tab === 'console' && detail.details && detail.details.install_state !== 'pending') {
        const result = await api<{ lines: string[] }>(`/api/servers/${serverId}/console?lines=300`);
        if (result.lines.join('\n') !== lines.join('\n')) {
          lines = result.lines;
          if (stuckToBottom) { await tick(); consoleBox?.scrollTo({ top: consoleBox.scrollHeight }); }
        }
      }
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 404 && detail?.details?.deleting) { gone = true; stopPolling(); onDeleted(detail.name); return; }
      report(reason);
    }
  }
  function stopPolling() { if (timer) clearInterval(timer); timer = undefined; }
  onMount(() => { void refresh(); timer = setInterval(() => { if (!gone) void refresh(); }, 2000); });
  onDestroy(stopPolling);

  async function act(action: () => Promise<void>, success = '') {
    busy = true; error = ''; notice = '';
    try { await action(); if (success) notice = success; await refresh(); } catch (reason) { report(reason); } finally { busy = false; }
  }
  const powerAction = (action: 'start' | 'stop' | 'restart' | 'kill') => act(async () => {
    if (action === 'kill' && !confirm('Force-stop without saving? Unsaved progress can be lost.')) return;
    await api(`/api/servers/${serverId}/power`, { method: 'POST', body: JSON.stringify({ action }) });
  });
  async function send(event: SubmitEvent) {
    event.preventDefault();
    const text = command.trim();
    if (!text) return;
    await act(async () => { await api(`/api/servers/${serverId}/console`, { method: 'POST', body: JSON.stringify({ command: text }) }); command = ''; });
  }
  function scrolled() {
    if (consoleBox) stuckToBottom = consoleBox.scrollHeight - consoleBox.scrollTop - consoleBox.clientHeight < 24;
  }

  async function loadExtensions() {
    try { extensions = await api<Extensions>(`/api/servers/${serverId}/extensions`); } catch (reason) { report(reason); }
  }
  async function search(event?: SubmitEvent) {
    event?.preventDefault();
    await act(async () => { hits = await api<Hit[]>(`/api/servers/${serverId}/modrinth?query=${encodeURIComponent(query)}`); });
  }
  const installExtension = (hit: Hit) => act(async () => {
    const result = await api<{ installing: Extension[] }>(`/api/servers/${serverId}/extensions`, { method: 'POST', body: JSON.stringify({ project_id: hit.project_id }) });
    if (!result.installing.length) { notice = `${hit.title} is already up to date.`; return; }
    notice = `Installing ${result.installing.map(e => `${e.title} ${e.version_number}${e.dependency ? ' (required dependency)' : ''}`).join(', ')}.`;
    setTimeout(loadExtensions, 1500);
  });
  const removeExtension = (extension: Extension) => act(async () => {
    await api(`/api/servers/${serverId}/extensions/${encodeURIComponent(extension.project_id)}`, { method: 'DELETE' });
    notice = `Removing ${extension.title}.`;
    setTimeout(loadExtensions, 1500);
  });
  async function loadActivity() {
    try { activity = await api<Activity[]>(`/api/servers/${serverId}/activity`); } catch (reason) { report(reason); }
  }
  function open(next: typeof tab) {
    tab = next; error = ''; notice = '';
    if (next === 'mods') void loadExtensions();
    if (next === 'activity') void loadActivity();
  }
  const retryInstall = () => act(async () => { await api(`/api/servers/${serverId}/reinstall`, { method: 'POST' }); }, 'Installation queued again.');
  async function remove(event: SubmitEvent) {
    event.preventDefault();
    await act(async () => { await api(`/api/servers/${serverId}`, { method: 'DELETE', body: JSON.stringify({ confirm_name: confirmName }) }); }, 'Deleting the server…');
  }
</script>

<section class="page-content workspace-page">
  <div class="page-heading">
    <div>
      <button class="link-button" onclick={onBack}><Icon name="chevron" size={13} /> Servers</button>
      <h1>{detail?.name ?? 'Server'}</h1>
      {#if detail?.details}
        <p>{detail.details.egg_name}{#if runtime?.game} · Minecraft {runtime.game.version}{runtime.game.loader ? ` with ${runtime.game.loader}` : ''}{/if} · {detail.details.binding.ip}:{detail.details.binding.port}</p>
      {/if}
    </div>
    {#if detail?.details}
      <span class="status-pill" data-state={install === 'installed' ? power : install} role="status">{statusLabel}</span>
    {/if}
  </div>

  {#if error}<div class="feedback error inline" role="alert">{error}</div>{/if}
  {#if notice}<div class="feedback success inline" role="status">{notice}</div>{/if}

  {#if detail && !detail.details}
    <section class="panel"><h2>Not managed here</h2><p>This server has no installation in this panel.</p></section>
  {:else if detail?.details}
    {#if runtime?.problem}<div class="feedback warning inline" role="alert">{runtime.problem}</div>{/if}
    {#if detail.runtime_error}<div class="feedback warning inline">{detail.runtime_error}</div>{/if}
    {#if install === 'failed'}
      <section class="panel install-panel"><div><h2>Installation failed</h2><pre class="install-error">{detail.details.install_message}</pre></div><button class="primary" disabled={busy} onclick={retryInstall}>Retry install</button></section>
    {:else if install !== 'installed'}
      <section class="panel install-panel"><div><h2>{install === 'pending' ? 'Waiting to install' : 'Installing'}</h2><p>{runtime?.operation?.message ?? 'The installer will start shortly.'}</p></div></section>
    {/if}

    <div class="stats">
      <div><span>Memory limit</span><strong>{gib(detail.details.limits.memory_bytes)}</strong></div>
      <div><span>CPU limit</span><strong>{cores(detail.details.limits.cpu_millis)}</strong></div>
      <div><span>Disk</span><strong>{runtime?.disk_used_bytes != null ? `${gib(runtime.disk_used_bytes)} of ${gib(runtime.disk_total_bytes ?? detail.details.limits.disk_bytes)}` : gib(detail.details.limits.disk_bytes)}</strong></div>
    </div>

    <div class="power-bar" aria-label="Power controls">
      <button class="primary" disabled={busy || !installed || !stopped || !!operation} onclick={() => powerAction('start')}><Icon name="play" size={14} /> Start</button>
      <button class="secondary" disabled={busy || !installed || power === 'offline' || power === 'stopping'} onclick={() => powerAction('stop')}><Icon name="stop" size={14} /> Stop</button>
      <button class="secondary" disabled={busy || !installed || power !== 'running'} onclick={() => powerAction('restart')}><Icon name="restart" size={14} /> Restart</button>
      <button class="secondary danger" disabled={busy || !installed || power === 'offline'} onclick={() => powerAction('kill')}>Force stop</button>
    </div>

    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={tab === 'console'} class:active={tab === 'console'} onclick={() => open('console')}><Icon name="terminal" size={15} /> Console</button>
      <button role="tab" aria-selected={tab === 'mods'} class:active={tab === 'mods'} disabled={!installed} onclick={() => open('mods')}><Icon name="puzzle" size={15} /> Mods</button>
      <button role="tab" aria-selected={tab === 'activity'} class:active={tab === 'activity'} onclick={() => open('activity')}><Icon name="activity" size={15} /> Activity</button>
      <button role="tab" aria-selected={tab === 'settings'} class:active={tab === 'settings'} onclick={() => open('settings')}><Icon name="trash" size={15} /> Delete</button>
    </div>

    {#if tab === 'console'}
      <section class="console-panel" aria-label="Console">
        <!-- svelte-ignore a11y_no_noninteractive_tabindex (scrollable output must be reachable by keyboard) -->
        <div class="console-output" role="log" aria-label="Console output" bind:this={consoleBox} onscroll={scrolled} tabindex="0" aria-live="off">
          {#if !lines.length}<p class="muted">{install === 'installed' ? 'No output yet. Start the server to see its console.' : 'Installer output appears here.'}</p>{/if}
          {#each lines as line, index (index)}<div>{line}</div>{/each}
        </div>
        <form class="console-input" onsubmit={send}>
          <span aria-hidden="true">›</span>
          <input aria-label="Console command" placeholder={power === 'offline' ? 'Start the server to send commands' : 'Type a command, e.g. list'} bind:value={command} maxlength="512" disabled={power === 'offline' || power === 'stopping'} autocomplete="off" />
          <button class="secondary" disabled={busy || !command.trim() || power === 'offline' || power === 'stopping'}>Send</button>
        </form>
      </section>
    {:else if tab === 'mods'}
      <section class="panel mods-panel">
        {#if !stopped}<div class="feedback warning inline">Stop the server before adding or removing mods. Running servers keep their loaded mods.</div>{/if}
        {#if extensions && !extensions.available}
          <p>{extensions.reason}</p>
        {:else if extensions}
          <p class="muted">Showing Modrinth projects that support {extensions.game?.loader} on Minecraft {extensions.game?.version} and can run on a server. Required dependencies are added automatically.</p>
          <form class="mod-search" onsubmit={search}>
            <input aria-label="Search Modrinth" placeholder="Search Modrinth…" bind:value={query} />
            <button class="secondary" disabled={busy}>Search</button>
          </form>
          {#if hits.length}
            <ul class="mod-list">
              {#each hits as hit (hit.project_id)}
                <li><div><strong>{hit.title}</strong><small>{hit.description}</small></div>
                  <button class="primary" disabled={busy || !stopped || !!operation} onclick={() => installExtension(hit)}>{extensions.managed.some(m => m.project_id === hit.project_id) ? 'Update' : 'Install'}</button></li>
              {/each}
            </ul>
          {/if}
          <h3>Installed</h3>
          {#if !extensions.managed.length && !extensions.other_files.length}<p class="muted">No mods installed yet.</p>{/if}
          <ul class="mod-list">
            {#each extensions.managed as extension (extension.project_id)}
              <li><div><strong>{extension.title}</strong><small>{extension.version_number} · {extension.filename}{extension.dependency ? ' · dependency' : ''}</small></div>
                <button class="secondary" aria-label={`Remove ${extension.title}`} disabled={busy || !stopped || !!operation} onclick={() => removeExtension(extension)}>Remove</button></li>
            {/each}
            {#each extensions.other_files as name (name)}
              <li><div><strong>{name}</strong><small>Added outside this panel; compatibility unknown</small></div></li>
            {/each}
          </ul>
        {:else}
          <p class="muted">Loading mods…</p>
        {/if}
      </section>
    {:else if tab === 'activity'}
      <section class="panel">
        {#if !activity.length}<p class="muted">No activity yet.</p>{/if}
        <ul class="activity-list">
          {#each activity as entry, index (index)}
            <li><span class="outcome" data-outcome={entry.outcome}></span><div><strong>{entry.action.replaceAll('_', ' ')}</strong> <span class="muted">{entry.outcome}{entry.actor ? ` by ${entry.actor}` : ''}</span>{#if entry.detail}<small>{entry.detail}</small>{/if}</div><time>{when(entry.at)}</time></li>
          {/each}
        </ul>
      </section>
    {:else}
      <form class="panel danger-panel" onsubmit={remove}>
        <h2>Delete this server</h2>
        <p>The server is stopped and its files, world and mods are permanently removed. Its address and reserved resources become available again.</p>
        <label for="confirm-name">Type <strong>{detail.name}</strong> to confirm</label>
        <input id="confirm-name" bind:value={confirmName} autocomplete="off" />
        <button class="secondary danger" disabled={busy || confirmName !== detail.name || detail.details.deleting}>Delete server</button>
      </form>
    {/if}
  {:else}
    <div class="center-state" role="status">Loading server…</div>
  {/if}
</section>
