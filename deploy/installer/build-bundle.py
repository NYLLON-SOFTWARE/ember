#!/usr/bin/env python3
"""Build a reproducible, digest-pinned public installer from reviewed source."""
import argparse
import gzip
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import re
import tarfile

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("emberctl", HERE / "emberctl.py")
manager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(manager)


def build(version, image_digest, source_sha, output):
    release = manager.validate_manifest({"schema": 1, "version": version, "source_sha": source_sha,
                                         "image": f"{manager.IMAGE}@{image_digest}"}, bundle=False)
    output.mkdir(parents=True, exist_ok=True)
    encoded = (json.dumps(release, sort_keys=True, indent=2) + "\n").encode()
    bundle_name = f"ember-installer-{version}.tar.gz"
    bundle = output / bundle_name
    files = {name: (HERE / name).read_bytes() for name in ("install.sh", "emberctl", "emberctl.py")}
    files["install.json"] = encoded
    with bundle.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for name, data in sorted(files.items()):
                info = tarfile.TarInfo(name)
                info.size, info.mtime, info.uid, info.gid = len(data), 0, 0, 0
                info.mode = 0o755 if name in {"install.sh", "emberctl"} else 0o644
                archive.addfile(info, io.BytesIO(data))
    bundle_sha = hashlib.sha256(bundle.read_bytes()).hexdigest()
    release["bundle"] = {"url": manager.release_url(version, bundle_name), "sha256": bundle_sha}
    manager.validate_manifest(release)
    (output / "release.json").write_text(json.dumps(release, sort_keys=True, indent=2) + "\n")
    # Keep all executable statements inside main: an interrupted download must not start installation.
    bootstrap = f'''#!/bin/sh
# Ember {version}; reviewed source {source_sha}. Versioned bundle SHA256 is pinned below.
main() {{
  set -eu
  PATH=/usr/sbin:/usr/bin:/sbin:/bin
  export PATH
  umask 077
  [ "$(uname -s)" = Linux ] || {{ echo 'Ember supports Linux servers only.' >&2; exit 1; }}
  for dependency in curl sha256sum tar mktemp; do
    command -v "$dependency" >/dev/null 2>&1 || {{ echo "Missing prerequisite: $dependency" >&2; exit 1; }}
  done
  ember_tmp=$(mktemp -d)
  trap 'rm -rf "$ember_tmp"' EXIT HUP INT TERM
  curl --proto '=https' --tlsv1.2 -fsSL --max-filesize 10485760 '{release["bundle"]["url"]}' -o "$ember_tmp/bundle.tar.gz"
  printf '%s  %s\\n' '{bundle_sha}' "$ember_tmp/bundle.tar.gz" | sha256sum -c - >/dev/null
  # The checksum covers the complete, repo-built archive before any extraction or execution.
  tar -xzf "$ember_tmp/bundle.tar.gz" -C "$ember_tmp"
  sh "$ember_tmp/install.sh" "$@"
}}
main "$@"
'''
    (output / "bootstrap.sh").write_text(bootstrap)
    names = (bundle_name, "release.json", "bootstrap.sh")
    (output / "SHA256SUMS").write_text("".join(f"{hashlib.sha256((output / name).read_bytes()).hexdigest()}  {name}\n" for name in names))
    return release


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--image-digest", required=True, help="sha256:<64 lowercase hex digits> of the tested multiarch index")
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        build(args.version, args.image_digest, args.source_sha, args.output_dir)
    except manager.Failure as error:
        parser.error(str(error))
