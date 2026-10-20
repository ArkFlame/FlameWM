#!/usr/bin/env python3
"""Verify the actual XDG lookup precedence used by FlameWM session compatibility."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


@unittest.skipUnless(shutil.which('xdg-mime'), 'xdg-mime is not installed')
class MimeResolution(unittest.TestCase):
    def test_flamewm_uses_kde_fallback_but_never_overrides_user_default(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config_home = root / 'config'
            fallback = root / 'flamewm-compat-config'
            data = root / 'xdg-data'
            config_home.mkdir()
            fallback.mkdir()
            (data / 'applications').mkdir(parents=True)
            (fallback / 'flamewm-mimeapps.list').write_text(
                '[Default Applications]\n'
                'text/plain=org.kde.kate.desktop;\n'
                'video/mp4=org.kde.haruna.desktop;\n'
            )
            for desktop_id, mime in (
                ('org.kde.kate.desktop', 'text/plain'),
                ('org.kde.haruna.desktop', 'video/mp4'),
                ('custom-editor.desktop', 'text/plain'),
            ):
                (data / 'applications' / desktop_id).write_text(
                    '[Desktop Entry]\nType=Application\nName=Test\nExec=/bin/true\n'
                    f'MimeType={mime};\n'
                )
            environment = dict(os.environ)
            environment.update(
                XDG_CURRENT_DESKTOP='FlameWM',
                XDG_CONFIG_HOME=str(config_home),
                XDG_CONFIG_DIRS=str(fallback),
                XDG_DATA_HOME=str(root / 'empty-data'),
                XDG_DATA_DIRS=str(data),
            )
            def query(mime):
                result = subprocess.run(['xdg-mime', 'query', 'default', mime],
                    env=environment, text=True, capture_output=True, check=True)
                return result.stdout.strip()
            self.assertEqual(query('text/plain'), 'org.kde.kate.desktop')
            self.assertEqual(query('video/mp4'), 'org.kde.haruna.desktop')
            (config_home / 'mimeapps.list').write_text(
                '[Default Applications]\ntext/plain=custom-editor.desktop;\n'
            )
            self.assertEqual(query('text/plain'), 'custom-editor.desktop')
            self.assertEqual(query('video/mp4'), 'org.kde.haruna.desktop')


if __name__ == '__main__':
    unittest.main()
