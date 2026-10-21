"""Source and standards regression for missing KDE application-menu discovery."""
from pathlib import Path
import importlib.util
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('flamewm_open_with_check', ROOT/'tools/verify_flamewm_open_with.py')
check=importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)

class KServiceMenuContract(unittest.TestCase):
    def test_menu_is_valid_and_includes_all_standard_apps(self):
        root=ET.parse(ROOT/'packaging/menus/flamewm-applications.menu').getroot()
        self.assertEqual(root.tag,'Menu')
        for key in ('DefaultAppDirs','DefaultDirectoryDirs','DefaultMergeDirs','DefaultLayout'):
            self.assertIsNotNone(root.find(key),key)
        self.assertIsNotNone(root.find('Include/All'))
    def test_real_session_writes_menu_before_xdg_config_dirs_are_prepended(self):
        source=(ROOT/'crates/flamewm-wm/src/session_app_compat.rs').read_text()
        self.assertIn('install_applications_menu(&config_dir)?;',source)
        self.assertIn('fs::create_dir_all(&data_dir)',source)
        self.assertIn('"menus/flamewm-applications.menu"',source)
        self.assertLess(source.index('install_applications_menu(&config_dir)?;'),source.index('for plan in processes {'))
        self.assertIn('XDG_MENU_PREFIX', (ROOT/'crates/flamewm-session-core/src/lib.rs').read_text())
        self.assertIn('"XDG_MENU_PREFIX",', (ROOT/'crates/flamewm-wm/src/bin/flamewm-session.rs').read_text())
    def test_package_installs_menu_for_native_sessions(self):
        script=(ROOT/'scripts/package').read_text()
        self.assertIn('"$STAGE/etc/xdg/menus/flamewm-applications.menu"',script)
        self.assertIn('/etc/xdg/menus/flamewm-applications.menu',script)
        self.assertIn('packaging/menus/flamewm-applications.menu', (ROOT/'packaging/install-manifest.txt').read_text())
    def test_xephyr_and_companion_inherit_flamewm_menu(self):
        self.assertIn('export XDG_MENU_PREFIX=flamewm-', (ROOT/'scripts/xephyr').read_text())
        wrapper=(ROOT/'scripts/flamewm-session-wrapper').read_text()
        self.assertIn('export XDG_MENU_PREFIX=flamewm-', wrapper)
        self.assertIn('export XDG_CURRENT_DESKTOP=FlameWM', wrapper)
        self.assertIn('result["XDG_MENU_PREFIX"] = "flamewm-"', (ROOT/'tools/flamewm_desktop_services.py').read_text())
    def test_xdg_resolution_prioritizes_user_then_owned_then_system(self):
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp)
            config=p/'config'; owned=p/'owned'; system=p/'system'
            for d in (config,owned,system): (d/'menus').mkdir(parents=True)
            (owned/'menus/flamewm-applications.menu').write_text('owned')
            (system/'menus/flamewm-applications.menu').write_text('system')
            env={'HOME':tmp,'XDG_CONFIG_HOME':str(config),'XDG_CONFIG_DIRS':f'{owned}:{system}','XDG_MENU_PREFIX':'flamewm-'}
            found=[x for x in check.menu_candidates(env) if x.is_file()]
            self.assertEqual(found,[owned/'menus/flamewm-applications.menu',system/'menus/flamewm-applications.menu'])
            (config/'menus/flamewm-applications.menu').write_text('user override')
            self.assertEqual(next(x for x in check.menu_candidates(env) if x.is_file()),config/'menus/flamewm-applications.menu')
    def test_diagnostics_do_not_write_or_change_defaults(self):
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp); cfg=p/'c'; data=p/'data'; (cfg/'menus').mkdir(parents=True);(data/'applications').mkdir(parents=True)
            (cfg/'menus/flamewm-applications.menu').write_text('<DefaultAppDirs/>')
            (data/'applications/editor.desktop').write_text('[Desktop Entry]\nType=Application\nMimeType=text/plain;\n')
            env={'HOME':tmp,'XDG_CURRENT_DESKTOP':'FlameWM','XDG_MENU_PREFIX':'flamewm-','XDG_CONFIG_DIRS':str(cfg),'XDG_DATA_DIRS':str(data)}
            self.assertTrue(check.audit(env))
            self.assertEqual(list(p.rglob('mimeapps.list')),[])
            env['XDG_MENU_PREFIX']='plasma-'
            self.assertFalse(check.audit(env))
if __name__=='__main__': unittest.main()
