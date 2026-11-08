import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import flamewm_harness_common as common  # noqa: E402


class DisplayInUseTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = pathlib.Path(tmp.name)

    def test_lock_with_dead_owner_is_not_alive(self):
        child = subprocess.Popen([sys.executable, "-c", "pass"])
        child.wait()
        lock = self.root / "lock"
        lock.write_text(f"{child.pid}\n", encoding="ascii")
        self.assertFalse(common._lock_owner_alive(lock))

    def test_lock_with_live_owner_is_alive(self):
        lock = self.root / "lock"
        lock.write_text(f"{os.getpid()}\n", encoding="ascii")
        self.assertTrue(common._lock_owner_alive(lock))

    def test_unreadable_or_garbage_lock_is_not_alive(self):
        self.assertFalse(common._lock_owner_alive(self.root / "missing"))
        garbage = self.root / "garbage"
        garbage.write_text("not-a-pid", encoding="ascii")
        self.assertFalse(common._lock_owner_alive(garbage))

    def test_stale_socket_file_does_not_accept(self):
        path = self.root / "X99"
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(str(path))
        server.close()  # leaves the socket file behind with no listener
        self.assertTrue(path.exists())
        self.assertFalse(common._socket_accepts(path))

    def test_listening_socket_accepts(self):
        path = self.root / "X98"
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(str(path))
        server.listen(1)
        self.addCleanup(server.close)
        self.assertTrue(common._socket_accepts(path))

    def test_stale_files_do_not_mark_display_busy(self):
        original = common.X11_SOCKET_DIR
        common.X11_SOCKET_DIR = self.root
        self.addCleanup(setattr, common, "X11_SOCKET_DIR", original)
        stale_socket = self.root / "X9123"
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(str(stale_socket))
        server.close()
        self.assertFalse(common.display_in_use("9123"))


if __name__ == "__main__":
    unittest.main()
