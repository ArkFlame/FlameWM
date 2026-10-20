#!/usr/bin/env python3
"""Owned-Xvfb EWMH and held-pointer workspace-edge acceptance for built FlameWM.

Never attaches to the login display, changes host settings, or kills processes
it did not start. Exit 0=PASS, 1=behavior FAIL, 3=missing environment/binary.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def first_owned_display() -> int | None:
    for n in range(97, 120):
        if not Path(f"/tmp/.X11-unix/X{n}").exists() and not Path(f"/tmp/.X{n}-lock").exists():
            return n
    return None


def wait_until(predicate, *, seconds=3.0, interval=0.025):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        result = predicate()
        if result:
            return result
        time.sleep(interval)
    return None


def property_first(window, atom, X):
    prop = window.get_full_property(atom, X.AnyPropertyType)
    return int(prop.value[0]) if prop is not None and len(prop.value) else None


def held_titlebar_grip(pointer_x: int, pointer_y: int, frame) -> bool:
    """The grabbed pointer must stay inside the transferred window's titlebar.

    `frame` is the X11 frame geometry (root-relative for our reparented frame).
    The 48px upper strip is intentionally tolerant of different titlebar skins.
    """
    return (frame.x <= pointer_x < frame.x + frame.width
            and frame.y <= pointer_y < frame.y + min(48, frame.height))


def enough_frame_visible(frame, root_width: int) -> bool:
    """A crossing must not strand almost the entire frame outside the root."""
    visible = max(0, min(root_width, frame.x + frame.width) - max(0, frame.x))
    return visible >= min(root_width, frame.width) // 2


def run(wm_bin: Path, work_dir: Path) -> dict:
    try:
        from Xlib import X, display, protocol
        from Xlib.ext import xtest
    except ImportError as error:
        raise EnvironmentError(f"python-xlib / XTEST unavailable: {error}") from error
    if not wm_bin.is_file() or not os.access(wm_bin, os.X_OK):
        raise EnvironmentError(f"built WM binary is missing or not executable: {wm_bin}")
    if not Path('/usr/bin/Xvfb').exists() and not shutil.which('Xvfb'):
        raise EnvironmentError('Xvfb unavailable')
    n = first_owned_display()
    if n is None:
        raise EnvironmentError('no unowned display in :97..:119')
    display_name = f":{n}"
    root_home = work_dir / 'home'
    xdg_run = work_dir / 'runtime'
    root_home.mkdir(parents=True, exist_ok=True)
    xdg_run.mkdir(parents=True, exist_ok=True, mode=0o700)
    xdg_run.chmod(0o700)
    env = os.environ.copy()
    env.update({
        'DISPLAY': display_name,
        'HOME': str(root_home),
        'XDG_CONFIG_HOME': str(root_home / '.config'),
        'XDG_CACHE_HOME': str(root_home / '.cache'),
        'XDG_DATA_HOME': str(root_home / '.local/share'),
        'XDG_RUNTIME_DIR': str(xdg_run),
        'FLAMEWM_WORKSPACES': '6',
        'XDG_SESSION_TYPE': 'x11',
    })
    env.pop('FLAMEWM_CONFIG_DIR', None)
    dpy = None
    xvfb = wm = None
    evidence = {'display': display_name, 'assertions': []}
    xlog = (work_dir / 'xvfb.log').open('wb')
    wlog = (work_dir / 'wm.log').open('wb')
    try:
        xvfb = subprocess.Popen(['Xvfb', display_name, '-screen', '0', '1280x800x24', '-nolisten', 'tcp'],
                                stdout=xlog, stderr=subprocess.STDOUT, env=env)
        if not wait_until(lambda: Path(f'/tmp/.X11-unix/X{n}').exists() or xvfb.poll() is not None):
            raise EnvironmentError('Xvfb did not start')
        if xvfb.poll() is not None:
            raise EnvironmentError(f'Xvfb exited {xvfb.returncode}')
        wm = subprocess.Popen([str(wm_bin)], stdout=wlog, stderr=subprocess.STDOUT, env=env)
        dpy = display.Display(display_name)
        screen = dpy.screen()
        root = screen.root
        current_atom = dpy.intern_atom('_NET_CURRENT_DESKTOP')
        window_atom = dpy.intern_atom('_NET_WM_DESKTOP')
        count_atom = dpy.intern_atom('_NET_NUMBER_OF_DESKTOPS')
        if not wait_until(lambda: property_first(root, count_atom, X) == 6 or wm.poll() is not None):
            raise AssertionError('WM did not publish 6 workspaces')
        if wm.poll() is not None:
            raise AssertionError(f'WM exited {wm.returncode}; see wm.log')
        assert property_first(root, count_atom, X) == 6
        evidence['assertions'].append('six workspaces published')

        def create_client(name, x, y):
            client = root.create_window(x, y, 380, 240, 0, screen.root_depth,
                                        X.InputOutput, X.CopyFromParent,
                                        background_pixel=screen.white_pixel,
                                        event_mask=X.StructureNotifyMask | X.PropertyChangeMask)
            client.set_wm_name(name)
            client.set_wm_class(name.lower(), 'FlameWMEdgeFixture')
            client.map()
            dpy.flush()
            if not wait_until(lambda: client.query_tree().parent.id != root.id):
                raise AssertionError(f'{name} was not reparented')
            return client, client.query_tree().parent

        a, frame = create_client('FlameWM-Workspace-Drag-A', 220, 185)
        b, b_frame = create_client('FlameWM-Workspace-Drag-B', 700, 195)
        if not wait_until(lambda: property_first(a, window_atom, X) == 0):
            raise AssertionError('first client lacks workspace property')

        def send_desktop(window, target, atom):
            root.send_event(protocol.event.ClientMessage(window=window, client_type=atom,
                                                         data=(32, [target, 0, 0, 0, 0])),
                            event_mask=X.SubstructureRedirectMask | X.SubstructureNotifyMask)
            dpy.flush()

        active_atom = dpy.intern_atom('_NET_ACTIVE_WINDOW')
        send_desktop(b, 1, active_atom)  # EWMH source-indication 1 = application
        if not wait_until(lambda: property_first(root, active_atom, X) == b.id):
            raise AssertionError('cannot focus second client before moving it')
        send_desktop(b, 2, window_atom)
        if not wait_until(lambda: property_first(b, window_atom, X) == 2):
            raise AssertionError('cannot move second client to workspace 2')
        evidence['assertions'].append('second client assigned to workspace 2')
        if not wait_until(lambda: property_first(root, active_atom, X) != b.id):
            raise AssertionError('moved-away window is still _NET_ACTIVE_WINDOW')
        evidence['assertions'].append('moving active window away clears stale EWMH focus')
        if not wait_until(lambda: b_frame.get_attributes().map_state != X.IsViewable):
            raise AssertionError('workspace-2 client remains visible on workspace 0')
        evidence['assertions'].append('other workspace window is unmapped')
        # Moving between two *inactive* workspaces must never map the frame.
        # Repeated redundant XUnmapWindow requests previously accumulated
        # ignore_unmap counters without a corresponding server notification.
        for destination in (1, 2, 1, 2):
            send_desktop(b, destination, window_atom)
            if not wait_until(lambda d=destination: property_first(b, window_atom, X) == d):
                raise AssertionError(f'hidden client failed move to workspace {destination}')
            if b_frame.get_attributes().map_state == X.IsViewable:
                raise AssertionError(f'hidden client became visible when moved to {destination}')
        evidence['assertions'].append('hidden→hidden EWMH transfers never remap the window')

        # Merely hovering at an edge must never switch a workspace. A held
        # titlebar move is the only eligible gesture.
        xtest.fake_input(dpy, X.MotionNotify, x=1279, y=220)
        dpy.sync()
        time.sleep(0.28)
        if property_first(root, current_atom, X) != 0:
            raise AssertionError('idle pointer at edge changed workspace')
        evidence['assertions'].append('idle edge hover does not switch')

        geom = frame.get_geometry()
        # The titlebar is above the reparented client. The test uses an x well
        # away from close/minimize controls and the resize edges.
        title_x = geom.x + min(145, max(65, geom.width // 3))
        title_y = geom.y + 12
        xtest.fake_input(dpy, X.MotionNotify, x=title_x, y=title_y)
        dpy.sync()
        xtest.fake_input(dpy, X.ButtonPress, detail=1)
        dpy.sync()
        try:
            xtest.fake_input(dpy, X.MotionNotify, x=title_x + 70, y=title_y)
            dpy.sync()
            if not wait_until(lambda: abs(frame.get_geometry().x - geom.x) >= 10):
                raise AssertionError('titlebar drag did not activate')
            # Six-workspace upper row has exactly three columns, so repeated
            # right-edge crossings test row-local wrap WITHOUT releasing.
            for expected in (1, 2, 0):
                xtest.fake_input(dpy, X.MotionNotify, x=1279, y=title_y)
                dpy.sync()
                time.sleep(0.54)  # initial 220ms dwell; after first crossing wait for 500ms axis cooldown
                if not wait_until(lambda: property_first(root, current_atom, X) == expected
                                  and property_first(a, window_atom, X) == expected, seconds=2):
                    raise AssertionError(f'held drag did not transfer into workspace {expected}')
                pointer = root.query_pointer()
                if not 2 <= pointer.root_x <= 95 or abs(pointer.root_y - title_y) > 2:
                    raise AssertionError(f'pointer warp wrong after transfer {expected}: '
                                         f'({pointer.root_x},{pointer.root_y})')
                if not wait_until(lambda: held_titlebar_grip(
                        root.query_pointer().root_x, root.query_pointer().root_y,
                        frame.get_geometry())):
                    raise AssertionError(f'dragging window detached from pointer in workspace {expected}')
                if not enough_frame_visible(frame.get_geometry(), 1280):
                    raise AssertionError(f'dragged frame mostly offscreen after transfer {expected}')
                if not wait_until(lambda: frame.get_attributes().map_state == X.IsViewable
                                  and (b_frame.get_attributes().map_state == X.IsViewable) == (expected == 2)):
                    raise AssertionError(f'workspace {expected} has incorrect frame visibility')
                evidence['assertions'].append(
                    f'workspace {expected}: cursor attached, frame visible, other windows isolated')
            # Also wrap backwards: first column -> last column of SAME row.
            xtest.fake_input(dpy, X.MotionNotify, x=0, y=title_y)
            dpy.sync()
            time.sleep(0.34)
            if not wait_until(lambda: property_first(root, current_atom, X) == 2
                              and property_first(a, window_atom, X) == 2, seconds=2):
                raise AssertionError('held left-edge drag did not wrap workspace 0 to 2')
            pointer = root.query_pointer()
            if not 1180 <= pointer.root_x <= 1270 or abs(pointer.root_y - title_y) > 2:
                raise AssertionError('left-edge crossing did not warp pointer to the right')
            if not held_titlebar_grip(pointer.root_x, pointer.root_y, frame.get_geometry()):
                raise AssertionError('dragged frame detached from pointer after left-edge wrap')
            if not enough_frame_visible(frame.get_geometry(), 1280):
                raise AssertionError('dragged frame mostly offscreen after left-edge wrap')
            evidence['assertions'].append('held left-edge wrap 0 → 2, grip retained')

            # Six desktops form 1 2 3 / 4 5 6. At index 2, the top
            # edge switches to index 5 (same column, lower row). The fresh
            # vertical lane is NOT blocked by the previous horizontal 500ms
            # cooldown. Up/down share a second cooldown with each other.
            before_vertical = time.monotonic()
            top_x = root.query_pointer().root_x
            xtest.fake_input(dpy, X.MotionNotify, x=top_x, y=0)
            dpy.sync()
            if not wait_until(lambda: property_first(root, current_atom, X) == 5
                              and property_first(a, window_atom, X) == 5, seconds=0.44):
                raise AssertionError('top edge did not switch workspace 2 to 5 independently of horizontal cooldown')
            pointer = root.query_pointer()
            if abs(pointer.root_x - top_x) > 2 or not 48 <= pointer.root_y < 800:
                raise AssertionError('top-edge crossing lost X or warped offscreen')
            if not held_titlebar_grip(pointer.root_x, pointer.root_y, frame.get_geometry()):
                raise AssertionError('top-edge crossing lost titlebar grip')
            if not enough_frame_visible(frame.get_geometry(), 1280):
                raise AssertionError('top-edge crossing moved frame mostly offscreen horizontally')
            evidence['assertions'].append('top edge 2 → 5, independent axis cooldown, titlebar grip kept')
            # Arm the opposite vertical edge while its own cooldown is active.
            # It must not fire at 220ms, but must fire after >=500ms total.
            xtest.fake_input(dpy, X.MotionNotify, x=top_x, y=799)
            dpy.sync()
            time.sleep(0.27)
            if property_first(root, current_atom, X) != 5:
                raise AssertionError('bottom edge bypassed vertical cooldown')
            if not wait_until(lambda: property_first(root, current_atom, X) == 2
                              and property_first(a, window_atom, X) == 2, seconds=0.65):
                raise AssertionError('bottom edge did not switch workspace 5 to 2 after cooldown')
            pointer = root.query_pointer()
            if abs(pointer.root_x - top_x) > 2 or not 2 <= pointer.root_y <= 95:
                raise AssertionError('bottom-edge pointer did not wrap to top inset')
            if not held_titlebar_grip(pointer.root_x, pointer.root_y, frame.get_geometry()):
                raise AssertionError('bottom-edge crossing lost titlebar grip')
            evidence['assertions'].append('bottom edge 5 → 2, vertical 500ms cooldown obeyed')
            prior_x = frame.get_geometry().x
            xtest.fake_input(dpy, X.MotionNotify, x=1090, y=title_y)
            dpy.sync()
            if not wait_until(lambda: frame.get_geometry().x != prior_x):
                raise AssertionError('window does not follow cursor after workspace crossings')
            evidence['assertions'].append('window continues following cursor after horizontal and vertical transfers')
        finally:
            xtest.fake_input(dpy, X.ButtonRelease, detail=1)
            dpy.sync()
        # Normal workspace switch: only clients in the current workspace map.
        send_desktop(root, 0, current_atom)
        if not wait_until(lambda: property_first(root, current_atom, X) == 0):
            raise AssertionError('direct switch to workspace 0 failed')
        if not wait_until(lambda: frame.get_attributes().map_state != X.IsViewable
                          and b_frame.get_attributes().map_state != X.IsViewable):
            raise AssertionError('workspace-2 clients remain visible on workspace 0')
        send_desktop(root, 2, current_atom)
        if not wait_until(lambda: property_first(root, current_atom, X) == 2
                          and frame.get_attributes().map_state == X.IsViewable
                          and b_frame.get_attributes().map_state == X.IsViewable):
            raise AssertionError('workspace-2 clients did not reappear after normal switch')
        if property_first(a, window_atom, X) != 2:
            raise AssertionError('dragged client membership changed after release')
        evidence['assertions'].append('both workspace-2 windows map only on workspace 2')
        return evidence
    finally:
        if dpy is not None:
            try: dpy.close()
            except Exception: pass
        for process in (wm, xvfb):
            if process is None: continue
            if process.poll() is None:
                process.terminate()
                try: process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)
        xlog.close()
        wlog.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wm-bin', type=Path, required=True, help='fresh compiled FlameWM WM executable')
    parser.add_argument('--work-dir', type=Path, help='empty directory for logs; default temporary')
    args = parser.parse_args()
    if args.work_dir:
        path = args.work_dir.resolve()
        if path.exists() and any(path.iterdir()):
            parser.error('--work-dir must be new or empty')
        path.mkdir(parents=True, exist_ok=True)
        try:
            result = run(args.wm_bin.resolve(), path)
        except EnvironmentError as error:
            print(json.dumps({'status': 'BLOCKED', 'reason': str(error), 'logs': str(path)}))
            return 3
        except Exception as error:
            print(json.dumps({'status': 'FAIL', 'reason': str(error), 'logs': str(path)}))
            return 1
    else:
        with tempfile.TemporaryDirectory(prefix='flamewm-workspace-edge-') as temporary:
            try:
                result = run(args.wm_bin.resolve(), Path(temporary))
            except EnvironmentError as error:
                print(json.dumps({'status': 'BLOCKED', 'reason': str(error)}))
                return 3
            except Exception as error:
                print(json.dumps({'status': 'FAIL', 'reason': str(error)}))
                return 1
    print(json.dumps({'status': 'PASS', **result}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
