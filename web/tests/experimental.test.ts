import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import Experimental from '../src/components/Experimental.svelte';

const entry = (key: string, name: string, extra: Record<string, unknown>) => ({
  key, name, repository: `owner/${name}`, website: 'https://example.test', summary: `${name} summary`, upstream: null, fetched_at: null, stale: true, error: null, egg_id: null, ...extra,
});
const upstream = {
  description: 'Rust server <b>fast</b>', url: 'https://github.com/owner/Pumpkin', pushed_at: '2026-10-27T10:00:00Z',
  releases: [{ tag: 'nightly', name: 'Nightly', published_at: '2026-10-27T09:00:00Z', prerelease: true, url: 'https://github.com/r', assets: [] }],
  commits: [{ sha: 'abc', message: 'Add chunk saving', date: '2026-10-27T08:00:00Z', url: 'https://github.com/c' }],
  milestones: [{ title: '1.0', open_issues: 7, closed_issues: 21, due_on: null, url: 'https://github.com/m' }],
};

test('upstream status is shown with its source and age, never as a completeness score', async () => {
  const calls: string[] = [];
  vi.stubGlobal('fetch', vi.fn(async (path: string, options?: RequestInit) => {
    calls.push(`${options?.method ?? 'GET'} ${path}`);
    if (path === '/api/experimental') return Response.json([
      entry('pumpkin', 'Pumpkin', { upstream, fetched_at: 1793160000, stale: true, error: 'GitHub rate limit reached' }),
      entry('steelmc', 'SteelMC', {}),
    ]);
    if (path === '/api/experimental/refresh') return Response.json({ code: 'refresh_limited', message: 'Upstream status was refreshed less than a minute ago' }, { status: 429 });
    if (path === '/api/experimental/steelmc/egg') return Response.json({ id: 'egg-2', name: 'SteelMC (experimental)', experimental: true }, { status: 201 });
    throw new Error(`Unexpected request: ${path}`);
  }));
  const onAdded = vi.fn();
  render(Experimental, { onAdded });
  const pumpkin = await screen.findByRole('article', { name: 'Pumpkin' });
  expect(within(pumpkin).getByText('Rust server <b>fast</b>')).toBeTruthy();
  expect(within(pumpkin).getByText(/21 of 28 issues in this milestone closed/)).toBeTruthy();
  expect(within(pumpkin).getByText(/Last known status from .* GitHub rate limit reached/)).toBeTruthy();
  expect(screen.queryByText(/%/)).toBeNull();
  const steel = screen.getByRole('article', { name: 'SteelMC' });
  expect(within(steel).getByText('Upstream status is not available yet.')).toBeTruthy();
  expect(within(steel).getByText('Never loaded')).toBeTruthy();
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Refresh status' }));
  await screen.findByText('Upstream status was refreshed less than a minute ago');
  await user.click(within(steel).getByRole('button', { name: 'Add to egg library' }));
  await waitFor(() => expect(onAdded).toHaveBeenCalled());
  expect(calls).toContain('POST /api/experimental/steelmc/egg');
});
