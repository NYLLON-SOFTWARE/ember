#!/bin/sh
# Keep the disposable ARM runner on the firmware used by the verified M5 cases.
set -eu
umask 077
[ "$(id -u)" -eq 0 ] || { printf '%s\n' 'ARM firmware preparation requires root' >&2; exit 1; }
[ "$(uname -s)" = Linux ] || { printf '%s\n' 'ARM firmware preparation requires Linux' >&2; exit 1; }
case "$(uname -m)" in
  aarch64|arm64) ;;
  *) printf '%s\n' 'ARM firmware preparation requires native ARM64' >&2; exit 1 ;;
esac
firmware_dir=$(mktemp -d)
trap 'rm -rf "$firmware_dir"' EXIT
trap 'exit 1' HUP INT TERM
curl --disable --proto '=https' --proto-redir '=https' -fsSL \
  --connect-timeout 15 --max-time 120 --output "$firmware_dir/firmware.deb" \
  https://archive.ubuntu.com/ubuntu/pool/main/e/edk2/qemu-efi-aarch64_2024.02-2ubuntu0.10_all.deb
printf '%s  %s\n' \
  fad8e80e929cd00dae8c5c8e5dd3ec85f37d9a832eea776957e5e2cf86ef2ab5 \
  "$firmware_dir/firmware.deb" | sha256sum --check --status
dpkg-deb --extract "$firmware_dir/firmware.deb" "$firmware_dir/extracted"
firmware_source="$firmware_dir/extracted/usr/share/qemu-efi-aarch64/QEMU_EFI.fd"
printf '%s  %s\n' \
  d2d1228a664adbaed2148c2661e30c930af9bf09f03b6d5aa2a8bbc9cb5cab43 \
  "$firmware_source" | sha256sum --check --status
install -d -o root -g root -m 0755 /usr/share/qemu-efi-aarch64
install -o root -g root -m 0644 "$firmware_source" \
  /usr/share/qemu-efi-aarch64/QEMU_EFI.fd
printf '%s  %s\n' \
  d2d1228a664adbaed2148c2661e30c930af9bf09f03b6d5aa2a8bbc9cb5cab43 \
  /usr/share/qemu-efi-aarch64/QEMU_EFI.fd | sha256sum --check --status
printf '%s\n' 'Pinned ARM firmware 2024.02-2ubuntu0.10 verified'
