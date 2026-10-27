export class ApiError extends Error {
  constructor(public status: number, message: string) { super(message); }
}

export async function api<T>(path: string, options: RequestInit = {}): Promise<T> {
  const response = await fetch(path, {
    ...options,
    credentials: 'same-origin',
    cache: 'no-store',
    headers: { ...(options.body ? { 'Content-Type': 'application/json' } : {}), ...options.headers },
  });
  const text = await response.text();
  let data: any = null;
  try { data = text ? JSON.parse(text) : null; } catch { /* Non-JSON proxy failures still need a useful error. */ }
  if (!response.ok) {
    const details = Array.isArray(data?.details)
      ? data.details.map((item: { path: string; message: string }) => `${item.path}: ${item.message}`).join('\n') : '';
    throw new ApiError(response.status, [data?.message ?? `Request failed (${response.status})`, details].filter(Boolean).join('\n'));
  }
  return data as T;
}
