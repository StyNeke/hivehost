import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import App from '../src/App.svelte';

test('sign in loads real API data and restricted cards stay inaccessible', async () => {
  let signedIn = false;
  const fetchMock = vi.fn(async (path: string, options?: RequestInit) => {
    if (path === '/api/session') return new Response(signedIn ? JSON.stringify({ id: 'member', is_admin: false }) : '{}', { status: signedIn ? 200 : 401 });
    if (path === '/api/login') {
      expect(JSON.parse(String(options?.body))).toEqual({ username: 'friend', password: 'a long test password' });
      signedIn = true;
      return new Response(null, { status: 200 });
    }
    if (path === '/api/servers') return Response.json([{ id: 'server', name: 'Private World', can_access: false, usage: null }]);
    if (path === '/api/eggs') return Response.json([]);
    throw new Error(`Unexpected request: ${path}`);
  });
  vi.stubGlobal('fetch', fetchMock);
  render(App);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText('Username'), 'friend');
  await user.type(screen.getByLabelText('Password'), 'a long test password');
  await user.click(screen.getByRole('button', { name: 'Sign in' }));
  const card = await screen.findByRole('button', { name: /Private World/ });
  expect((card as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: 'Invite teammate' })).toBeNull();
  expect(fetchMock.mock.calls.some(([path]) => String(path).includes('/api/servers/server'))).toBe(false);
});

test('shared library import errors show actionable compatibility details', async () => {
  vi.stubGlobal('fetch', vi.fn(async (path: string) => {
    if (path === '/api/session') return Response.json({ id: 'admin', is_admin: true });
    if (path === '/api/servers') return Response.json([]);
    if (path === '/api/eggs') return Response.json([]);
    if (path === '/api/experimental') return Response.json([]);
    throw new Error(`Unexpected request: ${path}`);
  }));
  render(App);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Egg library' }));
  await user.upload(screen.getByLabelText('Egg JSON file'), new File(['invalid'], 'egg.json', { type: 'application/json' }));
  vi.stubGlobal('fetch', vi.fn(async () => Response.json({ code: 'unsupported_egg', message: 'Egg could not be imported', details: [{ path: 'variables[0].rules', message: 'unsupported validation rule' }] }, { status: 422 })));
  await user.click(screen.getByRole('button', { name: 'Import egg' }));
  await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('variables[0].rules'));
});
