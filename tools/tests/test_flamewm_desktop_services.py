from __future__ import annotations

import importlib.util
from importlib.machinery import SourceFileLoader
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
MODULE = ROOT / 'tools' / 'flamewm_desktop_services.py'
HOOK = ROOT / 'scripts' / 'enable-post31-desktop-integration'


def load(path, name):
    spec = importlib.util.spec_from_loader(name, SourceFileLoader(name, str(path)))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

services = load(MODULE, 'services')
integration = load(HOOK, 'integration')


class Services(unittest.TestCase):
    def setUp(self):
        self.env = {'XDG_CURRENT_DESKTOP': 'FlameWM', 'PATH': '/usr/bin'}
        self.find = lambda program: f'/test/bin/{program}' if program in ('dex', 'lxqt-policykit-agent', 'xfce4-notifyd') else None
        self.choose = lambda **kwargs: services.choose_programs(
            self.env, find=self.find, process_active=lambda names: False,
            notifier_available=lambda: False, notifier_owned=lambda: False, **kwargs)

    def test_only_flamewm_sessions(self):
        self.env['XDG_CURRENT_DESKTOP'] = 'KDE'
        self.assertEqual(self.choose(), dict.fromkeys(['autostart', 'polkit', 'notifications', 'clipboard', 'bluetooth']))
        self.env['XDG_CURRENT_DESKTOP'] = 'FlameWM:XFCE'
        self.assertTrue(services.is_flamewm(self.env))
        self.env['XDG_CURRENT_DESKTOP'] = 'KDE:GNOME'
        self.assertFalse(services.is_flamewm(self.env))

    def test_no_shell_or_global_kde_impersonation(self):
        selected = self.choose()
        self.assertEqual(selected['autostart'], ['/test/bin/dex', '--autostart', '--environment', 'FlameWM'])
        self.assertEqual(selected['polkit'], ['/test/bin/lxqt-policykit-agent'])
        self.assertEqual(selected['notifications'], ['/test/bin/xfce4-notifyd'])
        self.assertEqual(self.env['XDG_CURRENT_DESKTOP'], 'FlameWM')

    def test_uses_existing_polkit_agent(self):
        plan = services.choose_programs(self.env, self.find, lambda names: 'lxqt-policykit-agent' in names, lambda: False, lambda: False)
        self.assertIsNone(plan['polkit'])

    def test_uses_existing_notification_provider(self):
        for check in ((lambda: True, lambda: False), (lambda: False, lambda: True)):
            plan = services.choose_programs(self.env, self.find, lambda _: False, *check)
            self.assertIsNone(plan['notifications'])

    def test_optional_dependencies_fail_open_without_spawning(self):
        plan = services.choose_programs(self.env, lambda _: None, lambda _: False, lambda: False, lambda: False)
        self.assertEqual(plan, dict.fromkeys(['autostart', 'polkit', 'notifications', 'clipboard', 'bluetooth']))

    def test_user_dbus_notification_activation_directory(self):
        with tempfile.TemporaryDirectory() as folder:
            home = Path(folder)
            service_dir = home / '.local/share/dbus-1/services'
            service_dir.mkdir(parents=True)
            service = service_dir / 'custom-notifications.service'
            service.write_text('[D-BUS Service]\nName=org.freedesktop.Notifications\nExec=/usr/bin/example\n')
            roots = services.session_service_roots({'HOME': str(home), 'XDG_DATA_DIRS': '/unusual/share'})
            self.assertEqual(roots[0], str(service_dir))
            self.assertTrue(services.notification_activatable(roots))
            self.assertFalse(services.notification_activatable(('/empty/not-a-real-path',)))

    def test_autostart_receives_native_session_overlays_without_kde_impersonation(self):
        with tempfile.TemporaryDirectory() as folder:
            config = Path(folder) / 'custom-config'
            compat = config / 'flamewm/session-compat'
            for directory in ('config', 'data'):
                (compat / directory).mkdir(parents=True)
            env = dict(self.env, XDG_CONFIG_HOME=str(config),
                       XDG_CONFIG_DIRS='/etc/xdg', XDG_DATA_DIRS='/usr/share')
            planned = services.flamewm_autostart_environment(env)
            self.assertEqual(planned['XDG_CURRENT_DESKTOP'], 'FlameWM')
            self.assertEqual(planned['XDG_CONFIG_DIRS'], str(compat / 'config') + ':/etc/xdg')
            self.assertEqual(planned['XDG_DATA_DIRS'], str(compat / 'data') + ':/usr/share')
            self.assertEqual(services.flamewm_autostart_environment(planned), planned)
            self.assertNotIn('KDE_SESSION_VERSION', planned)
            self.assertEqual(env['XDG_CONFIG_DIRS'], '/etc/xdg')

    def test_autostart_precedes_provider_decision_and_avoids_duplicate_agents(self):
        with tempfile.TemporaryDirectory() as folder:
            env = dict(self.env, XDG_RUNTIME_DIR=folder)
            file_descriptor = os.open(os.devnull, os.O_RDONLY)
            fake_dex = mock.Mock()
            fake_dex.wait.return_value = 0
            first = {'autostart': ['/bin/dex', '--autostart'], 'polkit': ['/bin/polkit']}
            second = {'autostart': ['/bin/dex', '--autostart'], 'polkit': None,
                      'notifications': None, 'clipboard': None, 'bluetooth': None}
            signals = []
            class NoWait:
                def is_set(self): return False
                def wait(self, timeout=None): return False
            def spawn(role, argv, environment):
                signals.append(role)
                return fake_dex
            with mock.patch.object(services, 'wait_for_wm', return_value=True), \
                 mock.patch.object(services, 'session_lock', return_value=file_descriptor), \
                 mock.patch.object(services, 'flamewm_autostart_environment', side_effect=lambda v: v), \
                 mock.patch.object(services, 'choose_programs', side_effect=[first, second]), \
                 mock.patch.object(services, 'launch_program', side_effect=spawn):
                self.assertEqual(services.run_services(env, NoWait()), 0)
            self.assertEqual(signals, ['autostart'])

    def test_dbus_activation_service_presence(self):
        with tempfile.TemporaryDirectory() as folder:
            self.assertFalse(services.notification_activatable((folder,)))
            (Path(folder) / 'org.freedesktop.Notifications.service').touch()
            self.assertTrue(services.notification_activatable((folder,)))

    def test_lifecycle_lock_requires_user_owned_runtime(self):
        with self.assertRaises(RuntimeError):
            services.session_lock({})
        with tempfile.TemporaryDirectory() as folder:
            first = services.session_lock({'XDG_RUNTIME_DIR': folder})
            try:
                second = services.session_lock({'XDG_RUNTIME_DIR': folder})
                self.assertIsNone(second)
            finally:
                os.close(first)

    def test_no_secret_inspection_in_diagnostics(self):
        result = services.doctor({'XDG_CURRENT_DESKTOP': 'KDE'})
        self.assertFalse(result['flamewm_session'])
        self.assertNotIn('wallet_password', result)
        self.assertNotIn('cookie', result)
        self.assertEqual(result['security_note'], 'Does not inspect wallet secrets or browser profiles')

    def test_wm_readiness_never_claims_success_when_no_xprop(self):
        with mock.patch.object(services.shutil, 'which', return_value=None):
            self.assertFalse(services.wait_for_wm(self.env, threading.Event(), timeout=0))

    def test_no_xdg_start_before_wm_ready(self):
        with tempfile.TemporaryDirectory() as folder:
            env = dict(self.env, XDG_RUNTIME_DIR=folder)
            with mock.patch.object(services, 'wait_for_wm', return_value=False), mock.patch.object(services.subprocess, 'Popen') as launch:
                self.assertEqual(services.run_services(env, threading.Event()), 1)
                launch.assert_not_called()

    def test_plan_has_no_startup_side_effect(self):
        result = subprocess.run([sys.executable, str(MODULE), '--plan'], env=dict(os.environ, XDG_CURRENT_DESKTOP='KDE'), capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn('"notifications": null', result.stdout)


    def test_clipboard_history_is_opt_in(self):
        self.assertIsNone(self.choose()['clipboard'])
        self.env['FLAMEWM_ENABLE_CLIPBOARD_HISTORY'] = '1'
        self.find = lambda name: '/mock/xfce4-clipman' if name == 'xfce4-clipman' else None
        self.assertEqual(self.choose()['clipboard'], ['/mock/xfce4-clipman'])

    def test_bluetooth_applet_is_opt_in(self):
        self.assertIsNone(self.choose()['bluetooth'])
        self.env['FLAMEWM_ENABLE_BLUETOOTH_APPLET'] = '1'
        self.find = lambda name: '/mock/blueman-applet' if name == 'blueman-applet' else None
        self.assertEqual(self.choose()['bluetooth'], ['/mock/blueman-applet'])


    def test_wrapper_owns_bridge_lifecycle_and_keeps_session_exit(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            session = root / 'session'
            bridge = root / 'bridge'
            gone = root / 'bridge-stopped'
            session.write_text('#!/bin/sh\nsleep 0.25\nexit 7\n')
            bridge.write_text('#!/bin/sh\ntrap "touch \"$STOP_FLAG\"; exit 0" TERM\nwhile :; do sleep 0.05; done\n')
            session.chmod(0o755)
            bridge.chmod(0o755)
            env = dict(os.environ, XDG_CURRENT_DESKTOP='FlameWM',
                       FLAMEWM_SESSION_BINARY=str(session),
                       FLAMEWM_SERVICES_BINARY=str(bridge), STOP_FLAG=str(gone))
            result = subprocess.run(['bash', str(ROOT / 'scripts/flamewm-session-wrapper')], env=env, timeout=5)
            self.assertEqual(result.returncode, 7)
            self.assertTrue(gone.is_file())


class Integration(unittest.TestCase):
    def fixture(self, folder):
        root = Path(folder)
        for item in integration.FILES:
            target = root / item
            target.parent.mkdir(parents=True, exist_ok=True)
            if item == integration.SESSION:
                target.write_text('[Desktop Entry]\nExec=/usr/bin/flamewm-session\n')
            elif item == integration.PACKAGE:
                target.write_text('install -m 0644 "$ROOT/packaging/flamewm-session.desktop" "$STAGE/usr/share/xsessions/flamewm.desktop"\n')
            elif item == integration.CONTROL:
                target.write_text('Depends: libpam0g\nRecommends: kwallet6, libpam-kwallet5\n')
            else:
                target.write_text('base\n')
        (root / 'Cargo.toml').write_text('version = "0.1.0"\n')
        return root

    def test_dry_run_does_not_edit_files(self):
        with tempfile.TemporaryDirectory() as folder:
            root = self.fixture(folder)
            before = (root / integration.SESSION).read_text()
            names = integration.apply(root, dry_run=True)
            self.assertEqual(len(names), 5)
            self.assertEqual((root / integration.SESSION).read_text(), before)

    def test_single_apply_and_idempotent_repeat(self):
        with tempfile.TemporaryDirectory() as folder:
            root = self.fixture(folder)
            changed = integration.apply(root)
            self.assertEqual(len(changed), 5)
            self.assertEqual(integration.apply(root), [])
            self.assertIn('flamewm-session-wrapper', (root / integration.SESSION).read_text())
            self.assertIn('flamewm-desktop-services', (root / integration.PACKAGE).read_text())
            control = (root / integration.CONTROL).read_text()
            self.assertIn('Depends: libpam0g, python3', control)
            self.assertNotIn('Recommends:', control)

    def test_reject_unrecognized_agent3_session_hook_without_mutating(self):
        with tempfile.TemporaryDirectory() as folder:
            root = self.fixture(folder)
            (root / integration.SESSION).write_text('Exec=/usr/bin/custom-agent-session\n')
            before = (root / integration.CONTROL).read_bytes()
            with self.assertRaises(ValueError):
                integration.apply(root)
            self.assertEqual((root / integration.CONTROL).read_bytes(), before)

    def test_reject_manifest_missing_without_partial_mutation(self):
        with tempfile.TemporaryDirectory() as folder:
            root = self.fixture(folder)
            (root / integration.MANIFEST).unlink()
            before = (root / integration.SESSION).read_text()
            with self.assertRaises(ValueError):
                integration.apply(root)
            self.assertEqual((root / integration.SESSION).read_text(), before)

    def test_drops_optional_recommendations_and_keeps_python_required(self):
        with tempfile.TemporaryDirectory() as folder:
            root = self.fixture(folder)
            (root / integration.CONTROL).write_text('Depends: libpam0g\nRecommends: kwallet6, libpam-kwallet5, other-package\n')
            integration.apply(root)
            rec = (root / integration.CONTROL).read_text()
            self.assertIn('Depends: libpam0g, python3', rec)
            self.assertNotIn('Recommends:', rec)
            for dep in ['kwallet6', 'libpam-kwallet5', 'other-package', 'dex', 'x11-utils']:
                self.assertNotIn(dep, rec)


if __name__ == '__main__':
    unittest.main()

