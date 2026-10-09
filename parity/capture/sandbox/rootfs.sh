#!/usr/bin/env bash
# Materializes the pinned Playwright image (the FROM line of parity/Dockerfile.playwright) as a
# plain root filesystem, and installs the harness' locked node_modules inside it. Used by
# parity/bin/capture when the Docker daemon isn't reachable: the same image bytes then run under
# bubblewrap instead of docker. Prints the rootfs path.
set -euo pipefail

PARITY="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE_LINE="$(grep -m1 '^FROM ' "$PARITY/Dockerfile.playwright" | awk '{print $2}')"
REPO_PATH="${IMAGE_LINE%%:*}"                       # mcr.microsoft.com/playwright
REGISTRY="${REPO_PATH%%/*}"
NAME="${REPO_PATH#*/}"
INDEX_DIGEST="${IMAGE_LINE##*@}"
CACHE="${PARITY_CACHE:-$HOME/.cache/ember-parity}"
ROOTFS="$CACHE/rootfs-${INDEX_DIGEST#sha256:}"
LOCKHASH="$(sha256sum "$PARITY/package-lock.json" | cut -c1-16)"

json() { node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{const j=JSON.parse(s);console.log($1)})"; }

if [ ! -f "$ROOTFS/.complete" ]; then
  echo "Fetching $IMAGE_LINE into $ROOTFS" >&2
  rm -rf "$ROOTFS" "$ROOTFS.tmp"; mkdir -p "$ROOTFS.tmp"
  ACCEPT='application/vnd.oci.image.index.v1+json,application/vnd.docker.distribution.manifest.list.v2+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.v2+json'
  MANIFEST_DIGEST="$(curl -fsSL -H "Accept: $ACCEPT" "https://$REGISTRY/v2/$NAME/manifests/$INDEX_DIGEST" \
    | json "j.manifests.find(m=>m.platform.architecture==='amd64'&&m.platform.os==='linux').digest")"
  LAYERS="$(curl -fsSL -H "Accept: $ACCEPT" "https://$REGISTRY/v2/$NAME/manifests/$MANIFEST_DIGEST" \
    | json "j.layers.map(l=>l.digest).join('\n')")"
  for layer in $LAYERS; do
    blob="$CACHE/blobs/${layer#sha256:}"
    mkdir -p "$CACHE/blobs"
    if [ ! -f "$blob" ]; then
      curl -fsSL -o "$blob.part" "https://$REGISTRY/v2/$NAME/blobs/$layer"
      echo "${layer#sha256:}  $blob.part" | sha256sum -c --quiet
      mv "$blob.part" "$blob"
    fi
    # Apply OCI whiteouts from this layer before extracting it over the lower layers.
    tar -tzf "$blob" | { grep -E '(^|/)\.wh\.' || true; } | while read -r wh; do
      dir="$(dirname "$wh")"; base="$(basename "$wh")"
      if [ "$base" = ".wh..wh..opq" ]; then
        find "$ROOTFS.tmp/$dir" -mindepth 1 -maxdepth 1 -exec rm -rf {} + 2>/dev/null || true
      else
        rm -rf "$ROOTFS.tmp/$dir/${base#.wh.}"
      fi
    done
    tar -xzf "$blob" -C "$ROOTFS.tmp" --no-same-owner --delay-directory-restore \
      --exclude='dev/*' --exclude='*.wh.*' --warning=no-unknown-keyword 2>/dev/null || true
  done
  chmod -R u+rwX "$ROOTFS.tmp"
  mv "$ROOTFS.tmp" "$ROOTFS"
  touch "$ROOTFS/.complete"
fi

# Same as the Dockerfile's RUN step; the runner binds $MODULES/node_modules at /node_modules.
MODULES="$ROOTFS/opt/parity-modules/$LOCKHASH"
if [ ! -f "$MODULES/.complete" ]; then
  echo "Installing locked node_modules inside the image" >&2
  mkdir -p "$MODULES"
  cp "$PARITY/package.json" "$PARITY/package-lock.json" "$MODULES/"
  bwrap --bind "$ROOTFS" / --dev /dev --proc /proc --tmpfs /tmp --ro-bind /etc/resolv.conf /etc/resolv.conf \
    --share-net --unshare-user --uid 0 --gid 0 --setenv HOME /root --chdir "/opt/parity-modules/$LOCKHASH" \
    npm ci --omit=dev --no-audit --no-fund >&2
  touch "$MODULES/.complete"
fi

mkdir -p "$ROOTFS/node_modules" # mount point for $MODULES/node_modules
echo "$ROOTFS $MODULES/node_modules"
