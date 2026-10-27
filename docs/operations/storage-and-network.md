# Storage and private networking qualification

## Observed private network

The fallback SSH address `100.116.183.201` belongs to the host's **nordlynx** interface. NordVPN Meshnet reports this same device address and connected Windows peers, including the machine making the current SSH connection. Meshnet is confirmed; no inference from the address alone is needed.

LAN: `192.168.178.90/24`; Docker bridges use `172.17.0.0/16` and `172.18.0.0/16`. Host also has global IPv6 addresses: IPv4-only checks would not establish privacy.

Filtered firewall inspection found INPUT policy ACCEPT, FORWARD DROP with Docker forwarding chains, NordVPN-managed nftables hooks, and existing TCPShield rules for game ports 25565/25567. Preserve these rules. Docker DNAT currently publishes the existing game only on its LAN address. A full public exposure audit was not performed.

## Proposed deployment settings — not installed

- Web API loopback `127.0.0.1:19090`, private HTTPS proxy `100.116.183.201:8443`; verify availability again at deployment time.
- TLS must validate the hostname or IP used by friends. Proposed initial approach: a private CA and explicit client trust setup, with a certificate SAN for the chosen private address/name. Do not ship browser instructions to ignore TLS warnings. CA trust setup is a deployment prerequisite.
- Game allocation pool bound to the verified Meshnet IP, initially a separately validated range above 25600. Check TCP and UDP, host sockets and legacy reservations before every assignment. Do not publish games on wildcard IPv4/IPv6 or reuse 25565/25567.
- Application data root `/var/lib/game-manager`, agent socket `/run/game-manager/agent.sock`; distinct from all observed legacy directories. These are proposals pending deployment checks, not existing paths.
- Create a dedicated bridge and qualify ingress/egress isolation. Block access to host control services, LAN services, legacy bridges, metadata addresses and agent/Docker sockets while permitting necessary public artifact downloads. Prove Docker forwarding does not bypass the intended rules.
- No public Cloudflare route for the panel. No changes to existing routes are necessary to build locally.

## Hard disk quota blocker

Root ext4 has neither the `quota`/`project` filesystem feature nor project-quota mount flags in inspected output; `setquota` and `xfs_quota` are absent from PATH. No supported hard quota path is qualified. Docker writable-layer accounting cannot bound bind-mounted game data; the reported driver is not classic overlay2.

Proposed fallback: a dedicated XFS volume mounted with project quotas for server data, with separately budgeted backups/staging. No spare disk was observed. A preallocated loopback volume is an option to assess, not a decision to consume existing space. Do not repartition the host, change the Docker storage driver, remount root, or create a volume without a concrete reviewed storage rollout.

Until quota enforcement and memory reservations are resolved, host provisioning must fail closed. Continue implementing local domain/API behavior without pretending host qualification passed.
