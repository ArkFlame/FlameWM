#!/usr/bin/env python3
"""Prepare a Cpak v3 *application* image from the existing native release stage.

This deliberately does not advertise a host-login session: cpak v3 cannot grant
FlameWM's direct logind system-bus calls to its native DRM backend.
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
DIGEST_IMAGE = re.compile(r'^[a-z0-9][a-z0-9._/-]*(?::[a-zA-Z0-9_.-]+)?@sha256:[a-f0-9]{64}$')


def make_manifest(image: str, version: str) -> dict:
    if not DIGEST_IMAGE.fullmatch(image):
        raise ValueError('OCI image must use an exact @sha256:<64 lowercase hex digits> digest')
    if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.+-]+)?', version):
        raise ValueError('invalid semantic version')
    return {
        '$schema': 'https://raw.githubusercontent.com/Containerpak/cpak/v2/schema/manifest-v3.json',
        'manifest_version': '3.0',
        'name': 'FlameWM Terminal',
        'description': 'FlameWM native terminal and Flamefetch, in an isolated X11 display',
        'version': version,
        'image': image,
        'binaries': list(BINARIES),
        'desktop_entries': ['/usr/share/applications/flamewm-terminal.desktop'],
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


def prepare(out: Path, source_payload: Path | None) -> None:
    if out.exists():
        raise ValueError(f'output already exists; choose a fresh directory: {out}')
    if source_payload is not None and not source_payload.is_dir():
        raise ValueError(f'payload directory missing: {source_payload}')
    out.mkdir(parents=True, exist_ok=False)
    payload = out / 'payload'
    try:
        if source_payload is None:
            subprocess.run([str(ROOT / 'scripts/package'), '--stage', str(payload)], cwd=ROOT, check=True)
        else:
            shutil.copytree(source_payload, payload)
        for binary in BINARIES:
            if not (payload / binary.lstrip('/')).is_file():
                raise ValueError(f'staged binary missing: {binary}')
        entry = payload / 'usr/share/applications/flamewm-terminal.desktop'
        if not entry.is_file():
            raise ValueError(f'staged desktop entry missing: {entry}')
        # Do not export a full host desktop session from an isolated application package.
        for relative in ['usr/share/xsessions', 'usr/share/wayland-sessions', 'etc/pam.d']:
            shutil.rmtree(payload / relative, ignore_errors=True)
        raw = entry.read_text(encoding='utf-8')
        if 'Exec=flamewm-terminal' not in raw:
            raise ValueError('unexpected terminal desktop entry; refusing to rewrite it')
        entry.write_text(raw.replace('Exec=flamewm-terminal', 'Exec=/usr/bin/flamewm-terminal')
                         .replace('TryExec=flamewm-terminal', 'TryExec=/usr/bin/flamewm-terminal'),
                         encoding='utf-8')
        shutil.copyfile(ROOT / 'packaging/cpak/Containerfile', out / 'Containerfile')
    except Exception:
        # The destination was required to be absent, so only our freshly created
        # build context is removed on failure. Never leave a half-built image input.
        shutil.rmtree(out)
        raise
    print(f'Prepared OCI build context: {out}')
    print(f'Build, test and push image with: podman build -f {out}/Containerfile -t <IMAGE:TAG> {out}')
    print('Only after pushing and obtaining its immutable digest, generate cpak.json with the manifest command.')


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    p = sub.add_parser('prepare', help='stage existing native release build for OCI image creation')
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--payload', type=Path, help='use a previously staged package payload')
    m = sub.add_parser('manifest', help='generate pinned Cpak v3 manifest after publishing image')
    m.add_argument('--image', required=True, help='e.g. ghcr.io/arkflame/flamewm-terminal@sha256:...')
    m.add_argument('--output', type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.action == 'prepare':
            prepare(args.output.resolve(), args.payload.resolve() if args.payload else None)
        else:
            if args.output.exists():
                raise ValueError(f'output exists; refusing to overwrite: {args.output}')
            version = subprocess.check_output([str(ROOT / 'scripts/version')], text=True).strip()
            manifest = make_manifest(args.image, version)
            args.output.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
            print(f'Wrote {args.output}; validate with: cpak validate {args.output}')
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'FLAMEWM-CPAK: {error}', file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
