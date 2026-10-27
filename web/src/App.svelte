<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from './api';
  import Icon from './components/Icon.svelte';

  type Session = { id: string; is_admin: boolean };
  type Server = { id: string; name: string; can_access: boolean; usage: null | { cpu_percent: number; memory_bytes: number; disk_bytes: number } };
  type Egg = { id: string; name: string; revision: string; runtime_qualified: boolean };
  type EggDetails = { name: string; description: string; author: string; images: [string, string][]; variables: { name: string; env_variable: string }[] };
  let session = $state<Session | null>(null);
  let checking = $state(true);
  let busy = $state(false);
  let page = $state<'servers' | 'library' | 'team' | 'server'>('servers');
  let servers = $state<Server[]>([]);
  let eggs = $state<Egg[]>([]);
  let username = $state('');
  let password = $state('');
  let error = $state('');
  let notice = $state('');
  let query = $state('');
  let selectedFile = $state<File | undefined>();
  let selectedEgg = $state<EggDetails | null>(null);
  let selectedServer = $state<{ id: string; name: string } | null>(null);
  let inviteLink = $state('');
  let createOpen = $state(false);
  let inviteToken = $state(new URLSearchParams(window.location.hash.slice(1)).get('invite') ?? '');
  const filteredServers = $derived(servers.filter(server => server.name.toLowerCase().includes(query.toLowerCase())));
  const pageTitle = $derived(page === 'library' ? 'Egg library' : page === 'team' ? 'Team' : page === 'server' ? selectedServer?.name ?? 'Server' : 'Servers');

  function showError(reason: unknown) {
    error = reason instanceof Error ? reason.message : 'Something went wrong. Please try again.';
    if (reason instanceof ApiError && reason.status === 401 && session) { session = null; servers = []; eggs = []; }
  }
  async function loadData() {
    [servers, eggs] = await Promise.all([api<Server[]>('/api/servers'), api<Egg[]>('/api/eggs')]);
  }
  async function run(action: () => Promise<void>) {
    busy = true; error = ''; notice = '';
    try { await action(); } catch (reason) { showError(reason); } finally { busy = false; }
  }
  onMount(() => {
    void (async () => {
      try { session = await api<Session>('/api/session'); await loadData(); }
      catch (reason) { if (!(reason instanceof ApiError && reason.status === 401)) showError(reason); }
      finally { checking = false; }
    })();
  });
  function navigate(next: typeof page) { page = next; error = ''; notice = ''; selectedEgg = null; createOpen = false; }
  async function signIn(event: SubmitEvent) {
    event.preventDefault();
    await run(async () => {
      if (inviteToken) {
        await api('/api/register', { method: 'POST', body: JSON.stringify({ invite: inviteToken, username, password }) });
        inviteToken = ''; history.replaceState(null, '', location.pathname + location.search);
      }
      await api('/api/login', { method: 'POST', body: JSON.stringify({ username, password }) });
      password = '';
      session = await api<Session>('/api/session'); await loadData();
    });
  }
  async function signOut() {
    await run(async () => { await api('/api/logout', { method: 'POST' }); session = null; servers = []; eggs = []; inviteLink = ''; navigate('servers'); });
  }
  async function importEgg(event: SubmitEvent) {
    event.preventDefault();
    await run(async () => {
      if (!selectedFile) return;
      if (selectedFile.size > 1024 * 1024) throw new Error('Choose an egg JSON file smaller than 1 MiB.');
      await api('/api/eggs', { method: 'POST', body: await selectedFile.text() });
      eggs = await api<Egg[]>('/api/eggs'); notice = 'Egg imported into the shared library.'; selectedFile = undefined;
    });
  }
  async function invite() {
    await run(async () => {
      const result = await api<{ token: string }>('/api/invites', { method: 'POST' });
      const link = new URL(location.href); link.hash = new URLSearchParams({ invite: result.token }).toString();
      inviteLink = link.toString();
    });
  }
  async function openServer(server: Server) {
    if (!server.can_access) return;
    await run(async () => { selectedServer = await api(`/api/servers/${server.id}`); navigate('server'); });
  }
  function memory(bytes: number) { return `${(bytes / 1024 ** 3).toFixed(1)} GiB`; }
</script>

<svelte:head><meta name="referrer" content="no-referrer" /></svelte:head>

<div class="app-shell">
  <aside class="sidebar" aria-label="Workspace navigation">
    <a class="brand" href="/" aria-label="Game Manager home"><span class="brand-symbol"><Icon name="servers" size={20} /></span><span>Game Manager</span></a>
    <button class="new-server" disabled={!session} onclick={() => { navigate('servers'); createOpen = true; }}><Icon name="plus" /> Add server</button>
    <div class="nav-label">Workspace</div>
    <nav>
      <button class:active={page === 'servers' || page === 'server'} disabled={!session} onclick={() => navigate('servers')}><Icon name="servers" /> Servers</button>
      <button class:active={page === 'library'} disabled={!session} onclick={() => navigate('library')}><Icon name="library" /> Egg library</button>
      {#if session?.is_admin}<button class:active={page === 'team'} onclick={() => navigate('team')}><Icon name="users" /> Team</button>{/if}
    </nav>
    <div class="sidebar-note"><span class="quiet-dot"></span> Private workspace</div>
    <div class="sidebar-footer">
      <span class="avatar">{session?.is_admin ? 'A' : 'T'}</span>
      <div><strong>{session?.is_admin ? 'Administrator' : session ? 'Team member' : 'Your team'}</strong><small>{session ? 'Personal account' : 'Invite-only access'}</small></div>
      {#if session}<button class="icon-button" aria-label="Sign out" disabled={busy} onclick={signOut}><Icon name="logout" /></button>{/if}
    </div>
  </aside>

  <div class="workspace">
    <header class="topbar"><span class="breadcrumb">Workspace <Icon name="chevron" size={13} /> <span>{session ? pageTitle : 'Welcome'}</span></span><span class="private-label"><Icon name="lock" size={12} /> Private</span></header>
    <main id="main-content">
      {#if error}<div class="feedback error" role="alert">{error}</div>{/if}
      {#if notice}<div class="feedback success" role="status">{notice}</div>{/if}
      {#if checking}
        <div class="center-state" role="status">Opening your workspace…</div>
      {:else if !session}
        <section class="welcome">
          <div class="eyebrow">A place for your games</div>
          <h1>Your servers,<br /><span>together.</span></h1>
          <p>One quiet workspace for the worlds you share.</p>
          <form class="auth-form" onsubmit={signIn}>
            <h2>{inviteToken ? 'Join your team' : 'Sign in to your workspace'}</h2>
            {#if inviteToken}<p class="muted">Your invitation is ready. Choose your account details.</p>{/if}
            <label for="username">Username</label><input id="username" bind:value={username} autocomplete="username" required minlength="3" maxlength="32" />
            <label for="password">Password</label><input id="password" type="password" bind:value={password} autocomplete={inviteToken ? 'new-password' : 'current-password'} required minlength={inviteToken ? 12 : undefined} />
            <button class="primary full" disabled={busy}>{busy ? 'Please wait…' : inviteToken ? 'Create account' : 'Sign in'}<Icon name="arrow" size={16} /></button>
            <small>Accounts are available by invitation from your administrator.</small>
          </form>
        </section>
      {:else if page === 'servers'}
        <section class="page-content">
          <div class="page-heading"><div><div class="eyebrow">Your workspace</div><h1>Servers</h1><p>All your worlds. One place to manage them.</p></div><button class="primary" onclick={() => { createOpen = !createOpen; }}><Icon name="plus" size={16} /> Add server</button></div>
          {#if createOpen}<section class="panel setup-panel"><div><h2>Prepare your host first</h2><p>Server creation is waiting for safe resource limits and storage setup. Your existing servers will stay as they are.</p></div><button class="secondary" onclick={() => navigate('library')}>Browse eggs <Icon name="arrow" size={15} /></button><button class="icon-button" aria-label="Close server setup" onclick={() => { createOpen = false; }}><Icon name="close" /></button></section>{/if}
          <div class="list-toolbar"><label class="search-box"><Icon name="search" size={16} /><input aria-label="Search servers" placeholder="Search servers…" bind:value={query} /></label><span class="muted">{servers.length} {servers.length === 1 ? 'server' : 'servers'}</span></div>
          {#if !servers.length}
            <div class="empty-state"><div class="empty-icon"><Icon name="servers" size={25} /></div><h2>A home for your next world</h2><p>Your servers will appear here. Start by adding game definitions to the shared egg library.</p><button class="secondary" onclick={() => navigate('library')}>Open egg library <Icon name="arrow" size={15} /></button></div>
          {:else if !filteredServers.length}<div class="empty-state"><h2>No matching servers</h2><p>Try another name.</p></div>
          {:else}<div class="server-list">{#each filteredServers as server (server.id)}<button class="server-card" disabled={!server.can_access} onclick={() => openServer(server)}><span class="server-symbol"><Icon name="servers" size={21} /></span><span class="server-name"><strong>{server.name}</strong><small>{server.can_access ? 'Open server' : 'Access not granted'}</small></span><span class="server-usage">{#if server.usage}<span>{server.usage.cpu_percent.toFixed(1)}% CPU</span><span>{memory(server.usage.memory_bytes)} RAM</span><span>{memory(server.usage.disk_bytes)} disk</span>{:else}<span>Usage unavailable</span>{/if}</span>{#if server.can_access}<Icon name="chevron" size={16} />{:else}<Icon name="lock" size={16} />{/if}</button>{/each}</div>{/if}
        </section>
      {:else if page === 'library'}
        <section class="page-content">
          <div class="page-heading"><div><div class="eyebrow">Shared with your team</div><h1>Egg library</h1><p>Bring your games with you. Import a game definition to get started.</p></div><span class="count-badge">{eggs.length} {eggs.length === 1 ? 'egg' : 'eggs'}</span></div>
          <form class="panel import-panel" onsubmit={importEgg}><div><h2>Import an egg</h2><p>Choose a Pterodactyl egg JSON file. Imported definitions are shared with everyone on your team.</p><label class="file-picker" for="egg-file">Egg JSON file</label><input id="egg-file" type="file" accept=".json,application/json" onchange={(event) => { selectedFile = event.currentTarget.files?.[0]; }} /></div><button class="primary" disabled={!selectedFile || busy}>{busy ? 'Importing…' : 'Import egg'}</button></form>
          {#if !eggs.length}<div class="empty-state compact"><div class="empty-icon"><Icon name="library" size={25} /></div><h2>Your shared collection starts here</h2><p>Import your first egg using the form above.</p></div>{:else}<div class="egg-grid">{#each eggs as egg (egg.id)}<button class="egg-card" onclick={() => run(async () => { selectedEgg = await api<EggDetails>(`/api/eggs/revisions/${egg.revision}`); })}><span class="egg-card-icon"><Icon name="library" size={20} /></span><h2>{egg.name}</h2><p class="muted">Shared game definition</p><span class="tag">{egg.runtime_qualified ? 'Ready to install' : 'Needs runtime testing'}</span></button>{/each}</div>{/if}
          {#if selectedEgg}<section class="panel egg-details"><div class="detail-heading"><h2>{selectedEgg.name}</h2><button class="icon-button" aria-label="Close egg details" onclick={() => { selectedEgg = null; }}><Icon name="close" /></button></div><p>{selectedEgg.description}</p><dl><dt>Author</dt><dd>{selectedEgg.author}</dd><dt>Available environments</dt><dd>{selectedEgg.images.map(([name]) => name).join(', ')}</dd><dt>Settings</dt><dd>{selectedEgg.variables.map(variable => variable.name).join(', ')}</dd></dl></section>{/if}
        </section>
      {:else if page === 'team' && session.is_admin}
        <section class="page-content"><div class="page-heading"><div><div class="eyebrow">Better with friends</div><h1>Team</h1><p>Invite the people you want to build worlds with.</p></div></div><section class="panel invite-panel"><span class="empty-icon"><Icon name="users" size={24} /></span><div><h2>Invite a teammate</h2><p>Each invitation works once and expires after 24 hours. Your teammate chooses their own password.</p><button class="primary" disabled={busy} onclick={invite}><Icon name="plus" size={16} /> Invite teammate</button>{#if inviteLink}<label for="invite-link">Invitation link</label><input id="invite-link" readonly value={inviteLink} onclick={(event) => event.currentTarget.select()} /><small class="muted">Share this link privately with your teammate.</small>{/if}</div></section></section>
      {:else if page === 'server' && selectedServer}
        <section class="page-content"><div class="page-heading"><div><div class="eyebrow">Your server</div><h1>{selectedServer.name}</h1></div><button class="secondary" onclick={() => navigate('servers')}>Back to servers</button></div><section class="panel"><h2>Host setup required</h2><p>Server controls will become available after host setup is complete.</p></section></section>
      {/if}
    </main>
    <footer class="workspace-footer">Game Manager <span>Built for your team. Hosted by you.</span></footer>
  </div>
</div>
