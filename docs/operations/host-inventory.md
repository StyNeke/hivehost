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

## Not established

Actual legacy disk/CPU allocation policy, Cloudflare route inventory, server game versions, restore quality, load-tested reserves, friend-peer browser access, and quota enforcement remain unverified. No private service credentials were copied into this repository.
