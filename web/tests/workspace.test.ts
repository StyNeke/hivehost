import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import ServerWorkspace from '../src/pages/ServerWorkspace.svelte';

const GIB = 1024 ** 3;
const detail = (power: string, desired: boolean, extra: Record<string, unknown> = {}) => ({
  id: 's1', name: 'Bots', is_owner: true, runtime_error: null,
  details: { egg_name: 'Fabric', limits: { memory_bytes: 3 * GIB, disk_bytes: 4 * GIB, cpu_millis: 2000 }, binding: { ip: '127.0.0.1', port: 25601, transport: 'tcp' }, install_state: 'installed', install_message: null, deleting: false },
  runtime: { installed: true, operation: null, power, desired_running: desired, problem: null, started_at: null, exit_code: null, game: { version: '26.3', loader: 'fabric' }, disk_used_bytes: GIB, disk_total_bytes: 4 * GIB, ...extra },
});

function mock(state: { detail: unknown; console?: string[] }) {
  const calls: { path: string; method: string; body: any }[] = [];
  const fetchMock = vi.fn(async (path: string, options?: RequestInit) => {
    const method = options?.method ?? 'GET';
    calls.push({ path, method, body: options?.body ? JSON.parse(String(options.body)) : null });
    if (path === '/api/servers/s1' && method === 'GET') return Response.json(state.detail);
    if (path.startsWith('/api/servers/s1/console') && method === 'GET') return Response.json({ lines: state.console ?? [] });
    if (path === '/api/servers/s1/extensions' && method === 'GET') return Response.json({ game: { version: '26.3', loader: 'fabric' }, available: true, reason: null, managed: [{ project_id: 'carpet', title: 'Carpet', version_number: '26.3', filename: 'carpet.jar', dependency: false }], other_files: ['manual.jar'] });
    if (path.startsWith('/api/servers/s1/modrinth')) return Response.json([{ project_id: 'lithium', title: 'Lithium', description: 'Fast', downloads: 1 }]);
    if (path === '/api/servers/s1/extensions' && method === 'POST') return Response.json({ installing: [{ project_id: 'lithium', title: 'Lithium', version_number: '1.0', filename: 'l.jar', dependency: false }], removing: [] }, { status: 202 });
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
