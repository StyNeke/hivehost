<script lang="ts">
  const paths = {
    servers: 'M4 4h16v6H4z M4 14h16v6H4z M7 7h.01 M7 17h.01',
    library: 'M4 4h4v16H4z M10 4h4v16h-4z M16 5l4-1 3 15-4 1z',
    plus: 'M12 5v14 M5 12h14',
    users: 'M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2 M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8 M22 21v-2a4 4 0 0 0-3-3.87 M16 3.13a4 4 0 0 1 0 7.75',
    search: 'M21 21l-4.3-4.3 M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16',
    lock: 'M5 11h14v10H5z M8 11V7a4 4 0 0 1 8 0v4',
    arrow: 'M5 12h14 M13 6l6 6-6 6',
    logout: 'M9 5H4v14h5 M10 12h11 M17 8l4 4-4 4',
    close: 'M6 6l12 12 M18 6 6 18',
    chevron: 'M9 5l7 7-7 7',
  };
  let { name, size = 18 }: { name: keyof typeof paths; size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[name]} /></svg>
