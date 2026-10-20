"""Offline ImageMagick provider contract for FlameWM desktop Convert.

This exercises the actual external decoder/encoder without starting X11. It
complements, but does not replace, the Rust UI-to-worker regression tests.
"""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


CONVERTER = shutil.which('magick') or shutil.which('convert')
IDENTIFY = shutil.which('identify') or shutil.which('magick')


@unittest.skipUnless(CONVERTER and IDENTIFY, 'ImageMagick executable unavailable')
class ImageMagickConvertTests(unittest.TestCase):
    def _create_gif(self, root: Path) -> Path:
        src = root / 'animated space.gif'
        subprocess.run([CONVERTER, '-size', '8x8', 'xc:red', '-size', '8x8',
                        'xc:blue', '-delay', '20', '-loop', '0', str(src)],
                       check=True, capture_output=True)
        return src

    def _identify_frames(self, source: Path) -> list[str]:
        cmd = [IDENTIFY]
        if Path(IDENTIFY).name == 'magick':
            cmd.append('identify')
        result = subprocess.run([*cmd, str(source)], check=True,
                                text=True, capture_output=True)
        return result.stdout.splitlines()

    def test_single_frame_targets_emit_exactly_one_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            source = self._create_gif(folder)
            for ext in ('jpg', 'png', 'bmp'):
                temp = folder / f'.flamewm-convert-test.{ext}'
                temp.touch(exist_ok=False)
                subprocess.run([CONVERTER, f'{source}[0]', f'{ext}:{temp}'],
                               check=True, capture_output=True)
                self.assertGreater(temp.stat().st_size, 0)
                self.assertEqual(len(self._identify_frames(temp)), 1)
                self.assertEqual(
                    sorted(path.name for path in folder.iterdir()),
                    sorted([source.name, *[f'.flamewm-convert-test.{suffix}'
                                                 for suffix in ('jpg', 'png', 'bmp')
                                                 if (folder / f'.flamewm-convert-test.{suffix}').exists()]]),
                    'single-frame conversion must not leave hidden numbered files',
                )

    def test_animated_webp_is_one_file_with_two_frames(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            source = self._create_gif(folder)
            temp = folder / '.flamewm-convert-test.webp'
            temp.touch(exist_ok=False)
            subprocess.run([CONVERTER, str(source), f'webp:{temp}'],
                           check=True, capture_output=True)
            self.assertGreater(temp.stat().st_size, 0)
            self.assertEqual(len(self._identify_frames(temp)), 2)
            self.assertEqual(set(p.name for p in folder.iterdir()),
                             {source.name, temp.name})


if __name__ == '__main__':
    unittest.main()
