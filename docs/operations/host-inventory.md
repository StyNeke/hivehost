# Host inventory — 2026-10-27

Read-only SSH inspection; no remote files, containers, firewall rules, or services changed.

- Debian 13.6; i5-9600K, six logical CPUs.
- Memory: 25,075,331,072 bytes total (23.35 GiB); about 20.76 billion bytes available at inspection. Available memory is a snapshot, not an admission budget.
- `/dev/sda2`: ext4, 110,441,844,736 bytes total; 76,898,758,656 bytes available. Mounted `rw,relatime,errors=remount-ro`.
- Docker 29.7.2, containerd image store reported as `overlayfs`, cgroup v2. Data exists under both `/var/lib/docker` (521 MiB) and `/var/lib/containerd` (16 GiB).
- Pterodactyl volumes: 5.7 GiB. Its backups directory currently uses 4 KiB. `/DATA`: 16 MiB.
- Existing networks: Docker default bridge, host, none, and `pterodactyl_nw` using `pterodactyl0`. No application network exists yet.
- Existing services include CasaOS, Wings, Pterodactyl queue worker, Nginx/PHP, MariaDB, Redis, Cloudflare Tunnel, NordVPN, Samba, and rclone.
- Nginx Pterodactyl site listens on IPv4/IPv6 port 8085 and serves `/var/www/pterodactyl/public`. Other observed listeners include 80, 8080 and 2022. Do not reuse these ports.

## Legacy game reservations

| Container prefix | State | Memory limit (bytes) |
| --- | --- | ---: |
| `94125d3b` | running | 22,020,096,000 |
| `b556a801` | stopped | 4,613,734,400 |
| `2e83460d` | stopped | 16,515,072,000 |
| `57d8f102` | stopped | 8,808,038,400 |

These game limits total **48.39 GiB**, already exceeding physical memory before OS/services overhead. The stale exited installer has a further 1,234,803,098-byte limit; do not remove it or count it as a confirmed active game. Existing inspected containers have Docker restart policy `no` and no explicit CPU quota. The running game uses LAN TCP/UDP port 25567.

**Provisioning blocker:** approved conservative reservations leave zero new RAM budget under the current legacy configuration. Build/test locally or on disposable infrastructure; reconcile the legacy limits explicitly before deploying new game workloads. Do not automatically resize, delete, or stop legacy games.

## Lifecycle

Docker, Wings, and NordVPN currently show 90-second systemd stop timeouts. Wings orders after Docker. New application units need their own stop budget exceeding 120 seconds plus cleanup, ordering after Docker and required mounts/network services so they stop first. Inspect runtime behavior on staging before any agreed live reboot check.

## Re-inventory — 2026-10-28 (read-only, over SSH with a dedicated key)

Same host, nothing installed or changed. Differences from the 2026-10-27 snapshot:

- Kernel `6.12.105+deb13-amd64`; IPv6 enabled (`/proc/sys/net/ipv6` present, `disable_ipv6=0`), so
  Bedrock's dual-stack sockets can open here.
- Memory unchanged (25,075,331,072 bytes); 20.65 GB available, swap 5.77 GiB unused.
- `/dev/sda2`: 75,597,799,424 bytes available (was 76,898,758,656); containerd grew to 17 GiB.
- Docker still 29.7.2, containerd `overlayfs` snapshotter, cgroup v2, systemd cgroup driver.
- Containers: the same four legacy games plus the exited installer `745af164…_installer`
  (node:20, exited 137 six weeks ago, 1,234,803,098-byte limit). Its volume directory no longer
  exists; it is an orphan, but the agent still counts it as a legacy reservation.
- **CPU is a second blocker:** no legacy container has a CPU quota, and the agent counts each
  uncapped container as the whole machine (6,000 millicores). Five containers = 30,000 m against a
  5,000 m budget, so lowering RAM alone does not unblock creation.
- The running game (`94125d3b`, NeoForge 21.1.265 for 1.21.1, 37 mods) peaked at
  2,836,484,096 bytes (cgroup `memory.peak`, 38 h uptime) under a 22,020,096,000-byte limit;
  2.52 GiB at inspection, 4.4 % CPU.
- Legacy startup: `-Xms128M -XX:MaxRAMPercentage=95.0`, so each heap follows its container limit.
  `SERVER_MEMORY` (MiB, Wings adds 5–15 % overhead to get the container limit): 20000, 4000, 15000,
  8000. Ports: 25567 (running, LAN only), 8083, 25565, 25566.
- Legacy volumes: 4.2 GiB (`2e83460d`, 26.2 `server.jar`, whitelist), 958 MiB (`57d8f102`,
  NeoForge 21.1.252, 40 mods), 319 MiB (`94125d3b`), 260 MiB (`b556a801`, 26.2 `server.jar`).
- Listeners now also include Wings on `*:8080` and `*:2022`, Samba on 139/445, `nordfileshare`
  on `100.116.183.201:49111`, CasaOS on `*:80`; nginx serves only Pterodactyl on 8085. Ports 8443
  and 19090 are free.
- Routes/networks: `172.17.0.0/16` (docker0, down), `172.18.0.0/16` + `fdba:17c8:6c94::/64`
  (pterodactyl0), `192.168.178.0/24`, `100.64.0.0/10` (nordlynx). Nothing in `172.30.0.0/16`.
- Firewall: iptables (nft backend) unchanged — INPUT ACCEPT, FORWARD DROP, Docker chains,
  `DOCKER-USER` → `TCPSHIELD` for ports 25565/25567 (allows 198.178.119.0/24, 104.234.6.0/25,
  192.168.178.0/24). `ip raw` drops traffic to 172.18.0.2 not from pterodactyl0. NordVPN's
  `inet nordvpn` table accepts Meshnet input only from its allow-list of peers.
- Meshnet name of this host: `stijnpikkert-altai.nord` (not usable for a public certificate).
- No `/var/lib/game-manager`, `/etc/game-manager`, `game-manager-api` user or UIDs 200000–299999.

## Not established

Actual legacy disk/CPU allocation policy, Cloudflare route inventory, server game versions, restore quality, load-tested reserves, friend-peer browser access, and quota enforcement remain unverified. No private service credentials were copied into this repository.
