# Deployment runbook (Debian)

This installs the manager next to existing services without changing them. Read the blockers in
[progression.md](../../progression.md) first: legacy container limits currently consume the
safe budget on the target host, and nothing here lowers them.

## What runs where

| Component | User | Authority |
| --- | --- | --- |
| `manager-agent` | root | Docker, loop-mounted per-server quota volumes under `GM_DATA_ROOT`, per-server UIDs |
| `manager-api` | `game-manager-api` | SQLite database, the agent socket, outbound HTTPS to Modrinth |
| nginx (or another proxy) | as packaged | private HTTPS on the private address only |

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

Requirements on the host: Docker, `e2fsprogs` (mkfs.ext4), `util-linux` (fallocate, mount) and
`iproute2`. Per-server quotas use fully allocated ext4 images, so `GM_DATA_ROOT` must be on a
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
# GM_DISK_RESERVE_BYTES=21474836480   space kept free for the OS (default 10 GiB)
# GM_STOP_GRACE_SECS=120              per-server graceful stop time before a forced stop
# GM_GAME_UID_BASE=200000             first per-server UID; keep the range unused
# GM_NETWORK_SUBNET=172.30.250.0/24   managed Docker network; must not overlap others
# GM_CONTAINER_PROXY=http://host:port only if outbound traffic must use a proxy
# GM_CONTAINER_CA_FILE / GM_CONTAINER_JAVA_TRUSTSTORE for proxies that re-sign TLS
```

`/etc/game-manager/api.env` (mode 0600):

```
GM_ORIGIN=https://panel.example.internal:8443
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

Adapt `deploy/nginx/game-manager.conf`: listen only on the private address, use a certificate
your devices trust, and keep it out of any public tunnel. Then:

```bash
sudo systemctl enable --now game-manager-agent game-manager-api
```

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
