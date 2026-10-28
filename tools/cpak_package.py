#!/usr/bin/env python3
"""Cpak v3 application packaging for FlameWM Terminal and Flamefetch.

`prune` turns a `scripts/package --stage` tree into the application-only image
payload; `manifest` writes the digest-pinned cpak.json for a pushed image.
"""
from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BINARIES = ('/usr/bin/flamefetch', '/usr/bin/flamewm-terminal')
DESKTOP_ENTRY = '/usr/share/applications/flamewm-terminal.desktop'
HOST_SESSION_PATHS = ('usr/share/xsessions', 'usr/share/wayland-sessions', 'etc/pam.d')
DIGEST_IMAGE = re.compile(r'^[a-z0-9][a-z0-9._/-]*(?::[a-zA-Z0-9_.-]+)?@sha256:[a-f0-9]{64}$')
VERSION = re.compile(r'[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.+-]+)?')


def make_manifest(image: str, version: str) -> dict:
    if not DIGEST_IMAGE.fullmatch(image):
        raise ValueError('OCI image must use an exact @sha256:<64 lowercase hex digits> digest')
    if not VERSION.fullmatch(version):
        raise ValueError('invalid semantic version')
    return {
        '$schema': 'https://raw.githubusercontent.com/Containerpak/cpak/v2/schema/manifest-v3.json',
        'manifest_version': '3.0',
        'name': 'FlameWM Terminal',
        'description': 'FlameWM native terminal and Flamefetch, in an isolated X11 display',
        'version': version,
        'image': image,
        'binaries': list(BINARIES),
        'desktop_entries': [DESKTOP_ENTRY],
        'form_factors': ['desktop'],
        'dependencies': [],
        'addons': [],
        'idle_time': 0,
        'override': {
            'displayX11': True,
            'clipboard': {'hostToApp': True, 'appToHost': True},
            'network': False,
            'process': False,
            'hostApplications': False,
            'deviceDri': False,
            'filesystem': [],
        },
    }


def prune(payload: Path) -> None:
    if not payload.is_dir():
        raise ValueError(f'payload directory missing: {payload}')
    for binary in BINARIES:
        if not (payload / binary.lstrip('/')).is_file():
            raise ValueError(f'staged binary missing: {binary}')
    entry = payload / DESKTOP_ENTRY.lstrip('/')
    if not entry.is_file():
        raise ValueError(f'staged desktop entry missing: {entry}')
    raw = entry.read_text(encoding='utf-8')
    if 'Exec=flamewm-terminal' not in raw and 'Exec=/usr/bin/flamewm-terminal' not in raw:
        raise ValueError('unexpected terminal desktop entry; refusing to rewrite it')
    for relative in HOST_SESSION_PATHS:
        shutil.rmtree(payload / relative, ignore_errors=True)
    entry.write_text(re.sub(r'(?m)^(Try)?Exec=flamewm-terminal', r'\1Exec=/usr/bin/flamewm-terminal', raw),
                     encoding='utf-8')


def write_manifest(image: str, output: Path) -> None:
    if output.exists():
        raise ValueError(f'output exists; refusing to overwrite: {output}')
    version = subprocess.check_output([str(ROOT / 'scripts/version')], text=True).strip()
    manifest = make_manifest(image, version)
    output.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    p = sub.add_parser('prune', help='reduce a staged package tree to the application-only cpak payload')
    p.add_argument('payload', type=Path)
    m = sub.add_parser('manifest', help='write the digest-pinned Cpak v3 manifest of a pushed image')
    m.add_argument('--image', required=True, help='e.g. ghcr.io/arkflame/flamewm-terminal@sha256:...')
    m.add_argument('--output', type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.action == 'prune':
            prune(args.payload.resolve())
            print(f'Pruned cpak payload: {args.payload}')
        else:
            write_manifest(args.image, args.output)
            print(f'Wrote {args.output}; validate with: cpak validate {args.output}')
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'FLAMEWM-CPAK: {error}', file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
