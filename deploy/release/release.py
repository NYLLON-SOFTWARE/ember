#!/usr/bin/env python3
"""Strict stable-release identity and monotonic publication checks."""
import argparse
import json
import re
import subprocess
import urllib.error
import urllib.request
from pathlib import Path

VERSION = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\Z')
SHA = re.compile(r'[0-9a-f]{40}\Z')
DIGEST = re.compile(r'sha256:[0-9a-f]{64}\Z')
REPOSITORY = 'NYLLON-SOFTWARE/ember'
IMAGE = 'ghcr.io/nyllon-software/ember'


def version(value):
    if not VERSION.fullmatch(value):
        raise ValueError('Use a stable MAJOR.MINOR.PATCH version without a v prefix')
    return tuple(map(int, value.split('.')))


def newer(candidate, releases):
    requested = version(candidate)
    for release in releases:
        tag = release['tag_name']
        # A draft reserves a version too. Prereleases never define stable ordering.
        if tag == 'v' + candidate:
            raise ValueError('This version is already reserved; never reuse release versions')
        if not release.get('prerelease') and VERSION.fullmatch(tag.removeprefix('v')):
            if requested <= version(tag.removeprefix('v')):
                raise ValueError('A stable release must be newer than every previous stable release')


def promotable(candidate, commit, releases):
    requested = version(candidate)
    if not SHA.fullmatch(commit):
        raise ValueError('Select an exact 40-character commit SHA')
    selected = None
    for release in releases:
        if release.get('draft') or release.get('prerelease'):
            continue
        tag = release['tag_name']
        if tag == 'v' + candidate:
            selected = release
        if VERSION.fullmatch(tag.removeprefix('v')) and requested < version(tag.removeprefix('v')):
            raise ValueError('A newer stable release already exists; refusing older publication')
    if selected is None or selected.get('target_commitish') != commit:
        raise ValueError('The published release does not match the selected source commit')


def releases():
    pages = json.loads(run('gh', 'api', '--paginate', '--slurp',
                           'repos/' + REPOSITORY + '/releases?per_page=100'))
    return [release for page in pages for release in page]


def run(*command):
    return subprocess.check_output(command, text=True).strip()


def require_public_repository(metadata):
    if metadata.get('private') is not False:
        raise ValueError('Stable release assets require a public repository for anonymous downloads')


def guard(requested, commit):
    version(requested)
    if not SHA.fullmatch(commit):
        raise ValueError('Select an exact 40-character commit SHA')
    require_public_repository(json.loads(run('gh', 'api', 'repos/' + REPOSITORY)))
    # Anonymous Git reads are intentional: release artifacts must also be anonymously readable.
    run('git', 'fetch', 'origin', 'main')
    run('git', 'merge-base', '--is-ancestor', commit, 'origin/main')
    if run('git', 'rev-parse', 'HEAD') != commit:
        raise ValueError('The checked out source does not match the selected commit')
    # Tags reserve versions even if a previous publication failed before its release.
    if run('git', 'ls-remote', '--tags', 'origin', 'refs/tags/v' + requested):
        raise ValueError('This version tag already exists')
    newer(requested, releases())



def unused_image_tags(requested, lookup):
    version(requested)
    for tag in ('v' + requested, requested):
        if lookup(tag):
            raise ValueError('This container version already exists; never overwrite a release image')


def registry_tag_exists(tag):
    token_request = urllib.request.Request('https://ghcr.io/token?service=ghcr.io&scope=repository:nyllon-software/ember:pull')
    with urllib.request.urlopen(token_request, timeout=30) as response:
        token = json.load(response)['token']
    request = urllib.request.Request('https://ghcr.io/v2/nyllon-software/ember/manifests/' + tag,
                                    method='HEAD', headers={
                                        'Authorization': 'Bearer ' + token,
                                        'Accept': 'application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json',
                                    })
    try:
        with urllib.request.urlopen(request, timeout=30):
            return True
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return False
        raise  # Authentication/network failures never mean a version is available.


def reserve(requested, commit):
    guard(requested, commit)
    unused_image_tags(requested, registry_tag_exists)
    # Reserve before any immutable image tags are written. If publication subsequently fails,
    # the Git tag makes reusing this identity fail closed even without a GitHub release.
    run('gh', 'api', '--method', 'POST', 'repos/' + REPOSITORY + '/git/refs',
        '-f', 'ref=refs/tags/v' + requested, '-f', 'sha=' + commit)


def publication(assets, output):
    manifest = json.loads((assets / 'release.json').read_text())
    version(manifest['version'])
    if not SHA.fullmatch(manifest['source_sha']):
        raise ValueError('Invalid release source SHA')
    if not manifest['image'].startswith(IMAGE + '@') or not DIGEST.fullmatch(manifest['image'].split('@')[-1]):
        raise ValueError('Invalid immutable image')
    bootstrap = (assets / 'bootstrap.sh').read_text()
    if manifest['bundle']['sha256'] not in bootstrap or manifest['bundle']['url'] not in bootstrap:
        raise ValueError('Bootstrap does not embed the tested bundle identity')
    guide = (Path(__file__).parent / 'guide.html').read_text().replace('{{VERSION}}', manifest['version'])
    output.write_text('// Generated only by the gated stable release.\nexport const publication = ' +
                      json.dumps(dict(bootstrap=bootstrap, manifest=manifest, guide=guide), ensure_ascii=True) + ';\n')


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='command', required=True)
    check = sub.add_parser('guard')
    check.add_argument('version')
    check.add_argument('commit')
    reservation = sub.add_parser('reserve')
    reservation.add_argument('version')
    reservation.add_argument('commit')
    promote = sub.add_parser('promotion')
    promote.add_argument('version')
    promote.add_argument('commit')
    build = sub.add_parser('worker')
    build.add_argument('assets', type=Path)
    build.add_argument('output', type=Path)
    args = parser.parse_args()
    if args.command == 'guard':
        guard(args.version, args.commit)
    elif args.command == 'reserve':
        reserve(args.version, args.commit)
    elif args.command == 'promotion':
        promotable(args.version, args.commit, releases())
    else:
        publication(args.assets, args.output)


if __name__ == '__main__':
    main()
