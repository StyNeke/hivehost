<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../api';
  import Icon from '../components/Icon.svelte';

  type Resources = { memory_bytes: number; disk_bytes: number; cpu_millis: number };
  type Host = { ready: boolean; blockers: string[]; budget: Resources; reserved: Resources; remaining: Resources };
  type Allocation = { id: string; ip: string; port: number; transport: 'tcp' | 'udp' };
  type Variable = { name: string; env_variable: string; default_value: string; description: string; user_viewable: boolean; user_editable: boolean };
  type EggDetails = { name: string; startup?: string; images: [string, string][]; variables: Variable[]; features: string[] };
  type Egg = { id: string; name: string; revision: string; runtime_qualified: boolean };

  let { eggs, isAdmin, onCreated, onClose }: { eggs: Egg[]; isAdmin: boolean; onCreated: (message: string, id: string) => Promise<void>; onClose: () => void } = $props();

  const GIB = 1024 ** 3;
  let host = $state<Host | null>(null);
  let hostError = $state('');
  let allocations = $state<Allocation[]>([]);
  let loading = $state(true);
  let busy = $state(false);
  let formError = $state('');
  let name = $state('');
  let revision = $state('');
  let details = $state<EggDetails | null>(null);
  let image = $state('');
  let allocation = $state('');
  let memoryGib = $state(2);
  let diskGib = $state(10);
  let cpuCores = $state(1);
  let values = $state<Record<string, string>>({});
  let licenseAccepted = $state(false);
  let newIp = $state('');
  let newPort = $state(25565);
  let newTransport = $state<'tcp' | 'udp'>('tcp');

  const editable = $derived(details?.variables.filter(variable => variable.user_viewable && variable.user_editable) ?? []);
  // Bedrock clients connect over UDP; Java over TCP. A mismatched address cannot be joined.
  const wantsUdp = $derived(!!details && /bedrock/i.test(`${details.name} ${details.startup ?? ''}`));
  const chosen = $derived(allocations.find(item => item.id === allocation));
  const transportMismatch = $derived(!!details && !!chosen && (chosen.transport === 'udp') !== wantsUdp);
  const needsLicense = $derived(details?.features.includes('eula') ?? false);
  const canSubmit = $derived(!!host?.ready && !!name.trim() && !!details && !!image && !!allocation && (!needsLicense || licenseAccepted) && !busy);

  function gib(bytes: number) { return `${(bytes / GIB).toFixed(1)} GiB`; }
  function cores(millis: number) { return `${(millis / 1000).toFixed(2).replace(/\.?0+$/, '')} cores`; }
  function message(reason: unknown) { return reason instanceof Error ? reason.message : 'Something went wrong. Please try again.'; }

  async function loadHost() {
    hostError = '';
    try { host = await api<Host>('/api/host'); } catch (reason) { host = null; hostError = message(reason); }
    allocations = await api<Allocation[]>('/api/allocations');
    if (!allocations.some(item => item.id === allocation)) allocation = allocations[0]?.id ?? '';
  }
  onMount(() => { void loadHost().catch(reason => { hostError = message(reason); }).finally(() => { loading = false; }); });

  async function chooseEgg() {
    details = null; image = ''; values = {}; licenseAccepted = false;
    if (!revision) return;
    try {
      const loaded = await api<EggDetails>(`/api/eggs/revisions/${revision}`);
      details = loaded;
      image = loaded.images[0]?.[1] ?? '';
      values = Object.fromEntries(loaded.variables.filter(v => v.user_viewable && v.user_editable).map(v => [v.env_variable, v.default_value]));
    } catch (reason) { formError = message(reason); }
  }

  async function create(event: SubmitEvent) {
    event.preventDefault();
    if (!canSubmit) return;
    busy = true; formError = '';
    try {
      const created = await api<{ id: string }>('/api/servers', { method: 'POST', body: JSON.stringify({
        name: name.trim(), egg_revision: revision, image, allocation_id: allocation,
        limits: { memory_bytes: Math.round(memoryGib * GIB), disk_bytes: Math.round(diskGib * GIB), cpu_millis: Math.round(cpuCores * 1000) },
        variables: values, license_accepted: licenseAccepted,
      }) });
      await onCreated('Server created and its resources reserved. Installation has started.', created.id);
    } catch (reason) { formError = message(reason); await loadHost().catch(() => {}); }
    finally { busy = false; }
  }

  async function addAllocation(event: SubmitEvent) {
    event.preventDefault();
    busy = true; formError = '';
    try {
      await api('/api/allocations', { method: 'POST', body: JSON.stringify({ ip: newIp.trim(), port: newPort, transport: newTransport }) });
      await loadHost();
    } catch (reason) { formError = message(reason); }
    finally { busy = false; }
  }
</script>

<section class="panel create-panel" aria-labelledby="create-title">
  <div class="detail-heading"><h2 id="create-title">New server</h2><button class="icon-button" aria-label="Close server setup" onclick={onClose}><Icon name="close" /></button></div>
  {#if loading}
    <p role="status">Checking the host…</p>
  {:else if hostError}
    <div class="feedback error inline" role="alert">{hostError}</div>
    <p>Servers can only be created from a live, trusted host status. Your existing servers are not affected.</p>
  {:else if host}
    <div class="capacity" aria-label="Safe host capacity">
      <div><span>RAM available</span><strong>{gib(host.remaining.memory_bytes)} of {gib(host.budget.memory_bytes)}</strong></div>
      <div><span>CPU available</span><strong>{cores(host.remaining.cpu_millis)} of {cores(host.budget.cpu_millis)}</strong></div>
      <div><span>Disk available</span><strong>{gib(host.remaining.disk_bytes)} of {gib(host.budget.disk_bytes)}</strong></div>
    </div>
    <p class="muted">The budget already subtracts host overhead, existing containers and every managed server, running or stopped.</p>
    {#if formError}<div class="feedback error inline" role="alert">{formError}</div>{/if}
    {#if !host.ready}
      <div class="blockers">
        <h3>The host isn't ready for new servers</h3>
        <ul>{#each host.blockers as blocker}<li>{blocker}</li>{/each}</ul>
      </div>
    {:else}
      <form class="create-form" onsubmit={create}>
        <label for="server-name">Server name</label>
        <input id="server-name" bind:value={name} maxlength="80" required />
        <label for="server-egg">Game</label>
        <select id="server-egg" bind:value={revision} onchange={chooseEgg} required>
          <option value="">Choose an egg…</option>
          {#each eggs as egg (egg.id)}<option value={egg.revision} disabled={!egg.runtime_qualified && !isAdmin}>{egg.name}{egg.runtime_qualified ? '' : isAdmin ? ' (test install, not yet qualified)' : ' (not yet qualified)'}</option>{/each}
        </select>
        {#if details}
          <label for="server-image">Environment</label>
          <select id="server-image" bind:value={image}>{#each details.images as [label, value]}<option {value}>{label}</option>{/each}</select>
          {#each editable as variable (variable.env_variable)}
            <label for={`var-${variable.env_variable}`}>{variable.name}</label>
            <input id={`var-${variable.env_variable}`} bind:value={values[variable.env_variable]} />
            {#if variable.description}<small class="muted">{variable.description}</small>{/if}
          {/each}
        {/if}
        <label for="server-allocation">Address</label>
        {#if allocations.length}
          <select id="server-allocation" bind:value={allocation}>{#each allocations as item (item.id)}<option value={item.id}>{item.ip}:{item.port} ({item.transport.toUpperCase()})</option>{/each}</select>
          {#if transportMismatch}<p class="feedback warning inline">{wantsUdp ? 'Bedrock players connect over UDP; choose a UDP address.' : 'Java players connect over TCP; choose a TCP address.'}</p>{/if}
        {:else}
          <p class="muted">No free addresses. {isAdmin ? 'Add one below.' : 'Ask your administrator to add one.'}</p>
        {/if}
        <div class="limits">
          <div><label for="server-ram">RAM (GiB)</label><input id="server-ram" type="number" min="0.5" step="0.5" bind:value={memoryGib} required /></div>
          <div><label for="server-disk">Disk (GiB)</label><input id="server-disk" type="number" min="1" step="1" bind:value={diskGib} required /></div>
          <div><label for="server-cpu">CPU (cores)</label><input id="server-cpu" type="number" min="0.25" step="0.25" bind:value={cpuCores} required /></div>
        </div>
        {#if needsLicense}
          <label class="checkbox"><input type="checkbox" bind:checked={licenseAccepted} /> I accept the game's license (for Minecraft, the Minecraft EULA)</label>
        {/if}
        <button class="primary" disabled={!canSubmit}>{busy ? 'Creating…' : 'Create server'}</button>
      </form>
    {/if}
    {#if isAdmin}
      <form class="allocation-form" onsubmit={addAllocation}>
        <h3>Add an address</h3>
        <p class="muted">Only private IPs present on the host and ports not used by other services are accepted.</p>
        <div class="limits">
          <div><label for="alloc-ip">IP</label><input id="alloc-ip" bind:value={newIp} required placeholder="100.116.183.201" /></div>
          <div><label for="alloc-port">Port</label><input id="alloc-port" type="number" min="1" max="65535" bind:value={newPort} required /></div>
          <div><label for="alloc-transport">Protocol</label><select id="alloc-transport" bind:value={newTransport}><option value="tcp">TCP</option><option value="udp">UDP</option></select></div>
        </div>
        <button class="secondary" disabled={busy}>Add address</button>
      </form>
    {/if}
  {/if}
</section>
