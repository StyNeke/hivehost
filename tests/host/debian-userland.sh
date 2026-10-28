#!/bin/bash
# Runs inside a privileged debian:trixie container: installs the packages the runbook lists and
# runs the agent's host-level test binaries against Debian's own tools (mkfs.ext4, mount,
# fallocate, e2fsck, resize2fs, tar, iptables) and the agent's host inspection.
# Usage (from the repository root, after `cargo test --no-run`):
#   docker run --rm --privileged --network <net> -e http_proxy=... \
#     -v "$PWD/target/debug/deps:/tests:ro" -v "$PWD/target/debug/manager-agent:/manager-agent:ro" \
#     -v /var/run/docker.sock:/var/run/docker.sock -v "$PWD/tests/host/debian-userland.sh:/run.sh:ro" \
#     debian:trixie bash /run.sh storage-HASH files-HASH ...
set -euo pipefail
. /etc/os-release
echo "== $PRETTY_NAME, kernel $(uname -r)"
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
    e2fsprogs util-linux iproute2 iptables tar ca-certificates docker-cli >/dev/null
for tool in mkfs.ext4 e2fsck resize2fs fallocate mount umount iptables docker; do
    printf '%-10s %s\n' "$tool" "$(command -v "$tool")"
done
mke2fs -V 2>&1 | head -1
iptables --version
status=0
for test in "$@"; do
    echo "== $test"
    if ! "/tests/$test" --test-threads=1 2>&1 | grep -E "^test |test result|panicked|skipped"; then
        status=1
    fi
done
echo "== manager-agent inspect-host"
GM_PRIVATE_IPS=127.0.0.1 GM_DATA_ROOT=/var/lib/hivehost /manager-agent inspect-host | head -c 1500
echo
exit $status
