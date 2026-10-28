<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import { api, ApiError } from '../api';
  import Icon from '../components/Icon.svelte';

  type Resources = { memory_bytes: number; disk_bytes: number; cpu_millis: number };
  type Operation = { kind: 'install' | 'extensions' | 'delete' | 'backup' | 'restore' | 'update'; state: 'running' | 'succeeded' | 'failed'; message: string };
  type Runtime = {
    installed: boolean; operation: Operation | null; power: 'offline' | 'starting' | 'running' | 'stopping';
    desired_running: boolean; problem: string | null; started_at: number | null; exit_code: number | null;
    game: { version: string; loader: string | null } | null; disk_used_bytes: number | null; disk_total_bytes: number | null;
    usage: null | { cpu_millis: number; memory_bytes: number; memory_limit_bytes: number };
  };
  type Detail = {
    id: string; name: string; owner: string; is_owner: boolean; runtime: Runtime | null; runtime_error: string | null;
    details: null | {
      egg_name: string; limits: Resources; binding: { ip: string; port: number; transport: string };
      install_state: 'pending' | 'installing' | 'installed' | 'failed'; install_message: string | null; deleting: boolean;
    };
  };
  type Extension = { project_id: string; title: string; version_number: string; filename: string; dependency: boolean };
  type Extensions = { game: Runtime['game']; available: boolean; reason: string | null; managed: Extension[]; other_files: string[] };
  type Hit = { project_id: string; title: string; description: string; downloads: number };
  type Activity = { at: number; actor: string | null; action: string; outcome: string; detail: string | null };
  type FileEntry = { name: string; kind: 'file' | 'directory' | 'link' | 'other'; size: number; modified: number };
  type Variable = { env_variable: string; name: string; description: string; value: string; editable: boolean };
  type Settings = { image: string; images: [string, string][]; startup: string; variables: Variable[] };
  type Member = { user_id: string; username: string; enabled: boolean };
  type Person = { id: string; username: string };
  type Schedule = { id?: string; action: 'restart' | 'backup' | 'command' | 'update'; command: string | null; hour: number; minute: number; weekdays: number; timezone: string; enabled: boolean; next_run_at?: number; last_run_at?: number | null; last_outcome?: string | null };
  type Backup = { id: string; kind: 'manual' | 'pre_restore' | 'pre_update'; created_at: number; size_bytes: number };

  let { serverId, onBack, onDeleted }: { serverId: string; onBack: () => void; onDeleted: (name: string) => void } = $props();

  let detail = $state<Detail | null>(null);
  let tab = $state<'console' | 'files' | 'mods' | 'backups' | 'schedules' | 'settings' | 'members' | 'activity' | 'delete'>('console');
  const localZone = (() => { try { return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'; } catch { return 'UTC'; } })();
  const blankSchedule = (): Schedule => ({ action: 'restart', command: '', hour: 4, minute: 0, weekdays: 127, timezone: localZone, enabled: true });
  let schedules = $state<Schedule[]>([]);
  let draft = $state<Schedule | null>(null);
  let settings = $state<Settings | null>(null);
  let limits = $state({ memory: 0, cpu: 0, disk: 0 });
  let members = $state<Member[]>([]);
  let people = $state<Person[]>([]);
  let grantTo = $state('');
  let folder = $state('');
  let entries = $state<FileEntry[]>([]);
  let editing = $state<{ path: string; text: string; version: string | null } | null>(null);
  let uploadInput = $state<HTMLInputElement | undefined>();
  let backups = $state<Backup[]>([]);
  let retention = $state(3);
  let keepBackups = $state(false);
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
    operation?.kind === 'backup' ? 'Backing up' : operation?.kind === 'restore' ? 'Restoring' :
    operation?.kind === 'update' ? 'Updating' :
    { offline: 'Offline', starting: 'Starting', running: 'Running', stopping: 'Stopping' }[power]);

  function gameLabel(game: Runtime['game']) {
    if (!game) return '';
    if (game.loader === 'bedrock') return game.version === 'Bedrock' ? 'Minecraft Bedrock Edition' : `Minecraft Bedrock Edition ${game.version}`;
    return `Minecraft ${game.version}${game.loader ? ` with ${game.loader}` : ''}`;
  }
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
  // Commands behave like a terminal: the input clears at once and commands are sent in order,
  // so typing the next one never waits for (or is blocked by) the previous request.
  let outgoing: Promise<void> = Promise.resolve();
  function send(event: SubmitEvent) {
    event.preventDefault();
    const text = command.trim();
    if (!text) return;
    command = '';
    error = '';
    outgoing = outgoing.then(async () => {
      try {
        await api(`/api/servers/${serverId}/console`, { method: 'POST', body: JSON.stringify({ command: text }) });
        void refresh();
      } catch (reason) {
        report(reason);
        if (!command) command = text;
      }
    });
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
  async function loadBackups() {
    try {
      const result = await api<{ retention: number; backups: Backup[] }>(`/api/servers/${serverId}/backups`);
      backups = result.backups; retention = result.retention;
    } catch (reason) { report(reason); }
  }
  const createBackup = () => act(async () => {
    await api(`/api/servers/${serverId}/backups`, { method: 'POST' });
    notice = power === 'offline' ? 'Creating a backup.' : 'Creating a backup. The server stops while the archive is written and starts again afterwards.';
    setTimeout(loadBackups, 3000);
  });
  const changeRetention = (value: number) => act(async () => {
    const manual = backups.filter(b => b.kind === 'manual').length;
    let removeOldest = false;
    if (manual > value) {
      if (!confirm(`Keeping ${value} backup${value === 1 ? '' : 's'} removes the ${manual - value} oldest now. Continue?`)) { retention = retention; return; }
      removeOldest = true;
    }
    await api(`/api/servers/${serverId}/backups/retention`, { method: 'PUT', body: JSON.stringify({ retention: value, remove_oldest: removeOldest }) });
    await loadBackups();
  });
  const restoreBackup = (backup: Backup) => act(async () => {
    if (!confirm(`Replace the server's current files with the backup from ${when(backup.created_at)}? The current files are saved as a pre-restore backup first.`)) return;
    await api(`/api/servers/${serverId}/backups/${backup.id}/restore`, { method: 'POST', body: JSON.stringify({ confirm: true }) });
    notice = 'Restoring the backup.';
  });
  const deleteBackup = (backup: Backup) => act(async () => {
    if (!confirm(`Delete the backup from ${when(backup.created_at)}? This cannot be undone.`)) return;
    await api(`/api/servers/${serverId}/backups/${backup.id}`, { method: 'DELETE' });
    await loadBackups();
  });
  async function loadSettings() {
    try {
      settings = await api<Settings>(`/api/servers/${serverId}/settings`);
      const current = detail?.details?.limits;
      if (current) limits = { memory: current.memory_bytes / 1024 ** 3, cpu: current.cpu_millis / 1000, disk: current.disk_bytes / 1024 ** 3 };
    } catch (reason) { report(reason); }
  }
  const later = () => (power === 'offline' ? 'They apply the next time the server starts.' : 'They apply after the next restart.');
  const saveSettings = (event: SubmitEvent) => act(async () => {
    event.preventDefault();
    if (!settings) return;
    const variables = Object.fromEntries(settings.variables.filter(v => v.editable).map(v => [v.env_variable, v.value]));
    const result = await api<{ changed: string[] }>(`/api/servers/${serverId}/settings`, { method: 'PUT', body: JSON.stringify({ image: settings.image, variables }) });
    notice = result.changed.length ? `Saved ${result.changed.length} change${result.changed.length === 1 ? '' : 's'}. ${later()}` : 'Nothing changed.';
    await loadSettings();
  });
  const saveLimits = (event: SubmitEvent) => act(async () => {
    event.preventDefault();
    const body = { memory_bytes: Math.round(limits.memory * 1024 ** 3), disk_bytes: Math.round(limits.disk * 1024 ** 3), cpu_millis: Math.round(limits.cpu * 1000) };
    await api(`/api/servers/${serverId}/limits`, { method: 'PUT', body: JSON.stringify(body) });
    notice = `Limits saved. ${later()}`;
  });
  async function loadMembers() {
    try {
      [members, people] = await Promise.all([api<Member[]>(`/api/servers/${serverId}/members`), api<Person[]>('/api/users')]);
    } catch (reason) { report(reason); }
  }
  const candidates = $derived(people.filter(p => p.id !== detail?.owner && !members.some(m => m.enabled && m.user_id === p.id)));
  const setAccess = (userId: string, enabled: boolean, name: string) => act(async () => {
    if (!enabled && !confirm(`Remove ${name}'s access? They lose the console, files and backups of this server at once.`)) return;
    await api(`/api/servers/${serverId}/members/${userId}`, { method: 'PUT', body: JSON.stringify({ enabled }) });
    grantTo = '';
    notice = enabled ? `${name} can now manage this server.` : `${name} no longer has access.`;
    await loadMembers();
  });
  const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  const ACTIONS = { restart: 'Restart', backup: 'Back up', command: 'Run command', update: 'Update (same game version)' };
  function days(mask: number) { return mask === 127 ? 'Every day' : mask === 31 ? 'Weekdays' : DAYS.filter((_, i) => mask & (1 << i)).join(', '); }
  const pad = (n: number) => String(n).padStart(2, '0');
  async function loadSchedules() {
    try { schedules = await api<Schedule[]>(`/api/servers/${serverId}/schedules`); } catch (reason) { report(reason); }
  }
  const saveSchedule = (event: SubmitEvent) => act(async () => {
    event.preventDefault();
    if (!draft) return;
    const { id, next_run_at, last_run_at, last_outcome, ...body } = draft;
    const payload = { ...body, command: body.action === 'command' ? body.command : null };
    if (id) await api(`/api/servers/${serverId}/schedules/${id}`, { method: 'PUT', body: JSON.stringify(payload) });
    else await api(`/api/servers/${serverId}/schedules`, { method: 'POST', body: JSON.stringify(payload) });
    draft = null;
    notice = 'Schedule saved.';
    await loadSchedules();
  });
  const removeSchedule = (schedule: Schedule) => act(async () => {
    if (!confirm('Delete this schedule?')) return;
    await api(`/api/servers/${serverId}/schedules/${schedule.id}`, { method: 'DELETE' });
    await loadSchedules();
  });
  const updateServer = () => act(async () => {
    if (!confirm('Update the server? It runs the installer again with the current startup settings. A backup is made first and the previous files are put back if the update fails. A running server is stopped and started again.')) return;
    await api(`/api/servers/${serverId}/update`, { method: 'POST' });
    notice = 'Updating. Check mods afterwards if the game version changed.';
  });
  function size(bytes: number) { return bytes >= 1024 ** 3 ? gib(bytes) : `${Math.max(1, Math.round(bytes / 1024 ** 2))} MiB`; }

  const join = (name: string) => (folder ? `${folder}/${name}` : name);
  async function loadFolder(path = folder) {
    try { entries = await api<FileEntry[]>(`/api/servers/${serverId}/files?path=${encodeURIComponent(path)}`); folder = path; editing = null; }
    catch (reason) { report(reason); }
  }
  const openEntry = (entry: FileEntry) => act(async () => {
    if (entry.kind === 'directory') { await loadFolder(join(entry.name)); return; }
    if (entry.kind !== 'file') return;
    const file = await api<{ text: string; version: string }>(`/api/servers/${serverId}/files/text?path=${encodeURIComponent(join(entry.name))}`);
    editing = { path: join(entry.name), text: file.text, version: file.version };
  });
  const saveFile = () => act(async () => {
    if (!editing) return;
    const saved = await api<{ version: string }>(`/api/servers/${serverId}/files/text`, { method: 'PUT', body: JSON.stringify({ path: editing.path, text: editing.text, version: editing.version }) });
    editing = { ...editing, version: saved.version };
    notice = `Saved ${editing.path}.`;
  });
  const newFile = () => act(async () => {
    const name = prompt('New file name');
    if (!name) return;
    editing = { path: join(name.trim()), text: '', version: null };
  });
  const newFolder = () => act(async () => {
    const name = prompt('New folder name');
    if (!name) return;
    await api(`/api/servers/${serverId}/files/folder`, { method: 'POST', body: JSON.stringify({ path: join(name.trim()) }) });
    await loadFolder();
  });
  const renameEntry = (entry: FileEntry) => act(async () => {
    const name = prompt(`Rename ${entry.name} to`, entry.name);
    if (!name || name === entry.name) return;
    await api(`/api/servers/${serverId}/files/rename`, { method: 'POST', body: JSON.stringify({ from: join(entry.name), to: join(name.trim()) }) });
    await loadFolder();
  });
  const deleteEntry = (entry: FileEntry) => act(async () => {
    if (!confirm(`Delete ${entry.name}${entry.kind === 'directory' ? ' and everything in it' : ''}? This cannot be undone.`)) return;
    await api(`/api/servers/${serverId}/files?path=${encodeURIComponent(join(entry.name))}`, { method: 'DELETE' });
    await loadFolder();
  });
  const uploadFiles = (list: FileList | null) => act(async () => {
    for (const file of Array.from(list ?? [])) {
      const response = await fetch(`/api/servers/${serverId}/files/upload?path=${encodeURIComponent(join(file.name))}`, { method: 'POST', body: file, credentials: 'same-origin' });
      if (!response.ok) {
        const body = await response.json().catch(() => null);
        throw new ApiError(response.status, body?.message ?? `Upload of ${file.name} failed`);
      }
    }
    notice = 'Upload finished.';
    if (uploadInput) uploadInput.value = '';
    await loadFolder();
  });
  const crumbs = $derived(folder ? folder.split('/').map((name, index, parts) => ({ name, path: parts.slice(0, index + 1).join('/') })) : []);

  function open(next: typeof tab) {
    tab = next; error = ''; notice = '';
    if (next === 'mods') void loadExtensions();
    if (next === 'files') void loadFolder('');
    if (next === 'backups') void loadBackups();
    if (next === 'activity') void loadActivity();
    if (next === 'settings') void loadSettings();
    if (next === 'members') void loadMembers();
    if (next === 'schedules') { draft = null; void loadSchedules(); }
  }
  const retryInstall = () => act(async () => { await api(`/api/servers/${serverId}/reinstall`, { method: 'POST' }); }, 'Installation queued again.');
  async function remove(event: SubmitEvent) {
    event.preventDefault();
    await act(async () => { await api(`/api/servers/${serverId}`, { method: 'DELETE', body: JSON.stringify({ confirm_name: confirmName, keep_backups: keepBackups }) }); }, 'Deleting the server…');
  }
</script>

<section class="page-content workspace-page">
  <div class="page-heading">
    <div>
      <button class="link-button" onclick={onBack}><Icon name="chevron" size={13} /> Servers</button>
      <h1>{detail?.name ?? 'Server'}</h1>
      {#if detail?.details}
        <p>{[detail.details.egg_name, gameLabel(runtime?.game ?? null), `${detail.details.binding.ip}:${detail.details.binding.port}`].filter(Boolean).join(' · ')}</p>
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
      <div><span>Memory</span><strong>{runtime?.usage ? `${gib(runtime.usage.memory_bytes)} of ${gib(detail.details.limits.memory_bytes)}` : `${gib(detail.details.limits.memory_bytes)} limit`}</strong></div>
      <div><span>CPU</span><strong>{runtime?.usage ? `${(runtime.usage.cpu_millis / 10).toFixed(0)}% of ${cores(detail.details.limits.cpu_millis)}` : `${cores(detail.details.limits.cpu_millis)} limit`}</strong></div>
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
      <button role="tab" aria-selected={tab === 'files'} class:active={tab === 'files'} disabled={!installed} onclick={() => open('files')}><Icon name="folder" size={15} /> Files</button>
      <button role="tab" aria-selected={tab === 'mods'} class:active={tab === 'mods'} disabled={!installed} onclick={() => open('mods')}><Icon name="puzzle" size={15} /> Mods</button>
      <button role="tab" aria-selected={tab === 'backups'} class:active={tab === 'backups'} disabled={!installed} onclick={() => open('backups')}><Icon name="archive" size={15} /> Backups</button>
      <button role="tab" aria-selected={tab === 'schedules'} class:active={tab === 'schedules'} disabled={!installed} onclick={() => open('schedules')}><Icon name="clock" size={15} /> Schedules</button>
      <button role="tab" aria-selected={tab === 'settings'} class:active={tab === 'settings'} onclick={() => open('settings')}><Icon name="sliders" size={15} /> Settings</button>
      <button role="tab" aria-selected={tab === 'members'} class:active={tab === 'members'} onclick={() => open('members')}><Icon name="users" size={15} /> Members</button>
      <button role="tab" aria-selected={tab === 'activity'} class:active={tab === 'activity'} onclick={() => open('activity')}><Icon name="activity" size={15} /> Activity</button>
      <button role="tab" aria-selected={tab === 'delete'} class:active={tab === 'delete'} onclick={() => open('delete')}><Icon name="trash" size={15} /> Delete</button>
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
          <button class="secondary" disabled={!command.trim() || power === 'offline' || power === 'stopping'}>Send</button>
        </form>
      </section>
    {:else if tab === 'files'}
      <section class="panel files-panel" aria-label="Files">
        {#if editing}
          <div class="editor-heading"><h2>{editing.path}</h2>
            <div class="row-actions"><button class="secondary" onclick={() => { editing = null; }}>Close</button><button class="primary" disabled={busy} onclick={saveFile}>Save</button></div></div>
          <textarea class="editor" aria-label={`Contents of ${editing.path}`} bind:value={editing.text} spellcheck="false"></textarea>
          <p class="muted">Changes apply when the server next reads the file; most settings need a restart.</p>
        {:else}
          <div class="files-toolbar">
            <nav class="crumbs" aria-label="Folder"><button class="link-button" onclick={() => loadFolder('')}>Server files</button>{#each crumbs as crumb (crumb.path)}<span>/</span><button class="link-button" onclick={() => loadFolder(crumb.path)}>{crumb.name}</button>{/each}</nav>
            <div class="row-actions">
              <button class="secondary" disabled={busy} onclick={newFile}>New file</button>
              <button class="secondary" disabled={busy} onclick={newFolder}>New folder</button>
              <label class="secondary upload-button">Upload<input bind:this={uploadInput} type="file" multiple onchange={(event) => uploadFiles(event.currentTarget.files)} /></label>
            </div>
          </div>
          {#if !entries.length}<p class="muted">This folder is empty.</p>{/if}
          <ul class="mod-list file-list">
            {#each entries as entry (entry.name)}
              <li>
                <button class="file-name" aria-label={entry.kind === 'directory' ? `Open folder ${entry.name}` : entry.kind === 'file' ? `Open ${entry.name}` : `${entry.name} (link, not followed)`} disabled={entry.kind !== 'file' && entry.kind !== 'directory'} onclick={() => openEntry(entry)}>
                  <Icon name={entry.kind === 'directory' ? 'folder' : 'file'} size={15} /><span><strong>{entry.name}</strong><small>{entry.kind === 'directory' ? 'Folder' : entry.kind === 'file' ? size(entry.size) : 'Link (not followed)'}</small></span>
                </button>
                <div class="row-actions">
                  {#if entry.kind === 'file'}<a class="secondary" href={`/api/servers/${serverId}/files/download?path=${encodeURIComponent(join(entry.name))}`} download aria-label={`Download ${entry.name}`}>Download</a>{/if}
                  <button class="secondary" aria-label={`Rename ${entry.name}`} disabled={busy} onclick={() => renameEntry(entry)}>Rename</button>
                  <button class="secondary danger" aria-label={`Delete ${entry.name}`} disabled={busy} onclick={() => deleteEntry(entry)}>Delete</button>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
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
                  <button class="primary" disabled={busy || !stopped || !!operation} onclick={() => installExtension(hit)}>{extensions.managed.some(m => m.project_id === hit.project_id) ? 'Check for update' : 'Install'}</button></li>
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
    {:else if tab === 'backups'}
      <section class="panel backups-panel">
        <div class="backup-heading">
          <div><h2>Backups</h2><p>Stored on this host. A running server stops while its backup is written, then starts again. Local backups do not protect against a failed disk.</p></div>
          <button class="primary" disabled={busy || !!operation} onclick={createBackup}>Create backup</button>
        </div>
        <label class="retention" for="retention">Keep
          <select id="retention" value={retention} disabled={busy} onchange={(event) => changeRetention(Number(event.currentTarget.value))}>
            {#each [1, 2, 3, 4, 5] as count}<option value={count}>{count}</option>{/each}
          </select>
          backups; the oldest is replaced only after a new backup succeeds.</label>
        {#if !backups.length}<p class="muted">No backups yet.</p>{/if}
        <ul class="mod-list">
          {#each backups as backup (backup.id)}
            <li><div><strong>{when(backup.created_at)}</strong><small>{size(backup.size_bytes)}{backup.kind === 'pre_restore' ? ' · saved automatically before a restore' : backup.kind === 'pre_update' ? ' · saved automatically before an update' : ''}</small></div>
              <div class="row-actions">
                <a class="secondary" href={`/api/servers/${serverId}/backups/${backup.id}/download`} download>Download</a>
                <button class="secondary" aria-label={`Restore backup from ${when(backup.created_at)}`} disabled={busy || !stopped || !!operation} onclick={() => restoreBackup(backup)}>Restore</button>
                <button class="secondary danger" aria-label={`Delete backup from ${when(backup.created_at)}`} disabled={busy || !!operation} onclick={() => deleteBackup(backup)}>Delete</button>
              </div></li>
          {/each}
        </ul>
        {#if !stopped && backups.length}<p class="muted">Stop the server to restore a backup.</p>{/if}
      </section>
    {:else if tab === 'schedules'}
      <section class="panel schedules-panel" aria-label="Schedules">
        <div class="backup-heading">
          <div><h2>Schedules</h2><p>Run restarts, backups, console commands or updates at a set time. Times use the chosen time zone, including daylight saving. A run missed while the panel was down is skipped, not run late.</p></div>
          <button class="primary" disabled={busy || !!draft} onclick={() => { draft = blankSchedule(); }}>New schedule</button>
        </div>
        {#if draft}
          <form class="settings-panel schedule-form" onsubmit={saveSchedule} aria-label="Schedule">
            <label for="schedule-action">Action</label>
            <select id="schedule-action" bind:value={draft.action}>
              {#each Object.entries(ACTIONS) as [value, label] (value)}<option {value}>{label}</option>{/each}
            </select>
            {#if draft.action === 'command'}
              <label for="schedule-command">Console command</label>
              <input id="schedule-command" bind:value={draft.command} maxlength="512" placeholder="say Restarting soon" autocomplete="off" />
            {/if}
            {#if draft.action === 'update'}<small class="muted">Scheduled updates install newer builds of the current game version only; a backup is made first and a failed update is rolled back.</small>{/if}
            <label for="schedule-time">Time</label>
            <input id="schedule-time" type="time" value={`${pad(draft.hour)}:${pad(draft.minute)}`} onchange={(event) => { const [h, m] = event.currentTarget.value.split(':').map(Number); if (draft && !isNaN(h) && !isNaN(m)) { draft.hour = h; draft.minute = m; } }} />
            <fieldset class="weekdays"><legend>Days</legend>
              {#each DAYS as day, index (day)}
                <label class="checkbox"><input type="checkbox" checked={(draft.weekdays & (1 << index)) !== 0} onchange={(event) => { if (draft) draft.weekdays = event.currentTarget.checked ? draft.weekdays | (1 << index) : draft.weekdays & ~(1 << index); }} /> {day}</label>
              {/each}
            </fieldset>
            <label for="schedule-zone">Time zone</label>
            <input id="schedule-zone" bind:value={draft.timezone} autocomplete="off" spellcheck="false" />
            <label class="checkbox"><input type="checkbox" bind:checked={draft.enabled} /> Enabled</label>
            <div class="row-actions"><button type="button" class="secondary" onclick={() => { draft = null; }}>Cancel</button><button class="primary" disabled={busy || !draft.weekdays}>Save schedule</button></div>
          </form>
        {/if}
        {#if !schedules.length && !draft}<p class="muted">No schedules yet.</p>{/if}
        <ul class="mod-list">
          {#each schedules as schedule (schedule.id)}
            <li><div><strong>{ACTIONS[schedule.action]}{schedule.action === 'command' ? `: ${schedule.command}` : ''}</strong>
              <small>{days(schedule.weekdays)} at {pad(schedule.hour)}:{pad(schedule.minute)} ({schedule.timezone}){schedule.enabled ? ` · next ${when(schedule.next_run_at ?? 0)}` : ' · paused'}{schedule.last_outcome ? ` · last: ${schedule.last_outcome}` : ''}</small></div>
              <div class="row-actions">
                <button class="secondary" aria-label={`Edit schedule ${ACTIONS[schedule.action]}`} disabled={busy} onclick={() => { draft = { ...schedule, command: schedule.command ?? '' }; }}>Edit</button>
                <button class="secondary danger" aria-label={`Delete schedule ${ACTIONS[schedule.action]}`} disabled={busy} onclick={() => removeSchedule(schedule)}>Delete</button>
              </div></li>
          {/each}
        </ul>
      </section>
    {:else if tab === 'settings'}
      <div class="settings-grid">
        <form class="panel settings-panel" onsubmit={saveSettings} aria-label="Startup settings">
          <h2>Startup</h2>
          {#if settings}
            <label for="image">Runtime image</label>
            <select id="image" bind:value={settings.image}>
              {#each settings.images as [label, image] (image)}<option value={image}>{label}</option>{/each}
            </select>
            {#each settings.variables as variable (variable.env_variable)}
              <label for={`var-${variable.env_variable}`}>{variable.name}</label>
              <input id={`var-${variable.env_variable}`} bind:value={variable.value} readonly={!variable.editable} autocomplete="off" spellcheck="false" />
              {#if variable.description}<small class="muted">{variable.description}{variable.editable ? '' : ' (fixed by the egg)'}</small>{/if}
            {/each}
            <details><summary>Startup command</summary><code class="startup">{settings.startup}</code></details>
            <button class="primary" disabled={busy}>Save startup settings</button>
            <p class="muted">Changing the image downloads it at the next start. To install a different game version or the newest build, save the version here and run an update.</p>
            <button type="button" class="secondary" disabled={busy || !installed || !!operation} onclick={updateServer}>Update server</button>
          {:else}<p class="muted">Loading settings…</p>{/if}
        </form>
        <form class="panel settings-panel" onsubmit={saveLimits} aria-label="Resource limits">
          <h2>Resource limits</h2>
          <label for="limit-memory">Memory (GiB)</label>
          <input id="limit-memory" type="number" min="0.5" step="0.5" bind:value={limits.memory} />
          <label for="limit-cpu">CPU (cores)</label>
          <input id="limit-cpu" type="number" min="0.25" step="0.25" bind:value={limits.cpu} />
          <label for="limit-disk">Disk (GiB)</label>
          <input id="limit-disk" type="number" min={(detail.details.limits.disk_bytes / 1024 ** 3)} step="1" bind:value={limits.disk} />
          <small class="muted">Disk can only grow; it is enlarged the next time the server starts. Limits must fit in the host's safe budget.</small>
          <button class="primary" disabled={busy}>Save limits</button>
        </form>
      </div>
    {:else if tab === 'members'}
      <section class="panel members-panel" aria-label="Members">
        <h2>Who can manage this server</h2>
        <p class="muted">Members can use the console, files, mods, backups and settings, and delete the server. Administrators always have access.</p>
        <ul class="mod-list">
          {#each people.filter(p => p.id === detail?.owner) as owner (owner.id)}<li><div><strong>{owner.username}</strong><small>Owner</small></div></li>{/each}
          {#each members.filter(m => m.enabled) as member (member.user_id)}
            <li><div><strong>{member.username}</strong><small>Member</small></div>
              <button class="secondary danger" aria-label={`Remove ${member.username}`} disabled={busy} onclick={() => setAccess(member.user_id, false, member.username)}>Remove</button></li>
          {/each}
        </ul>
        <form class="mod-search" onsubmit={(event) => { event.preventDefault(); const person = people.find(p => p.id === grantTo); if (person) setAccess(person.id, true, person.username); }}>
          <select aria-label="Teammate" bind:value={grantTo}>
            <option value="">Choose a teammate…</option>
            {#each candidates as person (person.id)}<option value={person.id}>{person.username}</option>{/each}
          </select>
          <button class="primary" disabled={busy || !grantTo}>Give access</button>
        </form>
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
        <p>The server is stopped and its files, world and mods are permanently removed. Its address and reserved resources become available again. Backups are removed too unless you keep them.</p>
        <label for="confirm-name">Type <strong>{detail.name}</strong> to confirm</label>
        <input id="confirm-name" bind:value={confirmName} autocomplete="off" />
        <label class="checkbox"><input type="checkbox" bind:checked={keepBackups} /> Keep this server's backups (people with access now can still download them)</label>
        <button class="secondary danger" disabled={busy || confirmName !== detail.name || detail.details.deleting}>Delete server</button>
      </form>
    {/if}
  {:else}
    <div class="center-state" role="status">Loading server…</div>
  {/if}
</section>
