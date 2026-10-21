#!/usr/bin/env python3
"""Read-only KDE MIME and desktop notification integration probe.

Use --refresh to rebuild the current user's KDE application metadata cache;
use --send-test to deliberately emit a noninteractive sample toast.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

from verify_flamewm_open_with import application_dirs, audit, menu_candidates
from flamewm_desktop_services import notification_owned


def check(env: dict[str, str], *, refresh: bool = False, send_test: bool = False) -> int:
    mime_ready = audit(env, refresh=refresh)
    print('KDE menu:', next((str(p) for p in menu_candidates(env) if p.is_file()), '<missing>'))
    print('KService application directories:', ', '.join(str(d) for d in application_dirs(env)))
    print('KService builder:', shutil.which('kbuildsycoca6') or shutil.which('kbuildsycoca5') or '<missing>')
    for mime in ('text/plain', 'application/pdf', 'image/png', 'video/mp4'):
        xdg = shutil.which('xdg-mime')
        if not xdg:
            print(f'Default {mime}: xdg-mime unavailable')
            continue
        try:
            result = subprocess.run([xdg, 'query', 'default', mime], capture_output=True,
                                    text=True, timeout=3, env=env, check=False)
            value = result.stdout.strip() if result.returncode == 0 else '<query failed>'
        except (OSError, subprocess.TimeoutExpired):
            value = '<unavailable>'
        print(f'Default {mime}: {value or "<unset>"}')
    notifications = notification_owned()
    print('org.freedesktop.Notifications owned:', notifications)
    print('KDE KIO JobViewServer present:', _has_bus_name('org.kde.JobViewServer'))
    if send_test:
        if not notifications:
            print('Cannot send test: notification server is not running', file=sys.stderr)
        else:
            sender = shutil.which('notify-send')
            if sender:
                try:
                    status = subprocess.run([sender, '-t', '3500', '-h', 'int:value:55',
                                             'FlameWM notifications', 'Toast and progress hint test'],
                                            env=env, timeout=5, check=False).returncode
                    print('Notification test command returned:', status)
                except (OSError, subprocess.TimeoutExpired):
                    print('Notification test command could not complete', file=sys.stderr)
            else:
                print('notify-send not installed; install libnotify-bin to test toast presentation')
    return 0 if mime_ready and notifications else 1


def _has_bus_name(name: str) -> bool:
    tool = shutil.which('busctl')
    if not tool:
        return False
    try:
        result = subprocess.run([tool, '--user', '--no-pager', '--no-legend', 'list'],
                                capture_output=True, text=True, timeout=3, check=False)
        return result.returncode == 0 and any(line.split()[:1] == [name]
                                               for line in result.stdout.splitlines())
    except (OSError, subprocess.TimeoutExpired):
        return False


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--refresh', action='store_true', help='rebuild KDE application cache for current user')
    parser.add_argument('--send-test', action='store_true', help='send a visible test notification')
    options = parser.parse_args()
    sys.exit(check(dict(os.environ), refresh=options.refresh, send_test=options.send_test))
