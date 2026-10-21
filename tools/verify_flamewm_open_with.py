#!/usr/bin/env python3
"""Check the actual FlameWM/KService application-menu contract in this session.

Read-only unless --refresh is given, in which case KDE rebuilds the caller's
own KSycoca cache. Never edits user MIME choices, global menus or KDE files.
"""
from __future__ import annotations
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys


def menu_candidates(env: dict[str, str]) -> list[Path]:
    home = Path(env.get('HOME') or str(Path.home()))
    config_home = Path(env.get('XDG_CONFIG_HOME') or home / '.config')
    config_dirs = (env.get('XDG_CONFIG_DIRS') or '/etc/xdg').split(':')
    basename = f"{env.get('XDG_MENU_PREFIX', '')}applications.menu"
    return [p / 'menus' / basename for p in [config_home, *(Path(p) for p in config_dirs if p)]]


def application_dirs(env: dict[str, str]) -> list[Path]:
    home = Path(env.get('HOME') or str(Path.home()))
    data_home = Path(env.get('XDG_DATA_HOME') or home / '.local/share')
    system = (env.get('XDG_DATA_DIRS') or '/usr/local/share:/usr/share').split(':')
    return [p / 'applications' for p in [data_home, *(Path(p) for p in system if p)]]


def audit(env: dict[str, str], refresh: bool = False) -> bool:
    print('Desktop:', env.get('XDG_CURRENT_DESKTOP', '<unset>'))
    print('Menu prefix:', env.get('XDG_MENU_PREFIX', '<unset>'))
    matches = [p for p in menu_candidates(env) if p.is_file()]
    if not matches:
        print('ERROR: no applications.menu found in effective XDG_CONFIG_DIRS', file=sys.stderr)
        return False
    menu = matches[0]
    print('Active menu:', menu)
    if env.get('XDG_MENU_PREFIX') != 'flamewm-':
        print('ERROR: FlameWM session is not exporting its menu prefix', file=sys.stderr)
        return False
    if 'FlameWM' not in env.get('XDG_CURRENT_DESKTOP', '').split(':'):
        print('ERROR: not running in a FlameWM session', file=sys.stderr)
        return False
    content = menu.read_text(encoding='utf-8')
    if '<DefaultAppDirs/>' not in content:
        print('ERROR: menu does not scan XDG application directories', file=sys.stderr)
        return False
    apps = sum(len(list(d.glob('*.desktop'))) for d in application_dirs(env) if d.is_dir())
    print('Installed desktop entries in standard locations:', apps)
    if not apps:
        print('ERROR: no application desktop entries are discoverable', file=sys.stderr)
        return False
    if refresh:
        builder = shutil.which('kbuildsycoca6') or shutil.which('kbuildsycoca5')
        if not builder:
            print('ERROR: kbuildsycoca not installed; KDE application cache cannot be checked', file=sys.stderr)
            return False
        completed = subprocess.run([builder, '--noincremental'], env=env, timeout=40, check=False)
        if completed.returncode:
            print(f'ERROR: {builder} exited {completed.returncode}', file=sys.stderr)
            return False
        print('KSycoca rebuilt successfully')
    return True


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--refresh', action='store_true', help='request an explicit KDE KSycoca rebuild')
    args = parser.parse_args()
    sys.exit(0 if audit(dict(os.environ), args.refresh) else 1)
