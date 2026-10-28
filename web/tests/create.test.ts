import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import App from '../src/App.svelte';

const GIB = 1024 ** 3;
const resources = (memory: number, disk: number, cpu: number) => ({ memory_bytes: memory * GIB, disk_bytes: disk * GIB, cpu_millis: cpu });
const host = (ready: boolean, blockers: string[] = []) => ({
  measured_at: 1, ready, blockers,
  hardware: { cpu_millis: 6000, memory_bytes: 24 * GIB, disk_total_bytes: 100 * GIB, disk_available_bytes: 70 * GIB },
  budget: resources(16, 40, 4000), reserved: resources(4, 0, 1000), remaining: resources(12, 40, 3000),
});
const egg = {
  name: 'Paper', description: 'Paper server', author: 'upstream', features: ['eula'],
  images: [['Java 21', 'ghcr.io/ptero-eggs/yolks:java_21']],
  variables: [
    { name: 'Minecraft Version', env_variable: 'MINECRAFT_VERSION', default_value: 'latest', user_viewable: true, user_editable: true, description: 'Version to install' },
    { name: 'Hidden', env_variable: 'HIDDEN', default_value: 'x', user_viewable: false, user_editable: false, description: '' },
  ],
};

function mockApi(hostResponse: () => Response, onCreate?: (body: any) => Response, details: Record<string, unknown> = egg) {
  const fetchMock = vi.fn(async (path: string, options?: RequestInit) => {
    if (path === '/api/session') return Response.json({ id: 'member', is_admin: false });
    if (path === '/api/servers' && options?.method === 'POST') return onCreate!(JSON.parse(String(options.body)));
    if (path === '/api/servers') return Response.json([]);
    if (path === '/api/eggs') return Response.json([{ id: 'egg', name: 'Paper', revision: 'rev-1', runtime_qualified: true }]);
    if (path === '/api/host') return hostResponse();
    if (path === '/api/allocations') return Response.json([{ id: 'alloc-1', ip: '100.116.183.201', port: 25600, transport: 'tcp' }]);
    if (path === '/api/eggs/revisions/rev-1') return Response.json(details);
    throw new Error(`Unexpected request: ${path}`);
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

test('an unready host explains its blockers and remaining capacity instead of offering creation', async () => {
  mockApi(() => Response.json(host(false, ['Storage quota setup has not been qualified'])));
  render(App);
  const user = userEvent.setup();
  await user.click((await screen.findAllByRole('button', { name: /Add server/ }))[0]);
  const panel = await screen.findByRole('region', { name: 'New server' });
  await within(panel).findByText('Storage quota setup has not been qualified');
  expect(within(panel).getByText(/12\.0 GiB of 16\.0 GiB/)).toBeTruthy();
  expect(within(panel).queryByRole('button', { name: 'Create server' })).toBeNull();
});

test('an unreachable agent is reported, not hidden', async () => {
  mockApi(() => Response.json({ code: 'host_unavailable', message: 'The host agent is not configured' }, { status: 503 }));
  render(App);
  const user = userEvent.setup();
  await user.click((await screen.findAllByRole('button', { name: /Add server/ }))[0]);
  const panel = await screen.findByRole('region', { name: 'New server' });
  await within(panel).findByText('The host agent is not configured');
  expect(within(panel).queryByRole('button', { name: 'Create server' })).toBeNull();
});

test('a ready host creates a server with the chosen egg, allocation and limits', async () => {
  let created: any = null;
  const fetchMock = mockApi(() => Response.json(host(true)), (body) => {
    created = body;
    return Response.json({ id: 'server-1', job_id: 'job-1' }, { status: 201 });
  });
  render(App);
  const user = userEvent.setup();
  await user.click((await screen.findAllByRole('button', { name: /Add server/ }))[0]);
  const panel = await screen.findByRole('region', { name: 'New server' });
  await user.type(await within(panel).findByLabelText('Server name'), 'Survival');
  await user.selectOptions(within(panel).getByLabelText('Game'), 'rev-1');
  const version = await within(panel).findByLabelText('Minecraft Version');
  expect(within(panel).queryByLabelText('Hidden')).toBeNull();
  await user.clear(version);
  await user.type(version, '1.21.4');
  await user.clear(within(panel).getByLabelText(/RAM/));
  await user.type(within(panel).getByLabelText(/RAM/), '4');
  await user.clear(within(panel).getByLabelText(/Disk/));
  await user.type(within(panel).getByLabelText(/Disk/), '10');
  await user.clear(within(panel).getByLabelText(/CPU/));
  await user.type(within(panel).getByLabelText(/CPU/), '1.5');
  const submit = within(panel).getByRole('button', { name: 'Create server' }) as HTMLButtonElement;
  expect(submit.disabled).toBe(true);
  await user.click(within(panel).getByLabelText(/accept the game's license/));
  expect(submit.disabled).toBe(false);
  await user.click(submit);
  await waitFor(() => expect(created).not.toBeNull());
  expect(created).toEqual({
    name: 'Survival', egg_revision: 'rev-1', image: 'ghcr.io/ptero-eggs/yolks:java_21', allocation_id: 'alloc-1',
    limits: { memory_bytes: 4 * GIB, disk_bytes: 10 * GIB, cpu_millis: 1500 },
    variables: { MINECRAFT_VERSION: '1.21.4' }, license_accepted: true,
  });
  await screen.findByText(/Installation has started/i);
  expect(fetchMock.mock.calls.filter(([path, options]) => path === '/api/servers' && !options?.method).length).toBeGreaterThan(1);
});

test('a Bedrock egg on a TCP address is flagged before creation', async () => {
  mockApi(() => Response.json(host(true)), undefined, { ...egg, name: 'Vanilla Bedrock', startup: './bedrock_server', features: [] });
  render(App);
  const user = userEvent.setup();
  await user.click((await screen.findAllByRole('button', { name: /Add server/ }))[0]);
  const panel = await screen.findByRole('region', { name: 'New server' });
  await user.selectOptions(await within(panel).findByLabelText('Game'), 'rev-1');
  await within(panel).findByText(/Bedrock players connect over UDP/);
});
