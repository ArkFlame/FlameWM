#!/usr/bin/env python3
"""The archiver's desktop registration must cover exactly the formats the core accepts.

`crates/flamewm-archiver/src/format.rs` is the only owner of accepted suffixes; the desktop entry
and the mimeapps list are the only owners of the MIME registration. This guard derives the MIME
types from the suffixes with the system's own classifier, so adding a format to the crate without
registering it (or the reverse) fails here instead of in a file manager.

Skipped when `xdg-mime` or the shared-mime-info database is unavailable.
"""
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DESKTOP = ROOT / 'packaging/flamewm-archiver.desktop'
MIMEAPPS = ROOT / 'packaging/flamewm-mimeapps.list'
FORMAT_RS = ROOT / 'crates/flamewm-archiver/src/format.rs'
DESKTOP_ID = 'flamewm-archiver.desktop'


def suffixes():
    """Suffixes from the `(".tar.gz", Format::TarGz)` table, most specific first."""
    return re.findall(r'\("(\.[^"]+)",\s*Format::\w+\)', FORMAT_RS.read_text())


def desktop_mime_types():
    for line in DESKTOP.read_text().splitlines():
        if line.startswith('MimeType='):
            return [m for m in line.split('=', 1)[1].split(';') if m]
    raise AssertionError('flamewm-archiver.desktop has no MimeType=')


def default_association(mime):
    """Every `mime=id;` line in the file's `[Default Applications]` section."""
    section = None
    for line in MIMEAPPS.read_text().splitlines():
        line = line.strip()
        if line.startswith('['):
            section = line
        elif section == '[Default Applications]' and '=' in line:
            key, value = line.split('=', 1)
            if key == mime:
                return [i for i in value.split(';') if i]
    return None


def classify(name, directory):
    """The MIME type `xdg-mime` reports for an empty file called `name`."""
    path = Path(directory) / name
    path.touch()
    result = subprocess.run(['xdg-mime', 'query', 'filetype', str(path)],
                            text=True, capture_output=True)
    return result.stdout.strip() if result.returncode == 0 else ''


def unavailable():
    if not shutil.which('xdg-mime'):
        return 'xdg-mime is not installed'
    with tempfile.TemporaryDirectory() as directory:
        if not classify('probe.zip', directory).startswith('application/'):
            return 'shared-mime-info database is unavailable'
    return None


@unittest.skipIf(unavailable(), unavailable() or '')
class ArchiverMimeContract(unittest.TestCase):
    def test_registered_types_are_exactly_the_classified_supported_suffixes(self):
        expected = set()
        with tempfile.TemporaryDirectory() as directory:
            for suffix in suffixes():
                mime = classify('archive' + suffix, directory)
                self.assertTrue(mime.startswith('application/'), suffix)
                expected.add(mime)
        self.assertEqual(set(desktop_mime_types()), expected)

    def test_every_registered_type_defaults_to_the_archiver(self):
        for mime in desktop_mime_types():
            self.assertEqual(default_association(mime), [DESKTOP_ID], mime)

    def test_no_default_association_without_a_desktop_registration(self):
        registered = set(desktop_mime_types())
        for line in MIMEAPPS.read_text().splitlines():
            if line.strip() and not line.startswith('[') and '=' in line:
                mime, apps = line.split('=', 1)
                if DESKTOP_ID in apps:
                    self.assertIn(mime, registered)


if __name__ == '__main__':
    unittest.main()
