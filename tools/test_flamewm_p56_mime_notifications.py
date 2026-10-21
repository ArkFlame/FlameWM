"""Contract regressions for FlameWM MIME integration and notification fallback.

Uses source and isolated subprocess fixtures, never requires an X11 display or
modifies a user's MIME settings. Runtime interoperability is a separate gate.
"""
import importlib.util
import pathlib
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]

spec = importlib.util.spec_from_file_location('flamewm_p56_services', ROOT / 'tools/flamewm_desktop_services.py')
services = importlib.util.module_from_spec(spec)
spec.loader.exec_module(services)


class MimeAndNotifications(unittest.TestCase):
    def test_session_menu_is_a_real_shared_contract_not_just_an_orphan_file(self):
        core = (ROOT / 'crates/flamewm-session-core/src/lib.rs').read_text()
        session = (ROOT / 'crates/flamewm-wm/src/bin/flamewm-session.rs').read_text()
        compat = (ROOT / 'crates/flamewm-wm/src/session_app_compat.rs').read_text()
        script = (ROOT / 'scripts/package').read_text()
        self.assertIn('"XDG_MENU_PREFIX".to_owned(), "flamewm-".to_owned()', core)
        self.assertIn('"XDG_MENU_PREFIX",', session)
        self.assertIn('install_applications_menu(&config_dir)?;', compat)
        self.assertIn('include_str!("../../../packaging/menus/flamewm-applications.menu")', compat)
        self.assertIn('"$STAGE/etc/xdg/menus/flamewm-applications.menu"', script)
        self.assertIn('fs::create_dir_all(&data_dir)', compat)

    def test_standard_menu_is_not_replaced_with_dummy_kde_desktop_identity(self):
        menu = (ROOT / 'packaging/menus/flamewm-applications.menu').read_text()
        self.assertIn('<DefaultAppDirs/>', menu)
        self.assertIn('<Include><All/></Include>', menu)
        self.assertNotIn('XDG_CURRENT_DESKTOP=KDE',
                         (ROOT / 'scripts/flamewm-session-wrapper').read_text())
        self.assertIn('export XDG_MENU_PREFIX=flamewm-',
                      (ROOT / 'scripts/flamewm-session-wrapper').read_text())

    def test_existing_notification_owner_is_not_replaced(self):
        env = {'XDG_CURRENT_DESKTOP': 'FlameWM'}
        plan = services.choose_programs(env, find=lambda _: '/usr/bin/dunst',
            process_active=lambda _: False, notifier_available=lambda: False,
            notifier_owned=lambda: True)
        self.assertIsNone(plan['notifications'])

    def test_activatable_file_without_owner_does_not_hide_fallback(self):
        env = {'XDG_CURRENT_DESKTOP': 'FlameWM'}
        with mock.patch.object(services, 'notification_owned', return_value=False):
            plan = services.choose_programs(env, find=lambda _: '/usr/bin/dunst',
                process_active=lambda _: False, notifier_available=lambda: False,
                notifier_owned=lambda: False)
        self.assertEqual(plan['notifications'][0], '/usr/bin/dunst')
        self.assertEqual(plan['notifications'][1], '--config')
        self.assertTrue(pathlib.Path(plan['notifications'][2]).is_file())

    def test_activate_notification_service_rechecks_bus_owner(self):
        with mock.patch.object(services.shutil, 'which', return_value='/usr/bin/busctl'), \
             mock.patch.object(services.subprocess, 'run') as run, \
             mock.patch.object(services, 'notification_owned', return_value=False):
            run.return_value.returncode = 0
            self.assertFalse(services.activate_notification_service())
            self.assertIn('StartServiceByName', run.call_args.args[0])
        with mock.patch.object(services.shutil, 'which', return_value='/usr/bin/busctl'), \
             mock.patch.object(services.subprocess, 'run') as run, \
             mock.patch.object(services, 'notification_owned', return_value=True):
            run.return_value.returncode = 0
            self.assertTrue(services.activate_notification_service())

    def test_notification_configuration_packaged_with_bottom_right_and_timeout(self):
        source = (ROOT / 'packaging/notifications/dunstrc').read_text()
        self.assertIn('origin = bottom-right', source)
        self.assertIn('progress_bar = true', source)
        self.assertIn('timeout = 6', source)
        self.assertIn('timeout = 0', source)
        manifest = (ROOT / 'packaging/install-manifest.txt').read_text()
        self.assertIn('packaging/notifications/dunstrc', manifest)
        control = (ROOT / 'packaging/debian/control.in').read_text()
        self.assertIn('libpam-runtime, python3', control)
        self.assertNotIn('dunst', control)

    def test_autostart_preserves_existing_xdg_dirs_and_adds_prefix(self):
        with tempfile.TemporaryDirectory() as folder:
            config = pathlib.Path(folder)
            (config / 'flamewm/session-compat/config').mkdir(parents=True)
            (config / 'flamewm/session-compat/data').mkdir(parents=True)
            env = {'HOME': folder, 'XDG_CONFIG_HOME': str(config), 'XDG_CONFIG_DIRS': '/site:/etc/xdg',
                   'XDG_DATA_DIRS': '/opt/share:/usr/share', 'XDG_CURRENT_DESKTOP': 'FlameWM'}
            result = services.flamewm_autostart_environment(env)
            self.assertEqual(result['XDG_MENU_PREFIX'], 'flamewm-')
            self.assertTrue(result['XDG_CONFIG_DIRS'].endswith(':/site:/etc/xdg'))
            self.assertTrue(result['XDG_DATA_DIRS'].endswith(':/opt/share:/usr/share'))
            self.assertNotIn('XDG_MENU_PREFIX', env)


if __name__ == '__main__':
    unittest.main()
