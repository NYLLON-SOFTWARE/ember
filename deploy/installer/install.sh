#!/bin/sh
# This file is executed only after the versioned bundle's SHA256 has been checked.
set -eu
PATH=/usr/sbin:/usr/bin:/sbin:/bin
export PATH
DOCKER_HOST=unix:///var/run/docker.sock
export DOCKER_HOST
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
umask 077
bundle_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
if [ "$(id -u)" -ne 0 ]; then
  command -v sudo >/dev/null 2>&1 || { echo 'Run this installer as root (or install sudo).' >&2; exit 1; }
  exec sudo sh "$bundle_dir/install.sh" "$@"
fi
[ "$(uname -s)" = Linux ] || { echo 'Ember supports Linux servers only.' >&2; exit 1; }
case "$(uname -m)" in x86_64|aarch64|arm64) ;; *) echo 'Supported architectures: amd64 and arm64.' >&2; exit 1;; esac
# /etc/os-release is an OS-owned shell assignment file.
# shellcheck source=/dev/null
. /etc/os-release
case "${ID:-}:${VERSION_ID:-}" in ubuntu:24.04|debian:13) ;; *) echo 'Supported systems: Ubuntu 24.04 and Debian 13.' >&2; exit 1;; esac
if [ ! -f /etc/ember/state.json ]; then
  for conflict in /etc/ember /var/lib/ember/storage /usr/local/bin/emberctl /usr/local/lib/ember /var/lib/once /etc/once /usr/local/bin/once; do
    [ ! -e "$conflict" ] && [ ! -L "$conflict" ] || { echo "Refusing unmanaged or incomplete installation: $conflict" >&2; exit 1; }
  done
fi
# Dependencies come from the distribution's signed repositories. No downloaded shell scripts run.
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y ca-certificates curl python3
ember_domain=$(/usr/bin/python3 "$bundle_dir/emberctl.py" preflight --bundle-dir "$bundle_dir" "$@")
if ! command -v docker >/dev/null 2>&1; then
  [ ! -S /var/run/docker.sock ] || { echo "An existing Docker daemon has no supported CLI; resolve it before installing Ember." >&2; exit 1; }
  for conflict in /etc/apt/sources.list.d/docker.list /etc/apt/sources.list.d/docker.sources; do
    [ ! -e "$conflict" ] || { echo "Existing Docker repository needs operator review: $conflict" >&2; exit 1; }
  done
  for package in docker.io docker-compose docker-compose-v2 docker-doc podman-docker containerd runc; do
    if dpkg-query -W -f='${Status}' "$package" 2>/dev/null | grep -q 'install ok installed'; then
      echo "Conflicting package $package is installed; resolve it before installing Ember." >&2
      exit 1
    fi
  done
  install -d -m 0755 /etc/apt/keyrings
  curl --proto '=https' --tlsv1.2 -fsSL "https://download.docker.com/linux/$ID/gpg" -o /etc/apt/keyrings/docker.asc
  chmod 0644 /etc/apt/keyrings/docker.asc
  architecture=$(dpkg --print-architecture)
  cat > /etc/apt/sources.list.d/docker.sources <<REPO
Types: deb
URIs: https://download.docker.com/linux/$ID
Suites: $VERSION_CODENAME
Components: stable
Architectures: $architecture
Signed-By: /etc/apt/keyrings/docker.asc
REPO
  apt-get update
  DEBIAN_FRONTEND=noninteractive apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin
  systemctl enable --now docker
fi
systemctl enable --now docker
docker info >/dev/null 2>&1 || { echo 'Docker is installed but its daemon is unavailable; resolve it and retry.' >&2; exit 1; }
exec /usr/bin/python3 "$bundle_dir/emberctl.py" install --bundle-dir "$bundle_dir" --domain "$ember_domain" "$@"
