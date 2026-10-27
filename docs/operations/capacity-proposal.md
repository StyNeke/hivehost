# Capacity proposal — not applied

User delegated choosing the next step on 2026-10-27. Continue local implementation and preserve live workloads while gathering evidence for an eventual limit change.

The host has 23.35 GiB physical memory, while four legacy game container limits total 48.39 GiB. There is no safe new reservation under those settings. Actual observed usage around 2.5 GiB for the running game does not establish its peak requirement.

An illustrative initial split, **subject to workload validation**, is:

| Purpose | RAM |
| --- | ---: |
| OS, CasaOS, databases, proxy, panel/agent and working headroom | 4 GiB |
| Existing running Java server | 8 GiB |
| Three stopped legacy games | 2 GiB each |
| First new test game | 4 GiB |
| Remaining unallocated headroom | about 1.35 GiB |

This is a feasible arithmetic example, not proof that the games will operate within those caps. Before applying it, inspect their installation types, JVM heap settings, player/mod workload and representative peaks. A heap larger than a new container cap must be adjusted at a coordinated restart. If a legacy game needs more, reserve more and reduce the new-game budget rather than overcommit.

Do not cap legacy CPU arbitrarily: existing games have no CPU quotas. Qualify a shared CPU budget from workload observation, reserve host capacity, and never promise dedicated CPU performance from a container limit.

Disk has a separate gate: no hard quota implementation is qualified. Keep space for the OS, image pulls, install/update staging, backup replacement and restore recovery copies. Backups need real space checks; never assume compression will make an operation fit. A dedicated-volume proposal and restore rehearsal precede any storage rollout.
