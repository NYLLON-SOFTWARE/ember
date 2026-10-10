"""Initial setup survives interruption before either staging directory is complete."""
import builtins
import os
from pathlib import Path
import select
import signal
import termios
import unittest
from unittest import mock

import test_installer as fixtures

m = fixtures.m


class PreparingInstall(unittest.TestCase):
    setUp = fixtures.Lifecycle.setUp
    fresh_bundle = fixtures.Lifecycle.fresh_bundle

    def interrupt_mkdir(self, args, key):
        original = Path.mkdir
        def interrupted(path, *values, **options):
            if path.name.startswith(".ember-install-" if key == "staged_config" else ".storage-install-"):
                journal = m.json_file(self.manager.journal_file)
                self.assertEqual(journal["phase"], "preparing")
                self.assertEqual(journal[key], str(path))
                raise KeyboardInterrupt("power lost during staging mkdir")
            return original(path, *values, **options)
        with mock.patch.object(m.socket, "getaddrinfo"), mock.patch.object(m.socket, "socket"), mock.patch.object(Path, "mkdir", interrupted):
            with self.assertRaisesRegex(KeyboardInterrupt, "power lost"):
                self.manager.install(args)
        return m.json_file(self.manager.journal_file)

    def test_journal_precedes_the_first_staging_directory(self):
        args = self.fresh_bundle()
        journal = self.interrupt_mkdir(args, "staged_config")
        self.assertFalse(Path(journal["staged_config"]).exists())
        self.assertFalse(Path(journal["staged_storage"]).exists())
        self.assertEqual(self.manager.preflight(None), args.domain)
        self.manager.install(args)
        self.assertFalse(self.manager.journal_file.exists())
        self.assertEqual(sum(event[1] == "run" for event in self.docker.events), 1)

    def test_second_directory_interruption_is_resumable_without_manual_cleanup(self):
        args = self.fresh_bundle()
        journal = self.interrupt_mkdir(args, "staged_storage")
        self.assertEqual(Path(journal["staged_config"]).stat().st_mode & 0o777, 0o700)
        self.assertFalse(Path(journal["staged_storage"]).exists())
        self.manager.install(args)
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])
        self.assertFalse(self.manager.journal_file.exists())

    def test_durable_secrets_survive_preparing_phase_retry(self):
        args = self.fresh_bundle()
        original = m.atomic_json
        def interrupted(path, value):
            if path.name == "runtime.json" and path.parent.name.startswith(".ember-install-"):
                raise KeyboardInterrupt("power lost after secrets persisted")
            return original(path, value)
        with mock.patch.object(m.socket, "getaddrinfo"), mock.patch.object(m.socket, "socket"), mock.patch.object(m, "atomic_json", side_effect=interrupted):
            with self.assertRaisesRegex(KeyboardInterrupt, "secrets persisted"):
                self.manager.install(args)
        journal = m.json_file(self.manager.journal_file)
        self.assertEqual(journal["phase"], "preparing")
        secret = (Path(journal["staged_config"]) / "app.env").read_bytes()
        self.assertNotIn("SECRET_KEY_BASE", self.manager.journal_file.read_text())
        self.assertNotIn("EMBER_SETUP_TOKEN", self.manager.journal_file.read_text())
        self.manager.install(args)
        self.assertEqual((self.manager.config / "app.env").read_bytes(), secret)
        self.assertEqual(sum(event[1] == "run" for event in self.docker.events), 1)

    def test_recovery_refuses_path_substitution_before_creating_outside_stage(self):
        args = self.fresh_bundle()
        journal = self.interrupt_mkdir(args, "staged_config")
        outside = self.manager.data.parent / "unmanaged"
        journal["staged_config"] = str(outside)
        m.atomic_json(self.manager.journal_file, journal)
        with self.assertRaisesRegex(m.Failure, "Unsafe preparing installation path"):
            self.manager.install(args)
        self.assertFalse(outside.exists())
        self.assertFalse(any(event[1] == "run" for event in self.docker.events))

    def test_recovery_refuses_symlink_or_public_staging_directory(self):
        args = self.fresh_bundle()
        journal = self.interrupt_mkdir(args, "staged_config")
        stage = Path(journal["staged_config"])
        stage.symlink_to(args.bundle_dir)
        with self.assertRaisesRegex(m.Failure, "Unsafe preparing installation directory"):
            self.manager.install(args)
        stage.unlink()
        stage.mkdir(mode=0o755)
        with self.assertRaisesRegex(m.Failure, "Unsafe preparing installation directory"):
            self.manager.install(args)
        self.assertFalse(any(event[1] == "run" for event in self.docker.events))

    def test_recovery_requires_original_manager_bytes(self):
        args = self.fresh_bundle()
        self.interrupt_mkdir(args, "staged_config")
        (args.bundle_dir / "emberctl.py").write_text("modified manager")
        with self.assertRaisesRegex(m.Failure, "Recovery manager files differ"):
            self.manager.install(args)
        self.assertFalse(any(event[1] == "run" for event in self.docker.events))

    def test_hostname_prompt_uses_real_nonseekable_terminal_streams(self):
        self.fresh_bundle()
        master, slave = os.openpty()
        previous_hup = signal.signal(signal.SIGHUP, signal.SIG_IGN)
        try:
            attributes = termios.tcgetattr(slave)
            attributes[3] &= ~termios.ECHO
            termios.tcsetattr(slave, termios.TCSANOW, attributes)
            terminal = os.ttyname(slave)
            os.write(master, b"CHAT.Example.COM\n")
            original = builtins.open
            def redirect(file, *values, **options):
                return original(terminal if file == "/dev/tty" else file, *values, **options)
            with mock.patch.object(builtins, "open", side_effect=redirect), mock.patch.object(m.socket, "getaddrinfo"), mock.patch.object(m.socket, "socket"):
                self.assertEqual(self.manager.preflight(None), "chat.example.com")
            self.assertTrue(select.select([master], [], [], 1)[0], "Hostname prompt did not reach the terminal")
            self.assertIn(b"Hostname pointing to this server", os.read(master, 4096))
        finally:
            os.close(slave)
            os.close(master)
            signal.signal(signal.SIGHUP, previous_hup)


if __name__ == "__main__":
    unittest.main()
