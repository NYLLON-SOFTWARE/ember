"""No daemon/root/network required: exercise real bundles, archives and transaction ordering."""
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


m = load("emberctl_test", HERE / "emberctl.py")
builder = load("bundle_test", HERE / "build-bundle.py")


def release(version="1.0.0", char="a"):
    value = {"schema": 1, "version": version, "source_sha": "b" * 40, "image": m.IMAGE + "@sha256:" + char * 64}
    return value


class Response(io.BytesIO):
    status = 200
    url = "https://github.com/NYLLON-SOFTWARE/ember/releases/download/v2.0.0/asset"


class Docker:
    def __init__(self, manager, state):
        self.manager, self.events, self.inputs = manager, [], []
        self.fail_pull = self.fail_create = self.fail_start = False
        self.after_create_failure = False
        self.volumes = {}
        self.setup_status = "pending"
        self.containers = {"ember": self.container(state, running=True)}

    def container(self, state, running=False):
        return {"Config": {"Image": state["release"]["image"], "Labels": {m.LABEL: "1", m.INSTALL_LABEL: state["install_id"]}},
                "Mounts": [{"Type": "bind", "Source": str(self.manager.storage), "Destination": "/rails/storage"}],
                "State": {"Running": running, "StartedAt": "2026-10-10T00:00:00Z" if running else "0001-01-01T00:00:00Z", "ExitCode": 0}}

    def __call__(self, args, *, capture=True, input=None, check=True):
        args = list(map(str, args))
        self.events.append(args)
        self.inputs.append((args, input))
        command = args[1]
        output, code = "", 0
        if args[1:3] == ["container", "inspect"]:
            item = self.containers.get(args[3])
            output, code = (json.dumps([item]), 0) if item else ("", 1)
        elif command == "ps":
            output = "\n".join(name for name, item in self.containers.items() if "-a" in args or item["State"]["Running"])
        elif args[1:3] == ["volume", "ls"]:
            output = "\n".join(self.volumes)
        elif args[1:3] == ["volume", "inspect"]:
            output = json.dumps([self.volumes[args[3]]])
        elif command == "exec" and "reset-password" in args:
            output = "Password reset.\n"
        elif command == "exec" and args[-1] == "setup-status":
            output = self.setup_status + "\n"
        elif command == "run" and args[-1] == "generate-secrets":
            output = "SECRET_KEY_BASE=" + "a" * 128 + "\nVAPID_PUBLIC_KEY=" + "b" * 87 + "\nVAPID_PRIVATE_KEY=" + "c" * 43 + "\nEMBER_SETUP_TOKEN=" + "d" * 64 + "\n"
        elif command == "pull":
            if self.fail_pull:
                raise m.Failure("registry unavailable")
        elif command == "image":
            output = "[]"
        elif command == "stop":
            self.containers[args[-1]]["State"]["Running"] = False
        elif command == "start":
            item = self.containers[args[-1]]
            item["State"].update(Running=True, StartedAt="2026-10-10T00:00:00Z")
            if self.fail_start:
                raise m.Failure("start failed after launch")
        elif command == "rename":
            self.containers[args[3]] = self.containers.pop(args[2])
        elif command == "rm":
            self.containers.pop(args[-1])
        elif command == "create":
            if self.fail_create:
                raise m.Failure("create failed before launch")
            state = self.manager.state()
            self.containers["ember"] = self.container(state)
            if self.after_create_failure:
                raise m.Failure("create API returned failure after creation")
        else:
            raise AssertionError(args)
        return subprocess.CompletedProcess(args, code, stdout=output, stderr="")


class Lifecycle(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.manager = m.Manager(root / "etc" / "ember", root / "data", root / "lib" / "ember", root / "bin" / "emberctl")
        self.manager.config.mkdir(parents=True, mode=0o700)
        self.manager.data.mkdir(mode=0o700)
        self.manager.storage.mkdir(mode=0o700)
        self.manager.command.parent.mkdir()
        self.manager.backups.mkdir(mode=0o700)
        self.state = {"managed_by": "emberctl-v1", "install_id": "c" * 32, "domain": "chat.example.com", "storage": str(self.manager.storage), "release": release()}
        m.atomic_json(self.manager.state_file, self.state)
        m.atomic_json(self.manager.config / "runtime.json", {})
        m.atomic_bytes(self.manager.config / "app.env", b"SECRET_KEY_BASE=private-secret\nEMBER_SETUP_TOKEN=" + b"d" * 64 + b"\n")
        self.manager.install_manager({"emberctl": b"original-wrapper", "emberctl.py": b"original-manager"})
        (self.manager.storage / "db").mkdir()
        connection = sqlite3.connect(self.manager.storage / "db" / "production.sqlite3")
        connection.execute("CREATE TABLE messages(body TEXT)")
        connection.execute("INSERT INTO messages VALUES ('original message')")
        connection.commit()
        connection.close()
        (self.manager.storage / "files").mkdir()
        (self.manager.storage / "files" / "upload.bin").write_bytes(b"original upload\x00")
        (self.manager.storage / "thruster").mkdir()
        (self.manager.storage / "thruster" / "cert.pem").write_text("original cached cert")
        self.docker = Docker(self.manager, self.state)
        # The policy defaults stay root/UID1000 in production; local test fixtures use their owner.
        self.patches = [mock.patch.object(m, "ROOT_UID", os.getuid()), mock.patch.object(m, "APP_UID", os.getuid()), mock.patch.object(m, "APP_GID", os.getgid()),
                        mock.patch.object(m, "run", self.docker), mock.patch.object(self.manager, "readiness")]
        for patch in self.patches:
            patch.start()
        self.addCleanup(self.temporary.cleanup)
        for patch in self.patches:
            self.addCleanup(patch.stop)
        output = root / "release"
        self.next_release = builder.build("2.0.0", "sha256:" + "e" * 64, "f" * 40, output)
        self.bundle = (output / "ember-installer-2.0.0.tar.gz").read_bytes()
        self.network = mock.patch.object(m.urllib.request, "urlopen", side_effect=lambda *a, **k: Response(self.bundle))
        self.network.start()
        self.addCleanup(self.network.stop)
        self.manifest = mock.patch.object(m, "download_manifest", return_value=self.next_release)
        self.manifest.start()
        self.addCleanup(self.manifest.stop)
        self.stdout = io.StringIO()
        self.output = contextlib.redirect_stdout(self.stdout)
        self.output.__enter__()
        self.addCleanup(lambda: self.output.__exit__(None, None, None))

    def test_real_cold_backup_restores_sqlite_upload_secrets_cert_and_manager(self):
        identifier = self.manager.backup(self.state)
        backup = self.manager.backups / identifier
        self.assertEqual(backup.stat().st_mode & 0o777, 0o700)
        self.assertEqual((backup / "snapshot.tar.gz").stat().st_mode & 0o777, 0o600)
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])
        (self.manager.storage / "files" / "upload.bin").write_text("new upload")
        (self.manager.storage / "thruster" / "cert.pem").write_text("new cert")
        m.atomic_bytes(self.manager.config / "app.env", b"new secrets")
        self.manager.install_manager({"emberctl": b"new-wrapper", "emberctl.py": b"new-manager"})
        with sqlite3.connect(self.manager.storage / "db" / "production.sqlite3") as db:
            db.execute("INSERT INTO messages VALUES ('new message')")
        with self.assertRaisesRegex(m.Failure, "accept-data-loss"):
            self.manager.restore(self.state, identifier, False)
        self.manager.restore(self.state, identifier, True)
        self.assertEqual((self.manager.storage / "files" / "upload.bin").read_bytes(), b"original upload\x00")
        self.assertEqual((self.manager.storage / "thruster" / "cert.pem").read_text(), "original cached cert")
        self.assertIn(b"private-secret", (self.manager.config / "app.env").read_bytes())
        self.assertEqual((self.manager.lib / "emberctl.py").read_bytes(), b"original-manager")
        with sqlite3.connect(self.manager.storage / "db" / "production.sqlite3") as db:
            self.assertEqual(db.execute("SELECT body FROM messages").fetchall(), [("original message",)])
        self.assertFalse(self.manager.journal_file.exists())
        self.assertNotIn("private-secret", self.stdout.getvalue())

    def fresh_bundle(self):
        for path in (self.manager.config, self.manager.storage, self.manager.backups, self.manager.lib):
            shutil.rmtree(path)
        self.manager.command.unlink()
        self.docker.containers.clear()
        bundle = self.manager.data.parent / "unpacked"
        bundle.mkdir()
        with tarfile.open(fileobj=io.BytesIO(self.bundle), mode="r:gz") as source:
            for member in source:
                (bundle / member.name).write_bytes(source.extractfile(member).read())
        return m.argparse.Namespace(bundle_dir=bundle, domain="chat.example.com", runtime_env=None, test_ca=None)

    def test_test_trust_environment_uses_existing_validation_before_image_pull(self):
        original = self.fresh_bundle()
        runtime = original.bundle_dir / "runtime.env"
        ca = original.bundle_dir / "test-ca.pem"
        runtime.write_text("ACME_DIRECTORY=https://pebble.example.test/dir\nSSL_CERT_FILE=/run/ember-test-ca.pem\n")
        ca.write_bytes(b"not a certificate")
        command = ["install", "--bundle-dir", str(original.bundle_dir), "--domain", original.domain]
        environment = {"EMBER_INSTALL_RUNTIME_ENV": str(runtime), "EMBER_INSTALL_TEST_CA": str(ca)}
        cases = [
            ("unpaired", {"EMBER_INSTALL_RUNTIME_ENV": str(runtime)}, "ACME_DIRECTORY=https://pebble.example.test/dir\nSSL_CERT_FILE=/run/ember-test-ca.pem\n", b"bad", "provided together"),
            ("unknown option", environment, "ACME_DIRECTORY=https://pebble.example.test/dir\nSSL_CERT_FILE=/run/ember-test-ca.pem\nRUN=$(touch never-execute)\n", b"bad", "Only ACME_DIRECTORY"),
            ("HTTP directory", environment, "ACME_DIRECTORY=http://pebble.example.test/dir\nSSL_CERT_FILE=/run/ember-test-ca.pem\n", b"bad", "must use HTTPS"),
            ("CA mount override", environment, "ACME_DIRECTORY=https://pebble.example.test/dir\nSSL_CERT_FILE=/etc/ssl/arbitrary.pem\n", b"bad", "Only ACME_DIRECTORY"),
            ("oversized CA", environment, "ACME_DIRECTORY=https://pebble.example.test/dir\nSSL_CERT_FILE=/run/ember-test-ca.pem\n", b"x" * (1024 * 1024 + 1), "exceeds 1 MiB"),
        ]
        for label, env, contents, certificate, diagnostic in cases:
            with self.subTest(label=label), mock.patch.dict(m.os.environ, env, clear=True):
                runtime.write_text(contents)
                ca.write_bytes(certificate)
                args = m.parser().parse_args(command)
                with mock.patch.object(m.socket, "getaddrinfo"), mock.patch.object(m.socket, "socket"):
                    with self.assertRaisesRegex(m.Failure, diagnostic):
                        self.manager.install(args)
        runtime.write_text("ACME_DIRECTORY=https://pebble.example.test/dir\nSSL_CERT_FILE=/run/ember-test-ca.pem\n")
        ca.write_bytes(b"not a certificate")
        with mock.patch.dict(m.os.environ, environment, clear=True), mock.patch.object(m.socket, "getaddrinfo"), mock.patch.object(m.socket, "socket"):
            args = m.parser().parse_args(command)
            with self.assertRaises(m.ssl.SSLError):
                self.manager.install(args)
            for source in (runtime, ca):
                contents = source.read_bytes()
                source.unlink()
                source.symlink_to(original.bundle_dir / "other-file")
                with self.assertRaisesRegex(m.Failure, "must not be symlinks"):
                    self.manager.install(args)
                source.unlink()
                source.write_bytes(contents)
        self.assertFalse(any(event[1] == "pull" for event in self.docker.events))
        self.assertFalse((original.bundle_dir / "never-execute").exists())

    def test_fresh_install_generates_once_and_repeat_preserves_secrets_and_data(self):
        args = self.fresh_bundle()
        with mock.patch.object(m.socket, "getaddrinfo", return_value=[(0, 0, 0, "", ("192.0.2.1", 443))]), mock.patch.object(m.socket, "socket"):
            self.manager.install(args)
            secret = (self.manager.config / "app.env").read_bytes()
            (self.manager.storage / "message.txt").write_text("preserved")
            state = self.manager.state_file.read_bytes()
            self.manager.install(args)
        self.assertEqual(secret, (self.manager.config / "app.env").read_bytes())
        self.assertEqual(state, self.manager.state_file.read_bytes())
        self.assertEqual((self.manager.storage / "message.txt").read_text(), "preserved")
        self.assertEqual(sum(event[1] == "run" for event in self.docker.events), 1)
        generator = next(event for event in self.docker.events if event[1] == "run")
        self.assertEqual(generator[-2:], ["ember", "generate-secrets"])
        self.assertEqual(generator[generator.index("--log-driver") + 1], "none")
        create = next(event for event in self.docker.events if event[1] == "create")
        self.assertEqual(json.loads((self.manager.config / "runtime.json").read_text()), {})
        self.assertFalse(any("ACME_DIRECTORY=" in arg or "SSL_CERT_FILE=" in arg or "test-ca.pem" in arg for arg in create))
        self.assertEqual(create[-2:], ["ember", "server"])
        self.assertEqual(create[create.index("--user") + 1], "1000:1000")
        self.assertEqual(create[create.index("--stop-timeout") + 1], "60")
        self.assertFalse(self.manager.journal_file.exists())
        self.assertIn("/first_run/access#token=", self.stdout.getvalue())

    def test_initial_config_promotion_interruption_resumes_without_rotating_secrets(self):
        args = self.fresh_bundle()
        real_rename = m.os.rename
        calls = []
        def interrupted(source, destination):
            calls.append(str(destination))
            if destination == self.manager.storage:
                raise OSError("simulated power loss after config promotion")
            return real_rename(source, destination)
        with mock.patch.object(m.socket, "getaddrinfo"), mock.patch.object(m.socket, "socket"), mock.patch.object(m.os, "rename", side_effect=interrupted):
            with self.assertRaisesRegex(OSError, "power loss"):
                self.manager.install(args)
        secret = (self.manager.config / "app.env").read_bytes()
        self.assertEqual(m.json_file(self.manager.journal_file)["phase"], "prepared")
        self.assertFalse(self.manager.storage.exists())
        self.manager.install(args)
        self.assertEqual(secret, (self.manager.config / "app.env").read_bytes())
        self.assertEqual(sum(event[1] == "run" for event in self.docker.events), 1)
        self.assertFalse(self.manager.journal_file.exists())
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])

    def test_setup_link_after_setup_is_refused_without_printing_token(self):
        self.docker.setup_status = "initialized"
        with self.assertRaisesRegex(m.Failure, "setup is complete"):
            self.manager.setup_link(self.state)
        self.assertNotIn("d" * 64, self.stdout.getvalue())

    def test_setup_status_failure_is_not_misreported_as_completed_install(self):
        args = self.fresh_bundle()
        self.docker.setup_status = "unexpected output"
        with mock.patch.object(m.socket, "getaddrinfo"), mock.patch.object(m.socket, "socket"):
            with self.assertRaisesRegex(m.Failure, "Unexpected setup status"):
                self.manager.install(args)
        self.assertEqual(m.json_file(self.manager.journal_file)["phase"], "starting")
        self.assertNotIn("already complete", self.stdout.getvalue())
        self.assertNotIn("d" * 64, self.stdout.getvalue())

    def test_password_is_piped_privately_at_exact_cli_byte_limit_then_connections_restart(self):
        password = "😀" * 1024
        with mock.patch.object(m.sys.stdin, "isatty", return_value=True), mock.patch.object(m.getpass, "getpass", side_effect=[password, password]):
            self.manager.reset_password(self.state, "person@example.com")
        invocation, supplied = next((args, data) for args, data in self.docker.inputs if "reset-password" in args)
        self.assertEqual(supplied, password)
        self.assertEqual(len(supplied.encode()), 4096)
        self.assertNotIn(password, invocation)
        self.assertNotIn(password, self.stdout.getvalue())
        self.assertTrue(any(event[1] == "stop" for event in self.docker.events))
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])

    def test_fresh_install_refuses_stopped_campfire_before_pull(self):
        args = self.fresh_bundle()
        self.docker.containers["unrelated-name"] = self.docker.container(self.state)
        self.docker.containers["unrelated-name"]["Config"]["Image"] = "ghcr.io/basecamp/once-campfire:latest"
        with mock.patch.object(m.socket, "getaddrinfo"), mock.patch.object(m.socket, "socket"):
            with self.assertRaisesRegex(m.Failure, "migration procedure"):
                self.manager.install(args)
        self.assertFalse(any(event[1] == "pull" for event in self.docker.events))

    def test_fresh_install_refuses_opaque_volume_with_once_labels(self):
        args = self.fresh_bundle()
        self.docker.volumes["opaque123"] = {"Labels": {"com.basecamp.once.app": "campfire"}}
        with mock.patch.object(m.socket, "getaddrinfo"), mock.patch.object(m.socket, "socket"):
            with self.assertRaisesRegex(m.Failure, "labelled"):
                self.manager.install(args)
        self.assertFalse(any(event[1] == "pull" for event in self.docker.events))

    def test_created_but_never_started_container_can_be_safely_removed(self):
        self.docker.after_create_failure = True
        with self.assertRaisesRegex(m.Failure, "create API"):
            self.manager.update(self.state, "2.0.0")
        self.assertEqual(self.manager.state(), self.state)
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])
        self.assertFalse(self.manager.journal_file.exists())

    def test_backup_failure_resumes_original_app(self):
        with mock.patch.object(self.manager, "archive", side_effect=m.Failure("disk write failed")):
            with self.assertRaisesRegex(m.Failure, "disk write"):
                self.manager.backup(self.state)
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])

    def test_second_running_writer_prevents_cold_backup_and_resumes_app(self):
        self.docker.containers["accidental-other-writer"] = self.docker.container(self.state, running=True)
        with self.assertRaisesRegex(m.Failure, "can write managed storage"):
            self.manager.backup(self.state)
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])
        self.assertEqual(list(self.manager.backups.iterdir()), [])

    def test_interrupted_full_restore_can_be_retried_from_retained_backup(self):
        identifier = self.manager.backup(self.state)
        self.manager.update(self.state, "2.0.0")
        (self.manager.storage / "files" / "upload.bin").write_bytes(b"after upgrade")
        real_rename = m.os.rename
        def interrupted(source, destination):
            if destination == self.manager.storage and source.name.startswith(".storage-restore-"):
                raise OSError("simulated power loss during storage promotion")
            return real_rename(source, destination)
        with mock.patch.object(m.os, "rename", side_effect=interrupted):
            with self.assertRaisesRegex(OSError, "power loss"):
                self.manager.restore(self.manager.state(), identifier, True)
        self.assertEqual(m.json_file(self.manager.journal_file)["operation"], "restore")
        self.assertFalse(self.manager.storage.exists())
        self.manager.restore(self.manager.state(recovery=True), identifier, True)
        self.assertEqual((self.manager.storage / "files" / "upload.bin").read_bytes(), b"original upload\x00")
        self.assertFalse(self.manager.journal_file.exists())
        self.assertEqual(list(self.manager.data.glob(".storage-restore-*")), [])
        self.assertEqual(self.manager.state(), self.state)

    def test_archived_file_corruption_fails_before_restore_stops_app(self):
        identifier = self.manager.backup(self.state)
        archive = self.manager.backups / identifier / "snapshot.tar.gz"
        with archive.open("r+b") as data:
            data.seek(-8, 2)
            data.write(b"corrupt!")
        self.docker.events.clear()
        with self.assertRaisesRegex(m.Failure, "archive SHA256 mismatch"):
            self.manager.restore(self.state, identifier, True)
        self.assertFalse(any(event[1] == "stop" for event in self.docker.events))
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])

    def test_archived_symlink_is_rejected_before_restore_stops_app(self):
        identifier = self.manager.backup(self.state)
        directory = self.manager.backups / identifier
        archive = directory / "snapshot.tar.gz"
        with tarfile.open(archive, "w:gz") as output:
            member = tarfile.TarInfo("storage/link")
            member.type, member.linkname = tarfile.SYMTYPE, "/etc/shadow"
            output.addfile(member)
        metadata = json.loads((directory / "backup.json").read_text())
        metadata["archive_sha256"] = m.digest_file(archive)
        m.atomic_json(directory / "backup.json", metadata)
        self.docker.events.clear()
        with self.assertRaisesRegex(m.Failure, "links, or special"):
            self.manager.restore(self.state, identifier, True)
        self.assertFalse(any(event[1] == "stop" for event in self.docker.events))

    def test_registry_failure_does_not_stop_or_change_previous_version(self):
        self.docker.fail_pull = True
        before = self.manager.state_file.read_bytes()
        with self.assertRaisesRegex(m.Failure, "registry"):
            self.manager.update(self.state, "2.0.0")
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])
        self.assertEqual(before, self.manager.state_file.read_bytes())
        self.assertFalse(any(event[1] == "stop" for event in self.docker.events))

    def test_bundle_checksum_failure_happens_before_downtime(self):
        self.next_release["bundle"]["sha256"] = "0" * 64
        with self.assertRaisesRegex(m.Failure, "SHA256"):
            self.manager.update(self.state, "2.0.0")
        self.assertFalse(any(event[1] == "stop" for event in self.docker.events))
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])

    def test_create_failure_restores_old_container_and_state_before_start(self):
        self.docker.fail_create = True
        with self.assertRaisesRegex(m.Failure, "create failed"):
            self.manager.update(self.state, "2.0.0")
        self.assertEqual(self.manager.state(), self.state)
        self.assertEqual(self.docker.containers["ember"]["Config"]["Image"], self.state["release"]["image"])
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])
        self.assertFalse(self.manager.journal_file.exists())

    def test_post_launch_failure_never_silently_rolls_back_new_writes(self):
        def failure(state):
            (self.manager.storage / "files" / "accepted-after-cutover").write_text("preserve me")
            raise m.Failure("HTTPS readiness failed")
        self.manager.readiness.side_effect = failure
        with self.assertRaisesRegex(m.Failure, "HTTPS"):
            self.manager.update(self.state, "2.0.0")
        journal = m.json_file(self.manager.journal_file)
        self.assertEqual(journal["phase"], "candidate-touched-data")
        self.assertEqual(self.manager.state()["release"]["version"], "2.0.0")
        self.assertFalse(self.docker.containers["ember"]["State"]["Running"])
        self.assertFalse(self.docker.containers["ember-previous"]["State"]["Running"])
        self.assertTrue((self.manager.storage / "files" / "accepted-after-cutover").exists())
        self.assertTrue((self.manager.backups / journal["backup"] / "snapshot.tar.gz").exists())
        self.manager.readiness.side_effect = None
        self.manager.restore(self.manager.state(), journal["backup"], True)
        self.assertEqual(self.manager.state(), self.state)
        self.assertFalse((self.manager.storage / "files" / "accepted-after-cutover").exists())

    def test_successful_update_installs_checked_matching_manager_and_retains_backup(self):
        self.manager.update(self.state, "2.0.0")
        self.assertEqual(self.manager.state()["release"]["image"], self.next_release["image"])
        self.assertEqual((self.manager.lib / "emberctl.py").read_bytes(), (HERE / "emberctl.py").read_bytes())
        self.assertEqual(self.docker.containers["ember-previous"]["Config"]["Image"], self.state["release"]["image"])
        self.assertFalse(self.manager.journal_file.exists())
        self.assertEqual(len(list(self.manager.backups.iterdir())), 1)
        pull = next(i for i, event in enumerate(self.docker.events) if event[1] == "pull")
        stop = next(i for i, event in enumerate(self.docker.events) if event[1] == "stop")
        self.assertLess(pull, stop)

    def test_interrupted_update_archive_retains_pre_downtime_journal_without_backup_id(self):
        class ProcessInterrupted(BaseException):
            pass
        def interruption(state):
            journal = m.json_file(self.manager.journal_file)
            self.assertEqual(journal["phase"], "preparing-backup")
            self.assertNotIn("backup", journal)
            self.assertFalse(self.docker.containers["ember"]["State"]["Running"])
            raise ProcessInterrupted()
        with mock.patch.object(self.manager, "archive", side_effect=interruption):
            with self.assertRaises(ProcessInterrupted):
                self.manager.update(self.state, "2.0.0")
        journal = m.json_file(self.manager.journal_file)
        staged_bundle = Path(journal["bundle_dir"])
        self.assertTrue(staged_bundle.exists())
        self.assertEqual(self.manager.state(), self.state)
        self.assertEqual(list(self.manager.backups.iterdir()), [])
        self.manager.restart(self.manager.state())
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])
        self.assertFalse(self.manager.journal_file.exists())
        self.assertFalse(staged_bundle.exists())
        self.assertEqual(self.manager.state(), self.state)

    def test_update_archive_failure_resumes_old_and_retains_original_error(self):
        with mock.patch.object(self.manager, "archive", side_effect=m.Failure("original archive error")):
            with self.assertRaisesRegex(m.Failure, "original archive error"):
                self.manager.update(self.state, "2.0.0")
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])
        self.assertFalse(self.manager.journal_file.exists())
        self.assertEqual(self.manager.state(), self.state)

    def test_update_archive_error_is_not_hidden_if_old_restart_also_fails(self):
        self.docker.fail_start = True
        with mock.patch.object(self.manager, "archive", side_effect=m.Failure("original archive error")):
            with self.assertRaisesRegex(m.Failure, "original archive error"):
                self.manager.update(self.state, "2.0.0")
        self.assertEqual(m.json_file(self.manager.journal_file)["phase"], "preparing-backup")
        self.assertEqual(self.manager.state(), self.state)
        self.docker.fail_start = False
        self.manager.restart(self.manager.state())
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])
        self.assertFalse(self.manager.journal_file.exists())

    def test_interrupted_prestart_update_can_resume_old_safely(self):
        next_state = {**self.state, "release": release("2.0.0", "e")}
        self.docker.containers["ember-previous"] = self.docker.containers.pop("ember")
        self.docker.containers["ember-previous"]["State"]["Running"] = False
        self.docker.containers["ember"] = self.docker.container(next_state)
        m.atomic_json(self.manager.state_file, next_state)
        self.manager.journal("update", next_state, phase="candidate-created", previous=self.state)
        self.manager.restart(next_state)
        self.assertEqual(self.manager.state(), self.state)
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])

    def test_prestart_recovery_refuses_candidate_that_someone_already_started(self):
        next_state = {**self.state, "release": release("2.0.0", "e")}
        self.docker.containers["ember"] = self.docker.container(next_state, running=True)
        with self.assertRaisesRegex(m.Failure, "Candidate has started"):
            self.manager.recover_prestart({"state": next_state, "previous": self.state})
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])

    def test_archive_traversal_is_rejected_before_stopping_or_extracting(self):
        identifier = self.manager.backup(self.state)
        directory = self.manager.backups / identifier
        archive = directory / "snapshot.tar.gz"
        with tarfile.open(archive, "w:gz") as output:
            info = tarfile.TarInfo("../../outside")
            info.size = 5
            output.addfile(info, io.BytesIO(b"owned"))
        metadata = json.loads((directory / "backup.json").read_text())
        metadata["archive_sha256"] = m.digest_file(archive)
        m.atomic_json(directory / "backup.json", metadata)
        self.docker.events.clear()
        with self.assertRaisesRegex(m.Failure, "Unsafe backup archive path"):
            self.manager.restore(self.state, identifier, True)
        self.assertFalse(any(event[1] == "stop" for event in self.docker.events))
        self.assertFalse((self.manager.data.parent / "outside").exists())

    def test_backup_refuses_symlink_and_resumes_app(self):
        (self.manager.storage / "files" / "escape").symlink_to("/etc/passwd")
        with self.assertRaisesRegex(m.Failure, "links or special"):
            self.manager.backup(self.state)
        self.assertTrue(self.docker.containers["ember"]["State"]["Running"])

    def test_unmanaged_container_is_never_mutated(self):
        self.docker.containers["ember"]["Config"]["Labels"] = {}
        with self.assertRaisesRegex(m.Failure, "unmanaged"):
            self.manager.restart(self.state)
        self.assertFalse(any(event[1] in {"stop", "rm", "start"} for event in self.docker.events))

    def test_lock_excludes_concurrent_management(self):
        with self.manager.lock():
            with self.assertRaisesRegex(m.Failure, "Another Ember"):
                with self.manager.lock():
                    self.fail("second operation acquired lock")


class Contracts(unittest.TestCase):
    def test_test_trust_environment_is_explicit_and_cli_paths_take_precedence(self):
        command = ["install", "--bundle-dir", "/tmp/bundle"]
        with mock.patch.dict(m.os.environ, {}, clear=True):
            args = m.parser().parse_args(command)
            self.assertIsNone(args.runtime_env)
            self.assertIsNone(args.test_ca)
            self.assertIsNone(args.domain)
        env = {"EMBER_INSTALL_RUNTIME_ENV": "/root/runtime.env", "EMBER_INSTALL_TEST_CA": "/root/ca.pem", "EMBER_INSTALL_DOMAIN": "ignored.example.com"}
        with mock.patch.dict(m.os.environ, env, clear=True):
            args = m.parser().parse_args(command)
            self.assertEqual(args.runtime_env, Path("/root/runtime.env"))
            self.assertEqual(args.test_ca, Path("/root/ca.pem"))
            self.assertIsNone(args.domain)
            args = m.parser().parse_args(command + ["--runtime-env", "/root/explicit.env", "--test-ca", "/root/explicit.pem"])
            self.assertEqual(args.runtime_env, Path("/root/explicit.env"))
            self.assertEqual(args.test_ca, Path("/root/explicit.pem"))
        with mock.patch.dict(m.os.environ, {"EMBER_INSTALL_RUNTIME_ENV": "", "EMBER_INSTALL_TEST_CA": ""}, clear=True):
            args = m.parser().parse_args(command)
            self.assertIsNone(args.runtime_env)
            self.assertIsNone(args.test_ca)

    def test_commands_match_the_actual_dockerfile_without_entrypoint(self):
        dockerfile = (HERE.parents[1] / "Dockerfile").read_text()
        self.assertFalse(any(line.startswith("ENTRYPOINT") for line in dockerfile.splitlines()))
        source = (HERE / "emberctl.py").read_text()
        self.assertIn('state["release"]["image"], "ember", "generate-secrets"', source)
        self.assertIn('arguments.extend([state["release"]["image"], "ember", "server"])', source)

    def test_rejects_untrusted_images_and_mismatched_versions(self):
        for image in ("ghcr.io/attacker/ember@sha256:" + "a" * 64, m.IMAGE + ":latest", m.IMAGE + "@sha256:not-a-digest"):
            with self.assertRaises(m.Failure):
                m.validate_manifest({**release(), "image": image}, bundle=False)
        with self.assertRaises(m.Failure):
            m.validate_manifest(release(), "9.0.0", bundle=False)
        for version in ("../1.0.0", "1.0", "1.0.0-rc1", "01.0.0", "$(touch /tmp/owned)"):
            with self.assertRaises(m.Failure):
                m.validate_manifest(release(version), bundle=False)

    def test_hostname_cannot_inject_env_url_shell_or_port(self):
        for value in ("https://chat.example.com", "chat.example.com:443", "example.com/evil", "a\nSECRET=x", "127.0.0.1", "-bad.example.com"):
            with self.assertRaises(m.Failure):
                m.hostname(value)
        self.assertEqual(m.hostname("CHAT.Example.COM."), "chat.example.com")

    def test_optional_update_uses_stable_manifest(self):
        self.assertIsNone(m.parser().parse_args(["update"]).version)
        value = release()
        value["bundle"] = {"url": m.release_url("1.0.0", "ember-installer-1.0.0.tar.gz"), "sha256": "f" * 64}
        with mock.patch.object(m.urllib.request, "urlopen", return_value=Response(json.dumps(value).encode())) as network:
            self.assertEqual(m.download_manifest(), value)
        self.assertEqual(network.call_args.args[0].full_url, "https://get.nyllon.com/releases/stable.json")

    def test_reproducible_bundle_and_checksums(self):
        with tempfile.TemporaryDirectory() as temporary:
            outputs = [Path(temporary) / "one", Path(temporary) / "two"]
            for output in outputs:
                builder.build("1.0.0", "sha256:" + "a" * 64, "b" * 40, output)
            for path in outputs[0].iterdir():
                self.assertEqual(path.read_bytes(), (outputs[1] / path.name).read_bytes())
            manifest = json.loads((outputs[0] / "release.json").read_text())
            self.assertEqual(manifest["bundle"]["sha256"], m.digest_file(outputs[0] / "ember-installer-1.0.0.tar.gz"))

    def test_bootstrap_never_executes_a_bad_or_partial_download(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "built"
            builder.build("1.0.0", "sha256:" + "a" * 64, "b" * 40, output)
            binaries = root / "bin"
            binaries.mkdir()
            scripts = {
                "uname": "#!/bin/sh\necho Linux\n",
                "sh": "#!/bin/sh\necho INSTALL_CALLED\n",
                "curl": "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do if [ \"$1\" = -o ]; then shift; target=$1; fi; shift; done\n/bin/cp \"$MOCK_BUNDLE\" \"$target\"\nexit \"${MOCK_CURL_EXIT:-0}\"\n",
                "sha256sum": f"#!{sys.executable}\nimport hashlib,sys\nline=sys.stdin.read().strip()\nexpected,path=line.split(None,1)\nsys.exit(0 if hashlib.sha256(open(path,'rb').read()).hexdigest()==expected else 1)\n",
            }
            for name, script in scripts.items():
                (binaries / name).write_text(script)
                (binaries / name).chmod(0o755)
            source = (output / "bootstrap.sh").read_text().replace("PATH=/usr/sbin:/usr/bin:/sbin:/bin", f"PATH={binaries}:/usr/bin:/bin")
            bootstrap = root / "bootstrap.sh"
            bootstrap.write_text(source)
            env = {**os.environ, "MOCK_BUNDLE": str(output / "ember-installer-1.0.0.tar.gz")}
            success = subprocess.run(["/bin/sh", str(bootstrap)], env=env, text=True, capture_output=True)
            self.assertEqual(success.returncode, 0, success.stderr)
            self.assertIn("INSTALL_CALLED", success.stdout)
            corrupt = root / "corrupt"
            corrupt.write_bytes(b"bad bundle")
            for extra in ({"MOCK_BUNDLE": str(corrupt)}, {"MOCK_CURL_EXIT": "22"}):
                failure = subprocess.run(["/bin/sh", str(bootstrap)], env={**env, **extra}, text=True, capture_output=True)
                self.assertNotEqual(failure.returncode, 0)
                self.assertNotIn("INSTALL_CALLED", failure.stdout)
            bootstrap.write_text(source[:source.index('  sh "$ember_tmp/install.sh"')])
            failure = subprocess.run(["/bin/sh", str(bootstrap)], env=env, text=True, capture_output=True)
            self.assertNotEqual(failure.returncode, 0)
            self.assertNotIn("INSTALL_CALLED", failure.stdout)


if __name__ == "__main__":
    unittest.main()
