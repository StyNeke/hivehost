# Deployment runbook (Debian)

This installs the manager next to existing services without changing them. Read the blockers in
[progression.md](../../progression.md) first: legacy container limits currently consume the
safe budget on the target host, and nothing here lowers them.

## What runs where

| Component | User | Authority |
| --- | --- | --- |
| `manager-agent` | root | Docker, loop-mounted per-server quota volumes under `GM_DATA_ROOT`, per-server UIDs |
| `manager-api` | `game-manager-api` | SQLite database, the agent socket, outbound HTTPS to Modrinth |
| `game-manager-proxy` (its own nginx instance) | root master, `www-data` workers | private HTTPS on the private address only |

The agent only acts on containers, volumes and UIDs recorded in its own registry. The API never
receives Docker access, host paths or container IDs from browsers.

## 1. Build

On a build machine with the pinned toolchain and Node:

```bash
cargo build --release -p manager-agent -p manager-api
```

```bash
npm --prefix web ci && npm --prefix web run build
```

## 2. Install files

```bash
sudo install -m 0755 target/release/manager-agent target/release/manager-api /usr/local/bin/
```

```bash
sudo mkdir -p /usr/local/share/game-manager && sudo cp -r web/dist /usr/local/share/game-manager/web
```

```bash
sudo install -m 0644 deploy/systemd/game-manager-*.service /etc/systemd/system/
```

The proxy runs nginx with its own configuration, pid file and temp paths
(`game-manager-proxy.service`), so an existing nginx (for example Pterodactyl's) is never
reconfigured or reloaded. This matters at boot: on the Debian host nginx starts at ~8 s but
NordVPN gives `nordlynx` its Meshnet address only at ~19.6 s. A site bound to that address inside a
shared nginx would make the whole nginx fail to start; the separate unit simply retries every
5 seconds until the address exists. For the same reason the agent waits (up to 5 minutes) for
every `GM_PRIVATE_IPS` address before it restarts servers after a boot.

Requirements on the host: Docker, `e2fsprogs` (mkfs.ext4, e2fsck, resize2fs), `util-linux`
(fallocate, mount), `iproute2`, `iptables` and the time zone database (`tzdata`). Kernel IPv6 must
stay available (it may be unconfigured): current Bedrock servers and Pumpkin open dual-stack
sockets and fail without it. Per-server quotas use fully allocated ext4 images, so `GM_DATA_ROOT` must be on a
filesystem with room for every server's disk limit plus `GM_DISK_RESERVE_BYTES`.

## 3. Users and configuration

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin game-manager-api
```

```bash
sudo install -d -m 0700 /var/lib/game-manager /etc/game-manager
```

`/etc/game-manager/agent.env` (mode 0600):

```
GM_API_UID=<uid of game-manager-api, from `id -u game-manager-api`>
GM_PRIVATE_IPS=100.116.183.201
# Optional:
# GM_BACKUP_ROOT=/var/lib/game-manager-backups  backups (root:API group 0750; may be another disk)
# GM_DISK_RESERVE_BYTES=21474836480   space kept free for the OS (default 10 GiB); backups never use it
# GM_STOP_GRACE_SECS=120              per-server graceful stop time before a forced stop
# GM_GAME_UID_BASE=200000             first per-server UID; keep the range unused
# GM_NETWORK_SUBNET=172.30.250.0/24   managed Docker network; must not overlap others
# GM_CONTAINER_PROXY=http://host:port only if outbound traffic must use a proxy
# GM_CONTAINER_CA_FILE / GM_CONTAINER_JAVA_TRUSTSTORE for proxies that re-sign TLS
# GM_NETWORK_ISOLATION=on             firewall rules for the game subnet (see below); off disables
```

`/etc/game-manager/api.env` (mode 0600):

```
# The exact URL people type: an IP origin keeps working when Meshnet names do not resolve.
GM_ORIGIN=https://100.116.183.201:8443
# Optional:
# GM_BACKUP_ROOT=/var/lib/game-manager-backups   must match the agent's
# GM_MODRINTH_API / GM_GITHUB_API                  only for testing against stand-ins
```

The agent gives the socket (0660) and `/run/game-manager` (0750) to the API user's primary
group at start-up and only answers that UID.

## 4. First administrator

```bash
sudo -u game-manager-api GM_DATABASE=/var/lib/game-manager-api/app.sqlite /usr/local/bin/manager-api init-admin YOUR_NAME
```

Run `systemctl start game-manager-api` once first if `/var/lib/game-manager-api` does not exist
yet (systemd creates it), then stop it, create the administrator and start it again. The password
is read from a hidden prompt.

## 5. Private HTTPS

Install `deploy/nginx/game-manager.conf` as `/etc/game-manager/nginx.conf` (0644) with your
private address and name. Keep it out of any public tunnel.

A private CA whose certificate devices install once. Its `nameConstraints` limit it to the
panel's address and Meshnet name, so a device that trusts it cannot be fooled for any other site:

```bash
sudo install -d -m 0700 /etc/game-manager/tls && cd /etc/game-manager/tls && sudo sh -c 'umask 077
openssl req -x509 -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout ca.key -out ca.crt -days 3650 -subj "/CN=Game manager private CA" -addext "basicConstraints=critical,CA:TRUE,pathlen:0" -addext "keyUsage=critical,keyCertSign,cRLSign" -addext "nameConstraints=critical,permitted;IP:100.116.183.201/255.255.255.255,permitted;DNS:stijnpikkert-altai.nord"
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout panel.key -out panel.csr -subj "/CN=stijnpikkert-altai.nord"
printf "basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
subjectAltName=IP:100.116.183.201,DNS:stijnpikkert-altai.nord
" > panel.ext
openssl x509 -req -in panel.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out panel.crt -days 397 -sha256 -extfile panel.ext
rm panel.csr panel.ext; chmod 0644 ca.crt panel.crt'
```

Renew `panel.crt` before it expires (397 days, the browser maximum) with the same `x509 -req`
step, then `systemctl reload game-manager-proxy`. Trust `ca.crt` on each device: Windows
`certutil -user -addstore Root ca.crt`; Android Settings → Security → Install a certificate → CA
certificate; iOS/iPadOS install the profile, then Settings → General → About → Certificate Trust
Settings. Then:

```bash
sudo systemctl enable --now game-manager-agent game-manager-api game-manager-proxy
```

**NordVPN Meshnet and game ports.** Game ports are published on the Meshnet address through
Docker's DNAT, so player traffic is *forwarded*, not delivered to the host. NordVPN's forward
chain drops forwarded Meshnet traffic unless the player's device has **Allow Routing** and **Allow
Local Network Access** enabled for this server (in that peer's Meshnet permissions on this
host). Without them the panel works (it is local) but game joins time out.

## 6. Qualify and accept

1. Sign in, import the egg (for example `tests/fixtures/eggs/fabric.json`) and, as
   administrator, test-install it. When it works, mark the revision qualified so teammates can
   use it.
2. Run the browser acceptance against the deployment:

```bash
GM_URL=https://panel.example.internal:8443 GM_USER=YOUR_NAME GM_PASSWORD='…' GM_EGG=tests/fixtures/eggs/fabric.json GM_ADDRESS_IP=100.116.183.201 GM_ADDRESS_PORT=25610 node tests/host/fabric-carpet-acceptance.cjs
```

It creates "Carpet Bots", installs Carpet, spawns three bots, checks they joined and stops the
server. Delete the server afterwards from its Delete tab.

## Stopping and rebooting

`systemctl stop game-manager-agent` asks every running server to stop (its egg stop command),
waits up to the grace period concurrently, force-stops the rest and records that. Servers that
were running start again, one at a time, when the agent starts. Docker restart policies are not
used for managed containers; the agent's crash policy (3 restarts per 10 minutes) applies.

Only one agent may use a data directory; a second instance refuses to start.

## Backups

Backups are gzip tar archives under `GM_BACKUP_ROOT/<server>/`, verified after writing and
recorded with a SHA-256. A running server is stopped for the backup and started again. Restores
need a stopped server, check the checksum, save the current files as a pre-restore backup, then
swap the restored files in; an interrupted restore is rolled back to the original files the next
time the agent starts. Set `GM_BACKUP_ROOT` in `api.env` too if you change it (the API streams
downloads from it). These are local backups: copy them elsewhere to survive a disk failure.

## Network isolation

At start-up, and before every install or start, the agent rebuilds two iptables chains that it
owns. They apply only to traffic from `GM_NETWORK_SUBNET`:

- `GM-ISOLATE-IN` (jumped from `INPUT`): containers may not open connections to the host itself
  (web panels, databases, SSH, Samba…). The only exception is `GM_CONTAINER_PROXY` when it listens
  on the bridge gateway.
- `GM-ISOLATE-FWD` (jumped from `DOCKER-USER`): containers may not reach private, LAN, VPN/Meshnet
  (100.64.0.0/10), link-local or other Docker ranges. Replies to connections players opened are
  allowed, and so is the internet.

Docker adds its own standard rules for the `gm-games` bridge (`br-…`) and a MASQUERADE rule for the
subnet. Nothing else in the host firewall is touched, so existing rules (for example TCPShield or the
legacy game ports) stay as they are. Check after the first start:

```bash
sudo iptables -S | grep GM-ISOLATE
```

## Updates and schedules

An update runs the egg's installer again on the existing files after a verified pre-update
backup. If the installer fails, or leaves no recognisable game behind, the previous files are put
back automatically; the same happens to an update interrupted by an agent restart. Once an
updated server has run, the world is never rolled back automatically: the pre-update backup stays
in the Backups tab. Scheduled updates only install newer builds of the installed game version and
pause their schedule after a failure. Schedule times use the host's time zone database.

## Experimental servers

The Experimental panel in the egg library offers Pumpkin and SteelMC for **new test servers**.
Their status comes from GitHub (cached for an hour; shown as stale with the reason when GitHub is
unreachable or rate-limits the panel). The API needs outbound HTTPS to `api.github.com` for that.
Pumpkin sends anonymous telemetry by default (`[telemetry]` in `pumpkin.toml`).

## Migrating a Pterodactyl server (one at a time, in an agreed window)

Never let two panels run the same world. For each server:

1. In the new panel, create a server with the same egg, version and loader, an unused address,
   and limits that fit the budget. Let it install.
2. Stop the server in Pterodactyl. Copy its volume and pack the copy, leaving the original
   untouched:

   ```bash
   sudo cp -a /var/lib/pterodactyl/volumes/<uuid> /var/tmp/migrate-<name>
   ```

   ```bash
   sudo tar czf /var/tmp/migrate-<name>.tar.gz -C /var/tmp/migrate-<name> .
   ```

3. In the new server's Backups tab, use **Import server files** to upload the archive. It is
   verified and listed as an imported backup. Restore it; the freshly installed files are kept as
   a pre-restore backup.
4. Start the server, check the console and the world, and join with a client. Re-add members,
   schedules and Modrinth-managed mods by hand (mods dropped in by hand show as "added outside
   this panel").
5. Only then move players to the new address. Keep the Pterodactyl server stopped but intact
   until the new one has run for a while. To roll back, stop the new server and start the old one.

## Checks on a new host

On a build machine, `cargo test -p manager-agent --no-run` prints the agent test binaries. Copy
the `storage`, `files`, `backups`, `file_manager` and `linux_socket` ones to the host and run each
as root; they use temporary directories and loop images only, and install nothing:

```bash
sudo ./storage-<hash> --test-threads=1
```

`tests/host/debian-userland.sh` does the same inside a disposable privileged `debian:trixie`
container (it installs packages, so never run it on the host itself).

Real client joins (the server must be in offline mode, `online-mode=false`, and for Bedrock also
`allow-list=false`, during the test only):

```bash
node tests/host/client-join.cjs java <address> <port> <version>
```

```bash
node tests/host/client-join.cjs bedrock-ping <address> <port>
```
