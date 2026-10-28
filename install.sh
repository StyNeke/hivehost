#!/bin/bash
# HiveHost installer for Debian (and similar systemd hosts with Docker). Run as root:
#   curl -fsSL https://raw.githubusercontent.com/StyNeke/hivehost/main/install.sh | sudo bash
# First run asks for the private address, port, certificate name and the administrator; later
# runs keep that configuration and only rebuild, reinstall and restart. Never touches an existing
# nginx, Docker containers or firewall rules (the agent adds only its own GM-ISOLATE chains).
set -euo pipefail

REF="${HIVEHOST_REF:-main}"
SOURCE="${HIVEHOST_SOURCE:-https://github.com/StyNeke/hivehost/archive/refs/heads/$REF.tar.gz}"
CONF=/etc/hivehost
BUILD=/var/cache/hivehost-build

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
# Answers come from the terminal even when the script itself arrives on stdin (curl | bash).
ask() {
    local prompt="$1" default="${2:-}" answer
    read -r -p "$prompt${default:+ [$default]}: " answer </dev/tty || die "no terminal to ask on; run the installer interactively"
    printf '%s' "${answer:-$default}"
}

[ "$(id -u)" = 0 ] || die "run as root (… | sudo bash)"
command -v systemctl >/dev/null || die "systemd is required"
command -v docker >/dev/null || die "Docker is required: install it first (https://docs.docker.com/engine/install/debian/)"
docker info >/dev/null 2>&1 || die "Docker is installed but not running"
[ -d /proc/sys/net/ipv6 ] || echo "warning: the kernel has no IPv6; Bedrock and Pumpkin servers will not start" >&2

say "Host packages"
packages=()
for pair in mkfs.ext4:e2fsprogs losetup:util-linux fallocate:util-linux ip:iproute2 iptables:iptables openssl:openssl curl:curl tar:tar; do
    command -v "${pair%%:*}" >/dev/null || PATH="$PATH:/usr/sbin:/sbin" command -v "${pair%%:*}" >/dev/null || packages+=("${pair#*:}")
done
[ -d /usr/share/zoneinfo ] || packages+=(tzdata)
fresh_nginx=0
[ -x /usr/sbin/nginx ] || { packages+=(nginx); fresh_nginx=1; }
if [ ${#packages[@]} -gt 0 ]; then
    apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "${packages[@]}" >/dev/null
fi
# HiveHost runs its own nginx instance; a freshly installed distribution nginx would otherwise
# claim port 80. An nginx that was already there is left exactly as it was.
[ "$fresh_nginx" = 0 ] || systemctl disable --now nginx >/dev/null 2>&1 || true

if [ ! -f "$CONF/api.env" ]; then
    say "Configuration (first install)"
    # Default to a VPN address (Meshnet/Tailscale range), else the first private IPv4.
    guess=$(ip -4 -o addr show | awk '{split($4,a,"/"); print a[1]}' | grep -E '^100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.' | head -1 || true)
    [ -n "$guess" ] || guess=$(ip -4 -o addr show scope global | awk '{split($4,a,"/"); print a[1]}' | grep -E '^(10\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.)' | head -1 || true)
    echo "The panel and game ports are published only on this private address (never public)."
    ip_address=$(ask "Private IP address" "$guess")
    ip -4 -o addr show | grep -qw "inet $ip_address" || die "$ip_address is not an address of this host"
    port=$(ask "Panel HTTPS port" "8443")
    [[ "$port" =~ ^[0-9]+$ ]] && [ "$port" -ge 1 ] && [ "$port" -le 65535 ] || die "invalid port"
    ! ss -Hltn "( sport = :$port )" | grep -q . || die "port $port is already in use"
    ! ss -Hltn "( sport = :19090 )" | grep -q . || die "port 19090 (the local API) is already in use"
    dns_name=$(ask "Extra DNS name for the certificate, e.g. a Meshnet name (blank for none)" "")
    [ -z "$dns_name" ] || [[ "$dns_name" =~ ^[A-Za-z0-9.-]+$ ]] || die "invalid DNS name"
fi

say "Building HiveHost ($REF) in disposable containers"
rm -rf "$BUILD/src" && mkdir -p "$BUILD/src" "$BUILD/cargo" "$BUILD/target" "$BUILD/rustup"
curl -fsSL "$SOURCE" | tar -xz -C "$BUILD/src" --strip-components=1
docker run --rm -v "$BUILD/src:/src" -v "$BUILD/cargo:/cargo" -v "$BUILD/target:/target" -v "$BUILD/rustup:/rustup" \
    -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/target -e RUSTUP_HOME=/rustup -w /src rust:1.99-slim \
    sh -c 'cargo build --locked --release -p manager-agent -p manager-api'
docker run --rm -v "$BUILD/src/web:/web" -w /web node:22-slim sh -c 'npm ci --no-fund --no-audit && npm run build'

say "Installing"
install -m 0755 "$BUILD/target/release/manager-agent" "$BUILD/target/release/manager-api" /usr/local/bin/
rm -rf /usr/local/share/hivehost/web && mkdir -p /usr/local/share/hivehost/docs
cp -r "$BUILD/src/web/dist" /usr/local/share/hivehost/web
install -m 0644 "$BUILD/src/docs/operations/deployment.md" /usr/local/share/hivehost/docs/
install -m 0644 "$BUILD/src"/deploy/systemd/hivehost-*.service /etc/systemd/system/
getent passwd hivehost-api >/dev/null || useradd --system --no-create-home --shell /usr/sbin/nologin hivehost-api
install -d -m 0700 /var/lib/hivehost "$CONF" "$CONF/tls"

if [ ! -f "$CONF/api.env" ]; then
    say "Private certificate authority"
    san="IP:$ip_address${dns_name:+,DNS:$dns_name}"
    # Name constraints: a device that trusts this CA can only be convinced about this panel.
    constraints="permitted;IP:$ip_address/255.255.255.255${dns_name:+,permitted;DNS:$dns_name}"
    (
        cd "$CONF/tls" && umask 077
        openssl req -x509 -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout ca.key -out ca.crt -days 3650 \
            -subj "/CN=HiveHost private CA ($ip_address)" -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
            -addext "keyUsage=critical,keyCertSign,cRLSign" -addext "nameConstraints=critical,$constraints" 2>/dev/null
        openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout panel.key -out panel.csr \
            -subj "/CN=${dns_name:-$ip_address}" 2>/dev/null
        printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=%s\n' "$san" > panel.ext
        openssl x509 -req -in panel.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out panel.crt -days 397 -sha256 -extfile panel.ext 2>/dev/null
        rm panel.csr panel.ext && chmod 0644 ca.crt panel.crt
    )
    printf 'GM_API_UID=%s\nGM_PRIVATE_IPS=%s\n' "$(id -u hivehost-api)" "$ip_address" > "$CONF/agent.env"
    printf 'GM_ORIGIN=https://%s:%s\n' "$ip_address" "$port" > "$CONF/api.env"
    chmod 0600 "$CONF/agent.env" "$CONF/api.env"
    sed -e "s/listen 100\.116\.183\.201:8443 ssl;/listen $ip_address:$port ssl;/" \
        -e "s/server_name panel\.example\.internal;/server_name $ip_address${dns_name:+ $dns_name};/" \
        "$BUILD/src/deploy/nginx/hivehost.conf" > "$CONF/nginx.conf"
    chmod 0644 "$CONF/nginx.conf"
fi

say "Starting services"
systemctl daemon-reload
systemctl enable hivehost-agent hivehost-api hivehost-proxy >/dev/null 2>&1
systemctl restart hivehost-agent hivehost-api hivehost-proxy
sleep 3
systemctl is-active --quiet hivehost-agent hivehost-api hivehost-proxy || die "a service did not start; see: journalctl -u 'hivehost-*'"

if [ ! -f "$CONF/admin-created" ]; then
    say "Administrator"
    admin=$(ask "Administrator username (3-32 letters, digits, _ or -; blank to skip)" "")
    if [ -n "$admin" ]; then
        # The password is read by manager-api from a hidden prompt; it never passes through here.
        systemctl stop hivehost-api
        if runuser -u hivehost-api -- env GM_DATABASE=/var/lib/hivehost-api/app.sqlite /usr/local/bin/manager-api init-admin "$admin" </dev/tty; then
            touch "$CONF/admin-created"
        fi
        systemctl start hivehost-api
    fi
fi

origin=$(sed -n 's/^GM_ORIGIN=//p' "$CONF/api.env")
say "HiveHost is running at $origin"
echo "Trust this CA certificate once on every device that opens the panel: $CONF/tls/ca.crt"
openssl x509 -in "$CONF/tls/ca.crt" -noout -fingerprint -sha256
[ -f "$CONF/admin-created" ] || echo "No administrator yet: run the installer again to create one."
