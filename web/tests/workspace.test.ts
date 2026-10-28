import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import ServerWorkspace from '../src/pages/ServerWorkspace.svelte';

const GIB = 1024 ** 3;
const detail = (power: string, desired: boolean, extra: Record<string, unknown> = {}) => ({
  id: 's1', name: 'Bots', owner: 'u-owner', is_owner: true, runtime_error: null,
  details: { egg_name: 'Fabric', limits: { memory_bytes: 3 * GIB, disk_bytes: 4 * GIB, cpu_millis: 2000 }, binding: { ip: '127.0.0.1', port: 25601, transport: 'tcp' }, install_state: 'installed', install_message: null, deleting: false },
  runtime: { installed: true, operation: null, power, desired_running: desired, problem: null, started_at: null, exit_code: null, game: { version: '26.3', loader: 'fabric' }, disk_used_bytes: GIB, disk_total_bytes: 4 * GIB, ...extra },
});

function mock(state: { detail: unknown; console?: string[]; members?: unknown[]; schedules?: any[] }) {
  const calls: { path: string; method: string; body: any }[] = [];
  const fetchMock = vi.fn(async (path: string, options?: RequestInit) => {
    const method = options?.method ?? 'GET';
    calls.push({ path, method, body: options?.body ? JSON.parse(String(options.body)) : null });
    if (path === '/api/servers/s1' && method === 'GET') return Response.json(state.detail);
    if (path.startsWith('/api/servers/s1/console') && method === 'GET') return Response.json({ lines: state.console ?? [] });
    if (path === '/api/servers/s1/extensions' && method === 'GET') return Response.json({ game: { version: '26.3', loader: 'fabric' }, available: true, reason: null, managed: [{ project_id: 'carpet', title: 'Carpet', version_number: '26.3', filename: 'carpet.jar', dependency: false }], other_files: ['manual.jar'] });
    if (path.startsWith('/api/servers/s1/modrinth')) return Response.json([{ project_id: 'lithium', title: 'Lithium', description: 'Fast', downloads: 1 }]);
    if (path === '/api/servers/s1/extensions' && method === 'POST') return Response.json({ installing: [{ project_id: 'lithium', title: 'Lithium', version_number: '1.0', filename: 'l.jar', dependency: false }], removing: [] }, { status: 202 });
    if (path === '/api/servers/s1/settings' && method === 'GET') return Response.json({ image: 'yolks:java_25', images: [['Java 25', 'yolks:java_25'], ['Java 21', 'yolks:java_21']], startup: 'java -jar {{SERVER_JARFILE}}', variables: [{ env_variable: 'SERVER_JARFILE', name: 'Server jar', description: 'Jar name', value: 'server.jar', editable: false }, { env_variable: 'MC_VERSION', name: 'Minecraft version', description: '', value: 'latest', editable: true }] });
    if (path === '/api/servers/s1/settings' && method === 'PUT') return Response.json({ changed: ['image', 'MC_VERSION'] });
    if (path === '/api/servers/s1/limits' && method === 'PUT') return Response.json(JSON.parse(String(options!.body)));
    if (path === '/api/servers/s1/members') return Response.json(state.members ?? []);
    if (path === '/api/servers/s1/schedules' && method === 'GET') return Response.json(state.schedules ?? []);
    if (path === '/api/servers/s1/schedules' && method === 'POST') { const body = JSON.parse(String(options!.body)); state.schedules = [...(state.schedules ?? []), { ...body, id: 'sch-1', next_run_at: 1793170000, last_run_at: null, last_outcome: null }]; return Response.json(state.schedules!.at(-1), { status: 201 }); }
    if (path === '/api/users') return Response.json([{ id: 'u-owner', username: 'owner' }, { id: 'u-ana', username: 'ana' }, { id: 'u-bo', username: 'bo' }]);
    if (method !== 'GET') return new Response(null, { status: path.endsWith('/console') ? 204 : 202 });
    throw new Error(`Unexpected request: ${method} ${path}`);
  });
  vi.stubGlobal('fetch', fetchMock);
  return calls;
}

test('a running server shows status and console, sends commands and offers only valid power actions', async () => {
  const calls = mock({ detail: detail('running', true), console: ['[Server] Done (1.2s)! For help, type "help"', '[Server] Bot_Alpha joined the game'] });
  render(ServerWorkspace, { serverId: 's1', onBack: () => {}, onDeleted: () => {} });
  expect((await screen.findByRole('status', { name: '' }).catch(() => screen.findByText('Running'))).textContent).toBeTruthy();
  await screen.findByText('[Server] Bot_Alpha joined the game');
  expect(screen.getByText(/Minecraft 26.3 with fabric/)).toBeTruthy();
  const controls = screen.getByLabelText('Power controls');
  expect((within(controls).getByRole('button', { name: /Start/ }) as HTMLButtonElement).disabled).toBe(true);
  expect((within(controls).getByRole('button', { name: /Stop/ }) as HTMLButtonElement).disabled).toBe(false);
  const user = userEvent.setup();
  // Commands typed back to back are all sent, in order, without waiting for each other.
  await user.type(screen.getByLabelText('Console command'), 'player Bot_Beta spawn{Enter}');
  expect((screen.getByLabelText('Console command') as HTMLInputElement).value).toBe('');
  await user.type(screen.getByLabelText('Console command'), 'player Bot_Gamma spawn{Enter}');
  await user.type(screen.getByLabelText('Console command'), 'list{Enter}');
  await waitFor(() => expect(calls.filter(c => c.method === 'POST' && c.path === '/api/servers/s1/console').map(c => c.body.command)).toEqual(['player Bot_Beta spawn', 'player Bot_Gamma spawn', 'list']));
  await user.click(within(controls).getByRole('button', { name: /Restart/ }));
  await waitFor(() => expect(calls.some(c => c.path === '/api/servers/s1/power' && c.body.action === 'restart')).toBe(true));
});

test('mods cannot be changed while the server runs and install when it is stopped', async () => {
  const state = { detail: detail('running', true) };
  const calls = mock(state);
  render(ServerWorkspace, { serverId: 's1', onBack: () => {}, onDeleted: () => {} });
  const user = userEvent.setup();
  await user.click(await screen.findByRole('tab', { name: /Mods/ }));
  await screen.findByText(/Stop the server before adding or removing mods/);
  expect(screen.getByText(/compatibility unknown/)).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Remove Carpet' }) as HTMLButtonElement).disabled).toBe(true);
  state.detail = detail('offline', false);
  await waitFor(() => expect(screen.queryByText(/Stop the server before adding or removing mods/)).toBeNull(), { timeout: 4000 });
  await user.type(screen.getByLabelText('Search Modrinth'), 'lithium{Enter}');
  await user.click(await screen.findByRole('button', { name: 'Install' }));
  await screen.findByText(/Installing Lithium 1.0/);
  expect(calls.some(c => c.method === 'POST' && c.path === '/api/servers/s1/extensions' && c.body.project_id === 'lithium')).toBe(true);
});

test('deletion requires the exact name and losing access stops polling', async () => {
  const calls = mock({ detail: detail('offline', false) });
  render(ServerWorkspace, { serverId: 's1', onBack: () => {}, onDeleted: () => {} });
  const user = userEvent.setup();
  await user.click(await screen.findByRole('tab', { name: /Delete/ }));
  const button = screen.getByRole('button', { name: 'Delete server' }) as HTMLButtonElement;
  await user.type(screen.getByLabelText(/to confirm/), 'bots');
  expect(button.disabled).toBe(true);
  await user.clear(screen.getByLabelText(/to confirm/));
  await user.type(screen.getByLabelText(/to confirm/), 'Bots');
  await user.click(button);
  await waitFor(() => expect(calls.some(c => c.method === 'DELETE' && c.body.confirm_name === 'Bots')).toBe(true));

  vi.stubGlobal('fetch', vi.fn(async () => Response.json({ code: 'forbidden', message: 'You do not have access to this server or action' }, { status: 403 })));
  await screen.findByText(/You do not have access/, {}, { timeout: 4000 });
  const fetchAfter = vi.mocked(fetch).mock.calls.length;
  await new Promise(resolve => setTimeout(resolve, 2500));
  expect(vi.mocked(fetch).mock.calls.length).toBe(fetchAfter);
});

test('backups can be created, restored only when stopped, and kept when deleting', async () => {
  const backup = { id: 'b1', kind: 'manual', created_at: 1793140000, size_bytes: 5 * 1024 ** 2 };
  const calls = mock({ detail: detail('running', true) });
  const original = vi.mocked(fetch).getMockImplementation()!;
  vi.stubGlobal('fetch', vi.fn(async (path: string, options?: RequestInit) => {
    if (path === '/api/servers/s1/backups' && (options?.method ?? 'GET') === 'GET') return Response.json({ retention: 3, backups: [backup, { ...backup, id: 'b2' }] });
    return original(path, options);
  }));
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  render(ServerWorkspace, { serverId: 's1', onBack: () => {}, onDeleted: () => {} });
  const user = userEvent.setup();
  await user.click(await screen.findByRole('tab', { name: /Backups/ }));
  expect((await screen.findAllByRole('link', { name: 'Download' }))[0].getAttribute('href')).toBe('/api/servers/s1/backups/b1/download');
  expect((screen.getAllByRole('button', { name: /^Restore backup/ })[0] as HTMLButtonElement).disabled).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Create backup' }));
  await screen.findByText(/server stops while the archive is written/);
  await user.selectOptions(screen.getByLabelText(/Keep/), '1');
  await waitFor(() => expect(calls.some(c => c.path.endsWith('/backups/retention') && c.body.retention === 1 && c.body.remove_oldest === true)).toBe(true));
  await user.click(screen.getByRole('tab', { name: /Delete/ }));
  await user.type(screen.getByLabelText(/to confirm/), 'Bots');
  await user.click(screen.getByLabelText(/Keep this server's backups/));
  await user.click(screen.getByRole('button', { name: 'Delete server' }));
  await waitFor(() => expect(calls.some(c => c.method === 'DELETE' && c.body.keep_backups === true)).toBe(true));
});

test('files can be browsed, edited with conflict detection, and uploaded', async () => {
  const calls = mock({ detail: detail('running', true) });
  const original = vi.mocked(fetch).getMockImplementation()!;
  vi.stubGlobal('fetch', vi.fn(async (path: string, options?: RequestInit) => {
    const method = options?.method ?? 'GET';
    if (path === '/api/servers/s1/files?path=') return Response.json([{ name: 'config', kind: 'directory', size: 0, modified: 0 }, { name: 'server.properties', kind: 'file', size: 20, modified: 0 }, { name: 'link', kind: 'link', size: 0, modified: 0 }]);
    if (path === '/api/servers/s1/files?path=config') return Response.json([]);
    if (path.startsWith('/api/servers/s1/files/text?path=server.properties')) return Response.json({ text: 'motd=hi\n', version: 'v1' });
    if (path === '/api/servers/s1/files/text' && method === 'PUT') {
      calls.push({ path, method, body: JSON.parse(String(options!.body)) });
      return Response.json({ code: 'conflict', message: 'the file changed since it was opened; reload it before saving' }, { status: 409 });
    }
    if (path.startsWith('/api/servers/s1/files/upload')) { calls.push({ path, method, body: null }); return new Response(null, { status: 201 }); }
    return original(path, options);
  }));
  render(ServerWorkspace, { serverId: 's1', onBack: () => {}, onDeleted: () => {} });
  const user = userEvent.setup();
  await user.click(await screen.findByRole('tab', { name: /Files/ }));
  expect((await screen.findByRole('button', { name: 'link (link, not followed)' }) as HTMLButtonElement).disabled).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Open server.properties' }));
  const editor = await screen.findByLabelText('Contents of server.properties');
  await user.clear(editor);
  await user.type(editor, 'motd=new');
  await user.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText(/changed since it was opened/);
  expect(calls.find(c => c.method === 'PUT')!.body).toEqual({ path: 'server.properties', text: 'motd=new', version: 'v1' });
  await user.click(screen.getByRole('button', { name: 'Close' }));
  await user.click(await screen.findByRole('button', { name: 'Open folder config' }));
  await screen.findByText('This folder is empty.');
  await user.upload(screen.getByLabelText('Upload'), new File(['data'], 'notes.txt'));
  await waitFor(() => expect(calls.some(c => c.path === '/api/servers/s1/files/upload?path=config%2Fnotes.txt')).toBe(true));
});

test('startup settings and limits are saved and explain when they apply', async () => {
  const calls = mock({ detail: detail('running', true, { usage: { cpu_millis: 1250, memory_bytes: 2 * GIB, memory_limit_bytes: 3 * GIB } }) });
  render(ServerWorkspace, { serverId: 's1', onBack: () => {}, onDeleted: () => {} });
  const user = userEvent.setup();
  await screen.findByText('2.0 GiB of 3.0 GiB');
  expect(screen.getByText('125% of 2 cores')).toBeTruthy();
  await user.click(screen.getByRole('tab', { name: /Settings/ }));
  const jar = (await screen.findByLabelText('Server jar')) as HTMLInputElement;
  expect(jar.readOnly).toBe(true);
  await user.selectOptions(screen.getByLabelText('Runtime image'), 'yolks:java_21');
  const version = screen.getByLabelText('Minecraft version');
  await user.clear(version);
  await user.type(version, '1.21.10');
  await user.click(screen.getByRole('button', { name: 'Save startup settings' }));
  await screen.findByText(/Saved 2 changes. They apply after the next restart./);
  expect(calls.find(c => c.method === 'PUT' && c.path === '/api/servers/s1/settings')!.body).toEqual({ image: 'yolks:java_21', variables: { MC_VERSION: '1.21.10' } });
  const memory = screen.getByLabelText('Memory (GiB)');
  await user.clear(memory);
  await user.type(memory, '4.5');
  await user.click(screen.getByRole('button', { name: 'Save limits' }));
  await screen.findByText(/Limits saved/);
  expect(calls.find(c => c.method === 'PUT' && c.path === '/api/servers/s1/limits')!.body).toEqual({ memory_bytes: 4.5 * GIB, disk_bytes: 4 * GIB, cpu_millis: 2000 });
});

test('members can be given and removed access; the owner is not offered', async () => {
  const state = { detail: detail('offline', false), members: [{ user_id: 'u-bo', username: 'bo', enabled: true }] };
  const calls = mock(state);
  vi.stubGlobal('confirm', () => true);
  render(ServerWorkspace, { serverId: 's1', onBack: () => {}, onDeleted: () => {} });
  const user = userEvent.setup();
  await user.click(await screen.findByRole('tab', { name: /Members/ }));
  await screen.findByText('Owner');
  const select = screen.getByLabelText('Teammate') as HTMLSelectElement;
  expect(Array.from(select.options).map(o => o.textContent)).toEqual(['Choose a teammate…', 'ana']);
  await user.selectOptions(select, 'u-ana');
  await user.click(screen.getByRole('button', { name: 'Give access' }));
  await screen.findByText('ana can now manage this server.');
  await user.click(screen.getByRole('button', { name: 'Remove bo' }));
  await waitFor(() => expect(calls.filter(c => c.method === 'PUT' && c.path.startsWith('/api/servers/s1/members/')).map(c => [c.path, c.body.enabled])).toEqual([
    ['/api/servers/s1/members/u-ana', true], ['/api/servers/s1/members/u-bo', false],
  ]));
});

test('a scheduled console command is created with its days, time and zone', async () => {
  const state: { detail: unknown; schedules: any[] } = { detail: detail('running', true), schedules: [] };
  const calls = mock(state);
  render(ServerWorkspace, { serverId: 's1', onBack: () => {}, onDeleted: () => {} });
  const user = userEvent.setup();
  await user.click(await screen.findByRole('tab', { name: /Schedules/ }));
  await screen.findByText('No schedules yet.');
  await user.click(screen.getByRole('button', { name: 'New schedule' }));
  await user.selectOptions(screen.getByLabelText('Action'), 'command');
  await user.type(screen.getByLabelText('Console command'), 'say Restarting in 5 minutes');
  const time = screen.getByLabelText('Time') as HTMLInputElement;
  time.value = '03:55';
  time.dispatchEvent(new Event('change', { bubbles: true }));
  for (const day of ['Sat', 'Sun']) await user.click(screen.getByLabelText(day));
  const zone = screen.getByLabelText('Time zone');
  await user.clear(zone);
  await user.type(zone, 'Europe/Amsterdam');
  await user.click(screen.getByRole('button', { name: 'Save schedule' }));
  await screen.findByText(/Run command: say Restarting in 5 minutes/);
  expect(screen.getByText(/Weekdays at 03:55 \(Europe\/Amsterdam\)/)).toBeTruthy();
  expect(calls.find(c => c.method === 'POST' && c.path === '/api/servers/s1/schedules')!.body).toEqual({
    action: 'command', command: 'say Restarting in 5 minutes', hour: 3, minute: 55, weekdays: 31, timezone: 'Europe/Amsterdam', enabled: true,
  });
});
