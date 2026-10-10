#!/usr/bin/env python3
"""Root-owned management for one fresh Ember install; Python standard library only."""
import argparse
import contextlib
import datetime
import fcntl
import getpass
import hashlib
import http.client
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import socket
import ssl
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.parse
import urllib.request
import uuid

REPOSITORY = "NYLLON-SOFTWARE/ember"
IMAGE = "ghcr.io/nyllon-software/ember"
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z")
DIGEST = re.compile(re.escape(IMAGE) + r"@sha256:[0-9a-f]{64}\Z")
BACKUP_ID = re.compile(r"[0-9]{8}T[0-9]{6}Z-[0-9a-f]{8}\Z")
ROOT_UID = 0
APP_UID = 1000
APP_GID = 1000
SECRET_KEYS = {"SECRET_KEY_BASE", "VAPID_PUBLIC_KEY", "VAPID_PRIVATE_KEY", "EMBER_SETUP_TOKEN"}
LABEL = "com.nyllon.ember.managed"
INSTALL_LABEL = "com.nyllon.ember.install-id"


class Failure(Exception):
    pass


def require(condition, message):
    if not condition:
        raise Failure(message)


def run(args, *, capture=True, input=None, check=True):
    environment = dict(os.environ)
    environment["PATH"] = "/usr/sbin:/usr/bin:/sbin:/bin"
    environment["DOCKER_HOST"] = "unix:///var/run/docker.sock"
    for key in ("DOCKER_CONTEXT", "DOCKER_TLS_VERIFY", "DOCKER_CERT_PATH"):
        environment.pop(key, None)
    result = subprocess.run([str(v) for v in args], input=input, text=True, env=environment,
                            stdout=subprocess.PIPE if capture else None,
                            stderr=subprocess.PIPE if capture else None)
    if check and result.returncode:
        # Never include stdout: generate-secrets and inspect contain private material.
        raise Failure(f"Command failed ({result.returncode}): {' '.join(str(v) for v in args[:3])}\n{result.stderr or ''}".strip())
    return result


def digest_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def durable_rename(source, destination):
    os.rename(source, destination)
    for parent in {source.parent, destination.parent}:
        sync_directory(parent)


def atomic_json(path, value):
    atomic_bytes(path, (json.dumps(value, sort_keys=True, indent=2) + "\n").encode())


def atomic_bytes(path, data, mode=0o600):
    descriptor, temporary = tempfile.mkstemp(prefix=".ember-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as target:
            target.write(data)
            target.flush()
            os.fsync(target.fileno())
        os.chmod(temporary, mode)
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(temporary)


def json_file(path):
    require(path.stat().st_size <= 65536, f"Oversized metadata: {path}")
    try:
        return json.loads(path.read_text())
    except (ValueError, UnicodeError) as error:
        raise Failure(f"Invalid JSON: {path}") from error


def validate_manifest(value, version=None, bundle=True):
    require(isinstance(value, dict), "Release metadata must be an object.")
    keys = {"schema", "version", "source_sha", "image"} | ({"bundle"} if bundle else set())
    require(set(value) == keys and value["schema"] == 1, "Unsupported release metadata schema.")
    require(isinstance(value["version"], str) and VERSION.fullmatch(value["version"]), "Expected a stable semantic version (for example 1.0.0).")
    require(version is None or value["version"] == version, "Release version does not match requested version.")
    require(isinstance(value["source_sha"], str) and re.fullmatch(r"[0-9a-f]{40}", value["source_sha"]), "Invalid source commit.")
    require(isinstance(value["image"], str) and DIGEST.fullmatch(value["image"]), "Release image must use the NYLLON repository and an immutable SHA256 digest.")
    if bundle:
        expected = release_url(value["version"], f"ember-installer-{value['version']}.tar.gz")
        require(isinstance(value["bundle"], dict) and set(value["bundle"]) == {"url", "sha256"}, "Invalid bundle metadata.")
        require(value["bundle"]["url"] == expected and isinstance(value["bundle"]["sha256"], str)
                and re.fullmatch(r"[0-9a-f]{64}", value["bundle"]["sha256"]), "Invalid first-party bundle URL or checksum.")
    return value


def release_url(version, name):
    return f"https://github.com/{REPOSITORY}/releases/download/v{version}/{name}"


def download_manifest(version=None):
    require(version is None or VERSION.fullmatch(version), "Expected a stable semantic version (for example 1.0.0).")
    request = urllib.request.Request(release_url(version, "release.json") if version else "https://get.nyllon.com/releases/stable.json", headers={"User-Agent": "emberctl/1"})
    with urllib.request.urlopen(request, timeout=30) as response:
        require(urllib.parse.urlsplit(response.url).scheme == "https", "Release download redirected outside HTTPS.")
        data = response.read(65537)
    require(len(data) <= 65536, "Oversized release metadata.")
    try:
        value = json.loads(data)
    except (ValueError, UnicodeError) as error:
        raise Failure("Invalid release metadata JSON.") from error
    return validate_manifest(value, version)


def hostname(value):
    require(isinstance(value, str), "A DNS hostname is required.")
    value = value.lower().rstrip(".")
    labels = value.split(".")
    require(len(value) <= 253 and len(labels) >= 2 and all(re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label) for label in labels),
            "Enter a DNS hostname such as chat.example.com, without a scheme, port, or path.")
    require(not re.fullmatch(r"[0-9.]+", value), "A DNS hostname is required, rather than an IP address.")
    return value


def private(path, directory=False):
    information = path.lstat()
    require(not stat.S_ISLNK(information.st_mode), f"Refusing symbolic link: {path}")
    require(information.st_uid == ROOT_UID and information.st_mode & 0o077 == 0, f"Expected root-private permissions: {path}")
    require(stat.S_ISDIR(information.st_mode) if directory else stat.S_ISREG(information.st_mode), f"Unexpected file type: {path}")


class Manager:
    def __init__(self, config=Path("/etc/ember"), data=Path("/var/lib/ember"), lib=Path("/usr/local/lib/ember"), command=Path("/usr/local/bin/emberctl")):
        self.config, self.data, self.lib, self.command = config, data, lib, command
        self.storage = data / "storage"
        self.backups = data / "backups"
        self.state_file = config / "state.json"
        self.journal_file = data / "recovery.json"

    @contextlib.contextmanager
    def lock(self):
        self.data.mkdir(mode=0o700, parents=True, exist_ok=True)
        private(self.data, True)
        descriptor = os.open(self.data / "manager.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        try:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise Failure("Another Ember management operation is running.") from error
            yield
        finally:
            os.close(descriptor)

    def state(self, recovery=False):
        if self.state_file.exists():
            private(self.config, True)
            private(self.state_file)
            value = json_file(self.state_file)
        elif recovery and self.journal_file.exists():
            private(self.journal_file)
            value = json_file(self.journal_file)["state"]
        else:
            raise Failure("No managed Ember installation. Existing Campfire/ONCE installs require the separate migration procedure.")
        require(isinstance(value, dict) and value.get("managed_by") == "emberctl-v1", "Refusing unmanaged installation.")
        validate_manifest(value.get("release"), bundle=False)
        require(isinstance(value.get("install_id"), str) and re.fullmatch(r"[0-9a-f]{32}", value["install_id"]), "Invalid installation identity.")
        hostname(value.get("domain", ""))
        require(value.get("storage") == str(self.storage), "Storage path differs from this managed installation.")
        return value

    def journal(self, operation, state, **extra):
        atomic_json(self.journal_file, {"schema": 1, "operation": operation, "state": state, **extra})

    def no_pending(self):
        require(not self.journal_file.exists(), "An interrupted/failed operation needs attention. Run emberctl status, restart the current candidate, or explicitly restore a backup; no automatic downgrade is safe.")

    def inspect(self, name="ember"):
        result = run(["docker", "container", "inspect", name], check=False)
        if result.returncode:
            return None
        value = json.loads(result.stdout)
        require(isinstance(value, list) and len(value) == 1, "Unexpected Docker inspect result.")
        return value[0]

    def managed_container(self, state, name="ember", required=False, exact=True):
        container = self.inspect(name)
        require(container is not None or not required, f"Managed container {name} is absent; use emberctl restart or restore.")
        if container is None:
            return None
        labels = container.get("Config", {}).get("Labels") or {}
        require(labels.get(LABEL) == "1" and labels.get(INSTALL_LABEL) == state["install_id"], f"Refusing unmanaged Docker container {name}.")
        mounts = container.get("Mounts", [])
        require(any(m.get("Type") == "bind" and m.get("Source") == str(self.storage) and m.get("Destination") == "/rails/storage" for m in mounts), "Container storage does not match managed state.")
        if exact:
            require(container["Config"]["Image"] == state["release"]["image"], "Container image differs from managed state; inspect recovery status before changing anything.")
        return container

    def runtime(self):
        path = self.config / "runtime.json"
        private(path)
        value = json_file(path)
        require(isinstance(value, dict) and set(value) <= {"ACME_DIRECTORY", "SSL_CERT_FILE"}, "Unsupported runtime options.")
        if value:
            require(set(value) == {"ACME_DIRECTORY", "SSL_CERT_FILE"} and value["SSL_CERT_FILE"] == "/run/ember-test-ca.pem", "Explicit ACME test options must include the fixed CA mount.")
            parsed = urllib.parse.urlsplit(value["ACME_DIRECTORY"])
            require(parsed.scheme == "https" and parsed.hostname and not parsed.username and not parsed.password, "ACME_DIRECTORY must be an HTTPS URL.")
            information = (self.config / "test-ca.pem").lstat()
            require(stat.S_ISREG(information.st_mode) and information.st_uid == ROOT_UID and information.st_mode & 0o022 == 0, "Expected a root-owned, read-only CA bundle.")
        return value

    def pull(self, image):
        run(["docker", "pull", image], capture=False)
        run(["docker", "image", "inspect", image])

    def create(self, state):
        arguments = ["docker", "create", "--name", "ember", "--label", f"{LABEL}=1", "--label", f"{INSTALL_LABEL}={state['install_id']}",
                     "--restart", "unless-stopped", "--stop-timeout", "60", "--user", "1000:1000", "--log-driver", "local", "--log-opt", "max-size=10m", "--log-opt", "max-file=3",
                     "-p", "80:80", "-p", "443:443", "--mount", f"type=bind,src={self.storage},dst=/rails/storage", "--env-file", str(self.config / "app.env")]
        for key, value in sorted(self.runtime().items()):
            arguments.extend(["--env", f"{key}={value}"])
        if self.runtime():
            arguments.extend(["--mount", f"type=bind,src={self.config / 'test-ca.pem'},dst=/run/ember-test-ca.pem,readonly"])
        arguments.extend([state["release"]["image"], "ember", "server"])
        run(arguments)

    def readiness(self, state, timeout=180):
        context = ssl.create_default_context()
        if self.runtime():
            context.load_verify_locations(str(self.config / "test-ca.pem"))
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            container = self.managed_container(state, required=True)
            require(container["State"]["Running"], "Ember exited before HTTPS readiness. Check emberctl logs.")
            try:
                # Connect locally while validating the public hostname and certificate chain.
                # Public DNS might otherwise point at a different, already healthy server.
                with socket.create_connection(("127.0.0.1", 443), timeout=5) as raw:
                    with context.wrap_socket(raw, server_hostname=state["domain"]) as connection:
                        connection.sendall(f"GET /up HTTP/1.1\r\nHost: {state['domain']}\r\nConnection: close\r\n\r\n".encode("ascii"))
                        response = http.client.HTTPResponse(connection)
                        response.begin()
                        if response.status == 200:
                            return
            except (OSError, ValueError, http.client.HTTPException):
                pass
            time.sleep(2)
        raise Failure("HTTPS readiness timed out. Verify DNS points to this server and inbound TCP 80/443 are reachable; run emberctl logs and emberctl restart after correcting them.")

    def stop(self, state):
        container = self.managed_container(state)
        if container is not None and container["State"]["Running"]:
            # Docker waits for the app's graceful shutdown, then terminates at the configured deadline.
            run(["docker", "stop", "--time", "60", "ember"])
            require(not self.managed_container(state, required=True)["State"]["Running"], "Container did not stop; no backup was taken.")
            return True
        return False

    def start(self, state):
        if self.managed_container(state) is None:
            self.create(state)
        run(["docker", "start", "ember"])

    def setup_status(self, state):
        self.managed_container(state, required=True)
        status = run(["docker", "exec", "ember", "ember", "setup-status"]).stdout.strip()
        require(status in {"pending", "initialized"}, "Unexpected setup status from the app.")
        return status

    def setup_link(self, state):
        require(self.setup_status(state) == "pending", "Initial administrator setup is complete; the private setup link no longer grants access.")
        self.print_setup_link(state)

    def print_setup_link(self, state):
        private(self.config / "app.env")
        values = dict(line.split("=", 1) for line in (self.config / "app.env").read_text().splitlines())
        token = values.get("EMBER_SETUP_TOKEN", "")
        require(re.fullmatch(r"[0-9a-f]{64}", token), "No valid managed setup token.")
        print(f"https://{state['domain']}/first_run/access#token={token}")

    def preflight(self, domain):
        if self.journal_file.exists():
            private(self.journal_file)
            if json_file(self.journal_file).get("operation") == "install":
                return self.state(recovery=True)["domain"]
        if self.state_file.exists():
            return self.state()["domain"]
        require(not self.config.exists() and not self.storage.exists() and not self.command.exists() and not self.lib.exists(), "Refusing unmanaged or incomplete installation paths.")
        if self.data.exists():
            private(self.data, True)
            require(not any(path.name != "manager.lock" for path in self.data.iterdir()), "Existing Ember data requires migration or recovery; fresh installation refused.")
        value = domain
        if not value:
            try:
                # A terminal is not seekable, so buffered read/write (r+) is unsupported.
                with open("/dev/tty", "r") as reader, open("/dev/tty", "w") as writer:
                    writer.write("Hostname pointing to this server (for example chat.example.com): ")
                    writer.flush()
                    value = reader.readline().strip()
            except OSError as error:
                raise Failure("An interactive terminal or --domain HOSTNAME is required.") from error
        domain = hostname(value)
        try:
            socket.getaddrinfo(domain, 443)
        except OSError as error:
            raise Failure("Hostname does not resolve yet. Point its DNS records to this server and retry.") from error
        for port in (80, 443):
            for family, address in ((socket.AF_INET, "0.0.0.0"), (socket.AF_INET6, "::")):
                try:
                    listener = socket.socket(family, socket.SOCK_STREAM)
                except OSError:
                    if family == socket.AF_INET6:
                        continue
                    raise
                with listener:
                    if family == socket.AF_INET6:
                        listener.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
                    try:
                        listener.bind((address, port))
                    except OSError as error:
                        if family == socket.AF_INET6 and error.errno in {47, 97, 99}:
                            continue  # IPv6 is disabled on this host.
                        raise Failure(f"TCP port {port} is already occupied or unavailable; stop the conflicting service explicitly.") from error
        return domain

    def install(self, args):
        release = validate_manifest(json_file(args.bundle_dir / "install.json"), bundle=False)
        if self.journal_file.exists():
            private(self.journal_file)
            pending = json_file(self.journal_file)
            if pending.get("operation") == "install":
                require(pending["state"]["release"] == release, "Finish the pending installation using its original versioned bootstrap or emberctl restart before changing versions.")
                self.resume_install(pending, args.bundle_dir)
                return
        if self.state_file.exists():
            self.no_pending()
            state = self.state()
            self.managed_container(state)
            print(f"Ember {state['release']['version']} is already managed at https://{state['domain']}. Use emberctl update VERSION for upgrades.")
            return
        self.no_pending()
        domain = self.preflight(args.domain)
        require(self.inspect() is None and self.inspect("ember-previous") is None, "Reserved Docker container name is already in use.")
        # Check Docker port bindings too: published ports may use NAT without a listening process.
        all_names = run(["docker", "ps", "-a", "--format", "{{.Names}}"] ).stdout.splitlines()
        for name in all_names:
            item = self.inspect(name) or {}
            image = item.get("Config", {}).get("Image", "").lower()
            labels = item.get("Config", {}).get("Labels") or {}
            require(not any(part in image for part in ("campfire", "basecamp/once", "nyllon-software/ember"))
                    and not any(re.search(r"(?:^|[._-])(?:once|campfire|ember)(?:$|[._-])", key.lower()) for key in labels)
                    and not re.search(r"(?:^|[-_])(?:once|campfire|ember)(?:$|[-_])", name.lower()),
                    "An existing ONCE/Campfire/Ember container requires the migration procedure; fresh installation refused.")
        volumes = run(["docker", "volume", "ls", "--format", "{{.Name}}"] ).stdout.splitlines()
        for volume in volumes:
            require(not re.search(r"(?:^|[-_])(?:once|campfire|ember)(?:$|[-_])", volume.lower()), "Existing ONCE/Campfire/Ember storage requires migration; fresh installation refused.")
            information = json.loads(run(["docker", "volume", "inspect", volume]).stdout)
            require(isinstance(information, list) and len(information) == 1, "Unexpected Docker volume metadata.")
            labels = information[0].get("Labels") or {}
            require(not any(re.search(r"(?:^|[._-])(?:once|campfire|ember)(?:$|[._-])", key.lower()) for key in labels), "Existing labelled ONCE/Campfire/Ember volume requires migration; fresh installation refused.")
        names = run(["docker", "ps", "--format", "{{.Names}}"] ).stdout.splitlines()
        for name in names:
            ports = (self.inspect(name) or {}).get("HostConfig", {}).get("PortBindings") or {}
            require(not any(binding.get("HostPort") in {"80", "443"} for bindings in ports.values() for binding in (bindings or [])), "Another Docker container publishes TCP 80 or 443.")
        options = {}
        if args.runtime_env or args.test_ca:
            require(args.runtime_env and args.test_ca, "--runtime-env and --test-ca must be provided together.")
            require(not args.runtime_env.is_symlink() and not args.test_ca.is_symlink(), "Runtime option files must not be symlinks.")
            options = dict(line.split("=", 1) for line in args.runtime_env.read_text().splitlines() if line)
            require(set(options) == {"ACME_DIRECTORY", "SSL_CERT_FILE"} and options["SSL_CERT_FILE"] == "/run/ember-test-ca.pem", "Only ACME_DIRECTORY and fixed SSL_CERT_FILE runtime options are accepted.")
            require(options["ACME_DIRECTORY"].startswith("https://"), "Test ACME directory must use HTTPS.")
            require(args.test_ca.stat().st_size <= 1024 * 1024, "CA bundle exceeds 1 MiB.")
            ssl.create_default_context(cafile=str(args.test_ca))
        self.pull(release["image"])
        state = {"managed_by": "emberctl-v1", "install_id": uuid.uuid4().hex, "domain": domain, "storage": str(self.storage), "release": release}
        staged_config = self.config.parent / (".ember-install-" + state["install_id"])
        staged_storage = self.data / (".storage-install-" + state["install_id"])
        require(not os.path.lexists(staged_config) and not os.path.lexists(staged_storage), "Installation staging paths already exist.")
        details = {"staged_config": str(staged_config), "staged_storage": str(staged_storage), "runtime": options,
                   "manager_checksums": {name: digest_file(args.bundle_dir / name) for name in ("emberctl", "emberctl.py")}}
        if args.test_ca:
            details.update(test_ca_source=str(args.test_ca.resolve()), test_ca_sha256=digest_file(args.test_ca))
        # The future paths are durable before the first mkdir or secret generation.
        self.journal("install", state, phase="preparing", **details)
        self.resume_install(json_file(self.journal_file), args.bundle_dir)

    def prepare_install(self, journal, bundle_dir):
        state = self.state(recovery=True)
        require(state == journal["state"], "Installation identity differs from its recovery journal.")
        require(not any(os.path.lexists(path) for path in (self.config, self.storage, self.lib, self.command)), "Unmanaged installation paths appeared during preparation.")
        require(bundle_dir is not None, "Finish preparing this installation with its original versioned bootstrap.")
        require(validate_manifest(json_file(bundle_dir / "install.json"), bundle=False) == state["release"], "Recovery bundle differs from the pending installation.")
        options = journal.get("runtime")
        require(isinstance(options, dict) and set(options) in (set(), {"ACME_DIRECTORY", "SSL_CERT_FILE"}), "Invalid preparing runtime options.")
        if options:
            parsed = urllib.parse.urlsplit(options["ACME_DIRECTORY"])
            require(parsed.scheme == "https" and parsed.hostname and not parsed.username and not parsed.password
                    and options["SSL_CERT_FILE"] == "/run/ember-test-ca.pem", "Invalid preparing ACME options.")
        checksums = journal.get("manager_checksums")
        require(isinstance(checksums, dict) and set(checksums) == {"emberctl", "emberctl.py"}
                and all(digest_file(bundle_dir / name) == expected for name, expected in checksums.items()), "Recovery manager files differ from the original bundle.")
        stages = []
        for key, parent, prefix, owners in (("staged_config", self.config.parent, ".ember-install-", {ROOT_UID}),
                                            ("staged_storage", self.data, ".storage-install-", {ROOT_UID, APP_UID})):
            stage = Path(journal[key])
            require(stage.parent == parent and stage.name == prefix + state["install_id"], "Unsafe preparing installation path.")
            if os.path.lexists(stage):
                information = stage.lstat()
                require(stat.S_ISDIR(information.st_mode) and information.st_uid in owners and information.st_mode & 0o077 == 0, "Unsafe preparing installation directory.")
            else:
                stage.mkdir(mode=0o700)
                sync_directory(parent)
            stages.append(stage)
        staged_config, staged_storage = stages
        os.chown(staged_storage, APP_UID, APP_GID)
        sync_directory(staged_storage)
        environment = staged_config / "app.env"
        if os.path.lexists(environment):
            private(environment)
            lines = environment.read_text().splitlines()
            values = dict(line.split("=", 1) for line in lines)
            require(len(lines) == len(SECRET_KEYS) + 2 and set(values) == SECRET_KEYS | {"TLS_DOMAIN", "EMBER_STORAGE_PATH"}
                    and values["TLS_DOMAIN"] == state["domain"] and values["EMBER_STORAGE_PATH"] == "/rails/storage", "Invalid staged installation environment.")
            secrets = {key: values[key] for key in SECRET_KEYS}
        else:
            generated = run(["docker", "run", "--rm", "--network", "none", "--log-driver", "none", "--user", "1000:1000", state["release"]["image"], "ember", "generate-secrets"]).stdout
            try:
                secrets = dict(line.split("=", 1) for line in generated.splitlines())
            except ValueError as error:
                raise Failure("Unexpected secret generator output.") from error
        require(set(secrets) == SECRET_KEYS and re.fullmatch(r"[0-9a-f]{128}", secrets["SECRET_KEY_BASE"])
                and re.fullmatch(r"[0-9a-f]{64}", secrets["EMBER_SETUP_TOKEN"])
                and re.fullmatch(r"[A-Za-z0-9_-]{87}", secrets["VAPID_PUBLIC_KEY"])
                and re.fullmatch(r"[A-Za-z0-9_-]{43}", secrets["VAPID_PRIVATE_KEY"]), "Unexpected secret generator output.")
        if not environment.exists():
            atomic_bytes(environment, ("\n".join(f"{key}={value}" for key, value in sorted(secrets.items()))
                         + f"\nTLS_DOMAIN={state['domain']}\nEMBER_STORAGE_PATH=/rails/storage\n").encode())
        if options:
            certificate = staged_config / "test-ca.pem"
            if os.path.lexists(certificate):
                information = certificate.lstat()
                require(stat.S_ISREG(information.st_mode) and information.st_uid == ROOT_UID and information.st_mode & 0o022 == 0, "Unsafe staged test CA.")
                require(digest_file(certificate) == journal["test_ca_sha256"], "Staged test CA differs from its preparing journal.")
            else:
                source = Path(journal["test_ca_source"])
                require(not source.is_symlink() and source.stat().st_size <= 1024 * 1024 and digest_file(source) == journal["test_ca_sha256"], "Original test CA changed during preparation.")
                ssl.create_default_context(cafile=str(source))
                atomic_bytes(certificate, source.read_bytes(), 0o644)
        for name, value in (("runtime.json", options), ("state.json", state)):
            target = staged_config / name
            if os.path.lexists(target):
                private(target)
                require(json_file(target) == value, "Staged configuration differs from its preparing journal.")
            else:
                atomic_json(target, value)
        saved = staged_config / "manager"
        if os.path.lexists(saved):
            private(saved, True)
        else:
            saved.mkdir(mode=0o700)
            sync_directory(staged_config)
        for name in ("emberctl", "emberctl.py"):
            target = saved / name
            if os.path.lexists(target):
                private(target)
                require(digest_file(target) == checksums[name], "Staged manager differs from the original bundle.")
            else:
                atomic_bytes(target, (bundle_dir / name).read_bytes())
        self.journal("install", state, phase="prepared", staged_config=str(staged_config), staged_storage=str(staged_storage))

    def resume_install(self, journal, bundle_dir=None):
        if journal.get("phase") == "preparing":
            self.prepare_install(journal, bundle_dir)
            journal = json_file(self.journal_file)
        state = journal["state"]
        validate_manifest(state["release"], bundle=False)
        require(state.get("storage") == str(self.storage) and state.get("managed_by") == "emberctl-v1", "Invalid install recovery identity.")
        if not self.config.exists():
            stage = Path(journal["staged_config"])
            require(stage.parent == self.config.parent and stage.name.startswith(".ember-install-"), "Unsafe staged configuration path.")
            private(stage, True)
            require(json_file(stage / "state.json") == state, "Staged configuration identity changed.")
            durable_rename(stage, self.config)
        require(self.state() == state, "Installed configuration differs from pending installation.")
        if not self.storage.exists():
            stage = Path(journal["staged_storage"])
            require(stage.parent == self.data and stage.name.startswith(".storage-install-"), "Unsafe staged storage path.")
            information = stage.lstat()
            require(stat.S_ISDIR(information.st_mode) and information.st_uid == APP_UID and information.st_mode & 0o077 == 0, "Invalid staged storage permissions.")
            durable_rename(stage, self.storage)
        information = self.storage.lstat()
        require(stat.S_ISDIR(information.st_mode) and information.st_uid == APP_UID, "Invalid managed storage directory.")
        self.backups.mkdir(mode=0o700, exist_ok=True)
        private(self.backups, True)
        self.runtime()
        files = {}
        for name in ("emberctl", "emberctl.py"):
            private(self.config / "manager" / name)
            files[name] = (self.config / "manager" / name).read_bytes()
        self.install_manager(files)
        self.journal("install", state, phase="starting", **{key: journal[key] for key in ("staged_config", "staged_storage") if key in journal})
        container = self.managed_container(state)
        if container is None:
            self.create(state)
        if container is None or not container["State"]["Running"]:
            self.start(state)
        self.readiness(state)
        status = self.setup_status(state)
        self.journal_file.unlink()
        if status == "pending":
            print(f"Ember {state['release']['version']} is ready. Open this private link to create the first administrator:")
            self.print_setup_link(state)
            print("Keep the link private. It stops granting setup access once an administrator exists.")
        else:
            print(f"Ember {state['release']['version']} is ready; initial administrator setup is already complete.")

    def install_manager(self, files):
        saved = self.config / "manager"
        saved.mkdir(mode=0o700, exist_ok=True)
        self.lib.mkdir(mode=0o755, parents=True, exist_ok=True)
        for name in ("emberctl.py", "emberctl"):
            atomic_bytes(saved / name, files[name])
        atomic_bytes(self.lib / "emberctl.py", files["emberctl.py"], 0o644)
        atomic_bytes(self.command, files["emberctl"], 0o755)

    def space_for_backup(self):
        required = 0
        for root in (self.storage, self.config):
            for path in [root, *root.rglob("*")]:
                info = path.lstat()
                require(stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode), f"Backup refuses links or special files: {path}")
                required += info.st_size if stat.S_ISREG(info.st_mode) else 0
        require(shutil.disk_usage(self.data).free > required + 256 * 1024 * 1024, "Insufficient disk space for a full backup plus 256 MiB reserve.")

    def stage_bundle(self, release):
        request = urllib.request.Request(release["bundle"]["url"], headers={"User-Agent": "emberctl/1"})
        directory = Path(tempfile.mkdtemp(prefix=".update-bundle-", dir=self.data))
        try:
            archive = directory / "bundle.tar.gz"
            with urllib.request.urlopen(request, timeout=60) as response, archive.open("wb") as output:
                require(urllib.parse.urlsplit(response.url).scheme == "https", "Bundle download redirected outside HTTPS.")
                total = 0
                for block in iter(lambda: response.read(65536), b""):
                    total += len(block)
                    require(total <= 10 * 1024 * 1024, "Installer bundle exceeds 10 MiB.")
                    output.write(block)
            require(digest_file(archive) == release["bundle"]["sha256"], "Installer bundle SHA256 mismatch.")
            files = {}
            with tarfile.open(archive, "r:gz") as source:
                for member in source:
                    require(member.name in {"install.sh", "install.json", "emberctl", "emberctl.py"} and member.name not in files
                            and member.isfile() and not member.issym() and not member.islnk() and member.size <= 2 * 1024 * 1024,
                            "Unsafe installer bundle member.")
                    with source.extractfile(member) as contents:
                        files[member.name] = contents.read()
            require(set(files) == {"install.sh", "install.json", "emberctl", "emberctl.py"}, "Incomplete installer bundle.")
            identity = {key: release[key] for key in ("schema", "version", "source_sha", "image")}
            require(json.loads(files["install.json"]) == identity, "Bundle identity differs from release manifest.")
            checksums = {}
            for name in ("emberctl", "emberctl.py"):
                atomic_bytes(directory / name, files[name])
                checksums[name] = digest_file(directory / name)
            archive.unlink()
            return directory, checksums
        except BaseException:
            shutil.rmtree(directory)
            raise

    def finish_manager_update(self, journal):
        directory = Path(journal["bundle_dir"])
        require(directory.parent == self.data and directory.name.startswith(".update-bundle-"), "Unsafe staged bundle path.")
        private(directory, True)
        files = {}
        for name in ("emberctl", "emberctl.py"):
            private(directory / name)
            require(digest_file(directory / name) == journal["manager_checksums"][name], "Staged manager checksum mismatch.")
            files[name] = (directory / name).read_bytes()
        self.install_manager(files)
        atomic_json(self.journal_file, {**journal, "phase": "manager-installed"})

    def assert_storage_quiescent(self):
        names = run(["docker", "ps", "--format", "{{.Names}}"] ).stdout.splitlines()
        for name in names:
            item = self.inspect(name) or {}
            for mount in item.get("Mounts", []):
                if mount.get("Type") != "bind" or mount.get("RW") is False:
                    continue
                source = Path(mount.get("Source", "/")).resolve()
                storage = self.storage.resolve()
                require(source != storage and source not in storage.parents and storage not in source.parents,
                        f"Running container {name} can write managed storage; stop it before taking/restoring a cold backup.")

    def archive(self, state):
        self.assert_storage_quiescent()
        self.backups.mkdir(mode=0o700, exist_ok=True)
        private(self.backups, True)
        identifier = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ-") + uuid.uuid4().hex[:8]
        directory = self.backups / identifier
        directory.mkdir(mode=0o700)
        entries = {}
        archive = directory / "snapshot.tar.gz"
        try:
            with tarfile.open(archive, "w:gz", dereference=True) as target:
                for prefix, source in (("config", self.config), ("storage", self.storage)):
                    for path in [source, *sorted(source.rglob("*"))]:
                        info = path.lstat()
                        require(stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode), f"Backup refuses links or special files: {path}")
                        name = str(PurePosixPath(prefix) / path.relative_to(source).as_posix())
                        record = {"kind": "directory" if path.is_dir() else "file", "mode": stat.S_IMODE(info.st_mode), "uid": info.st_uid, "gid": info.st_gid, "size": info.st_size if path.is_file() else 0}
                        if path.is_file():
                            record["sha256"] = digest_file(path)
                        entries[name] = record
                        target.add(path, arcname=name, recursive=False)
            os.chmod(archive, 0o600)
            with archive.open("r+b") as output:
                os.fsync(output.fileno())
            atomic_json(directory / "backup.json", {"schema": 1, "id": identifier, "state": state, "archive_sha256": digest_file(archive), "entries": entries})
            self.validate_backup(identifier, state)
            sync_directory(self.backups)
        except BaseException:
            shutil.rmtree(directory)
            raise
        return identifier

    def validate_backup(self, identifier, state):
        require(BACKUP_ID.fullmatch(identifier), "Pass a backup ID printed by emberctl backup; arbitrary archive paths are refused.")
        directory = self.backups / identifier
        private(directory, True)
        private(directory / "backup.json")
        private(directory / "snapshot.tar.gz")
        metadata_path = directory / "backup.json"
        # Per-file checksums can be larger than the release metadata limit.
        require(metadata_path.stat().st_size <= 64 * 1024 * 1024, "Oversized backup metadata.")
        metadata = json.loads(metadata_path.read_text())
        require(isinstance(metadata, dict) and metadata.get("schema") == 1 and metadata.get("id") == identifier, "Invalid backup metadata.")
        saved = metadata.get("state", {})
        require(saved.get("install_id") == state["install_id"] and saved.get("managed_by") == "emberctl-v1" and saved.get("storage") == str(self.storage), "Backup belongs to another installation.")
        validate_manifest(saved.get("release"), bundle=False)
        hostname(saved.get("domain", ""))
        archive = directory / "snapshot.tar.gz"
        require(digest_file(archive) == metadata.get("archive_sha256"), "Backup archive SHA256 mismatch.")
        entries = metadata.get("entries")
        require(isinstance(entries, dict), "Invalid backup entries.")
        seen = set()
        with tarfile.open(archive, "r:gz") as source:
            for member in source:
                name = member.name
                path = PurePosixPath(name)
                require(bool(path.parts) and not path.is_absolute() and ".." not in path.parts and str(path) == name and path.parts[0] in {"config", "storage"}, "Unsafe backup archive path.")
                require(name not in seen and (member.isfile() or member.isdir()) and not member.issym() and not member.islnk(), "Backup has duplicate paths, links, or special files.")
                seen.add(name)
                record = entries.get(name)
                require(isinstance(record, dict) and record.get("kind") == ("file" if member.isfile() else "directory")
                        and record.get("size") == member.size and record.get("mode") == member.mode and record.get("uid") == member.uid and record.get("gid") == member.gid,
                        "Backup member metadata mismatch.")
                require(member.mode & 0o7000 == 0, "Backup contains special permission bits.")
                require((member.uid == ROOT_UID and (member.mode & 0o077 == 0 or (name == "config/test-ca.pem" and member.mode == 0o644))) if path.parts[0] == "config" else member.uid in {ROOT_UID, APP_UID}, "Unexpected backup ownership or configuration permissions.")
                if member.isfile():
                    digest = hashlib.sha256()
                    with source.extractfile(member) as contents:
                        for block in iter(lambda: contents.read(1024 * 1024), b""):
                            digest.update(block)
                    require(digest.hexdigest() == record.get("sha256"), "Backup file SHA256 mismatch.")
        require(seen == set(entries) and {"config", "storage", "config/state.json", "config/app.env", "config/runtime.json", "config/manager", "config/manager/emberctl", "config/manager/emberctl.py"} <= seen, "Backup is incomplete.")
        return metadata

    def complete_availability_recovery(self, journal, *, restart=False):
        require(journal.get("operation") in {"backup", "restart"} and isinstance(journal.get("was_running"), bool), "Invalid availability recovery record.")
        state = journal["state"]
        require(state == self.state(), "Installed configuration differs from the interrupted operation.")
        self.managed_container(state)
        if journal["was_running"]:
            if restart:
                self.stop(state)
            self.start(state)
            self.readiness(state)
        else:
            self.stop(state)
        self.journal_file.unlink()
        sync_directory(self.data)

    def backup(self, state):
        self.no_pending()
        self.space_for_backup()
        container = self.managed_container(state)
        was_running = container is not None and container["State"]["Running"]
        # SIGKILL or a power loss skips finally; restart must still know the original state.
        self.journal("backup", state, phase="stopping", was_running=was_running)
        try:
            self.stop(state)
            return self.archive(state)
        finally:
            self.complete_availability_recovery(json_file(self.journal_file))

    def recover_prestart(self, journal, finish=True):
        previous = journal.get("previous", journal["state"])
        current = self.managed_container(previous, exact=False)
        if current is not None and current["Config"]["Image"] != previous["release"]["image"]:
            require(not current["State"]["Running"] and current["State"].get("StartedAt", "").startswith("0001-01-01"),
                    "Candidate has started; automatic recovery would risk losing writes. Explicit restore is required.")
            run(["docker", "rm", "ember"])
            current = None
        if current is None:
            self.managed_container(previous, "ember-previous", required=True, exact=True)
            run(["docker", "rename", "ember-previous", "ember"])
        atomic_json(self.state_file, previous)
        if finish:
            self.complete_prestart_recovery(journal)
        return previous

    def complete_prestart_recovery(self, journal):
        if "bundle_dir" in journal:
            directory = Path(journal["bundle_dir"])
            require(directory.parent == self.data and directory.name.startswith(".update-bundle-"), "Unsafe staged bundle path.")
            if directory.exists():
                private(directory, True)
                shutil.rmtree(directory)
        self.journal_file.unlink()
        sync_directory(self.data)

    def update(self, state, version):
        self.no_pending()
        full_release = download_manifest(version)
        release = {key: full_release[key] for key in ("schema", "version", "source_sha", "image")}
        version = release["version"]
        if version == state["release"]["version"]:
            require(release["image"] == state["release"]["image"], "Immutable version changed its image digest; release is invalid.")
            print(f"Ember {version} is already installed.")
            return
        require(tuple(map(int, version.split("."))) > tuple(map(int, state["release"]["version"].split("."))), "Downgrades require an explicit full-backup restore.")
        current = self.managed_container(state, required=True)
        previous = self.managed_container(state, "ember-previous", exact=False)
        require(previous is None or not previous["State"]["Running"], "The previous container is unexpectedly running; stop it explicitly.")
        self.pull(release["image"])
        bundle_dir, manager_checksums = self.stage_bundle(full_release)
        try:
            self.space_for_backup()
            was_running = current["State"]["Running"]
            preparing = {"previous": state, "bundle_dir": str(bundle_dir), "manager_checksums": manager_checksums}
            # Persist before the first downtime: unless-stopped will not reboot a manually stopped app.
            self.journal("update", state, phase="preparing-backup", **preparing)
            try:
                self.stop(state)
                identifier = self.archive(state)
            except Exception:
                # Abrupt termination leaves this journal for emberctl restart. Ordinary failures
                # resume the untouched version; a recovery error must not hide the original cause.
                try:
                    recovered = self.recover_prestart(json_file(self.journal_file), finish=False)
                    if was_running:
                        self.start(recovered)
                        self.readiness(recovered)
                    self.complete_prestart_recovery(json_file(self.journal_file))
                except Exception as recovery_error:
                    print(f"Previous-version recovery needs attention: {recovery_error}. Run emberctl status and emberctl restart.", file=sys.stderr)
                raise
            candidate = {**state, "release": release}
            details = {"backup": identifier, "previous": state, "bundle_dir": str(bundle_dir), "manager_checksums": manager_checksums}
            self.journal("update", state, phase="backed-up", **details)
            candidate_started = False
            try:
                if previous is not None:
                    run(["docker", "rm", "ember-previous"])
                run(["docker", "rename", "ember", "ember-previous"])
                atomic_json(self.state_file, candidate)
                self.journal("update", candidate, phase="candidate-created", **details)
                self.create(candidate)
                # Persist before docker start: a crash after this point cannot prove absence of writes.
                self.journal("update", candidate, phase="candidate-touched-data", **details)
                candidate_started = True
                self.start(candidate)
                self.readiness(candidate)
                self.finish_manager_update(json_file(self.journal_file))
            except BaseException:
                if not candidate_started:
                    try:
                        recovered = self.recover_prestart(json_file(self.journal_file), finish=False)
                        if was_running:
                            self.start(recovered)
                            self.readiness(recovered)
                        self.complete_prestart_recovery(json_file(self.journal_file))
                    except Exception as recovery_error:
                        print(f"Previous-version recovery needs attention: {recovery_error}. Run emberctl status and emberctl restart.", file=sys.stderr)
                    print("Update failed before the candidate started; the previous version was preserved.", file=sys.stderr)
                else:
                    with contextlib.suppress(Exception):
                        self.stop(candidate)
                    print(f"Update stopped. Recovery backup: {identifier}. The candidate may have changed data. Use emberctl logs/restart, or emberctl restore {identifier} --accept-data-loss to discard changes since that backup.", file=sys.stderr)
                raise
            self.journal_file.unlink()
            print(f"Ember {version} is ready. Full pre-update backup: {identifier}")
        finally:
            if bundle_dir.exists() and not self.journal_file.exists():
                shutil.rmtree(bundle_dir)

    def discard_restore_staging(self):
        if not self.journal_file.exists():
            return
        journal = json_file(self.journal_file)
        if journal.get("operation") != "restore":
            return
        for key, parent, prefix in (("staged_config", self.config.parent, ".ember-restore-"),
                                     ("staged_storage", self.data, ".storage-restore-")):
            if key not in journal:
                continue
            path = Path(journal[key])
            require(path.parent == parent and path.name.startswith(prefix), "Unsafe prior restore staging path.")
            if path.exists():
                information = path.lstat()
                require(stat.S_ISDIR(information.st_mode) and information.st_uid in {ROOT_UID, APP_UID}
                        and information.st_mode & 0o077 == 0, "Unsafe prior restore staging directory.")
                shutil.rmtree(path)

    def restore(self, state, identifier, accept):
        require(accept, "Restore discards ALL messages, uploads, and configuration changes since the backup. Repeat with --accept-data-loss after checking the backup ID and timestamp.")
        metadata = self.validate_backup(identifier, state)
        saved = metadata["state"]
        self.managed_container(state, exact=False)
        self.pull(saved["release"]["image"])
        self.discard_restore_staging()
        required = sum(record["size"] for record in metadata["entries"].values())
        require(shutil.disk_usage(self.data).free > required + 256 * 1024 * 1024, "Insufficient disk space to stage a complete restore.")
        token = uuid.uuid4().hex[:8]
        staged_config = self.config.parent / f".ember-restore-{token}"
        staged_storage = self.data / f".storage-restore-{token}"
        stage = {"config": staged_config, "storage": staged_storage}
        created = []
        try:
            for directory in (staged_config, staged_storage):
                directory.mkdir(mode=0o700)
                created.append(directory)
            with tarfile.open(self.backups / identifier / "snapshot.tar.gz", "r:gz") as source:
                members = source.getmembers()
                for member in members:
                    parts = PurePosixPath(member.name).parts
                    destination = stage[parts[0]].joinpath(*parts[1:])
                    if member.isdir():
                        destination.mkdir(mode=0o700, parents=True, exist_ok=True)
                    else:
                        destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                        with destination.open("xb") as output, source.extractfile(member) as data:
                            shutil.copyfileobj(data, output)
                            output.flush()
                            os.fsync(output.fileno())
                for member in sorted(members, key=lambda item: len(PurePosixPath(item.name).parts), reverse=True):
                    parts = PurePosixPath(member.name).parts
                    destination = stage[parts[0]].joinpath(*parts[1:])
                    os.chmod(destination, member.mode)
                    os.chown(destination, member.uid, member.gid)
                    os.utime(destination, (member.mtime, member.mtime))
                    if member.isdir():
                        sync_directory(destination)
            require(json_file(staged_config / "state.json") == saved, "Backup configuration state differs from backup metadata.")
            self.journal("restore", state, phase="staged", backup=identifier, staged_config=str(staged_config), staged_storage=str(staged_storage))
            self.stop(state)
            self.assert_storage_quiescent()
            if self.managed_container(state, exact=False) is not None:
                run(["docker", "rm", "ember"])
            old_config = self.config.parent / f"ember.pre-restore-{token}"
            old_storage = self.data / f"storage.pre-restore-{token}"
            self.journal("restore", saved, phase="replacing", backup=identifier, staged_config=str(staged_config), staged_storage=str(staged_storage), old_config=str(old_config), old_storage=str(old_storage))
            if self.config.exists():
                durable_rename(self.config, old_config)
            durable_rename(staged_config, self.config)
            if self.storage.exists():
                durable_rename(self.storage, old_storage)
            durable_rename(staged_storage, self.storage)
            self.create(saved)
            self.start(saved)
            self.readiness(saved)
            self.install_manager({name: (self.config / "manager" / name).read_bytes() for name in ("emberctl", "emberctl.py")})
            self.journal_file.unlink()
            print(f"Restored {identifier}: Ember {saved['release']['version']}. Superseded files remain private at {old_storage} and {old_config} for manual review.")
        finally:
            active = json_file(self.journal_file) if self.journal_file.exists() else {}
            retained = [active.get(key) for key in ("staged_config", "staged_storage")]
            for directory in created:
                if directory.exists() and str(directory) not in retained:
                    shutil.rmtree(directory)

    def restart(self, state):
        journal = json_file(self.journal_file) if self.journal_file.exists() else None
        prestart_recovery = None
        if journal and journal.get("operation") in {"backup", "restart"}:
            self.complete_availability_recovery(journal, restart=True)
            print(f"Recovered {journal['operation']}; Ember is {'ready' if journal['was_running'] else 'stopped as before the backup'}.")
            return
        if journal and journal.get("operation") == "install" and journal.get("phase") in {"preparing", "prepared"}:
            self.resume_install(journal)
            return
        if journal and journal.get("operation") == "update" and journal.get("phase") in {"preparing-backup", "backed-up", "candidate-created"}:
            state = self.recover_prestart(journal, finish=False)
            prestart_recovery = journal
            journal = None
        require(journal is None or journal.get("operation") in {"install", "update"}, "Restore recovery remains pending; rerun the explicit restore.")
        self.managed_container(state)
        if journal is None and prestart_recovery is None:
            self.journal("restart", state, phase="stopping", was_running=True)
            journal = json_file(self.journal_file)
        self.stop(state)
        self.start(state)
        self.readiness(state)
        if prestart_recovery is not None:
            self.complete_prestart_recovery(prestart_recovery)
        if journal is not None:
            if journal["operation"] == "update" and journal.get("phase") != "manager-installed":
                self.finish_manager_update(journal)
            self.journal_file.unlink()
            sync_directory(self.data)
            if journal["operation"] == "update":
                directory = Path(journal["bundle_dir"])
                require(directory.parent == self.data and directory.name.startswith(".update-bundle-"), "Unsafe staged bundle path.")
                if directory.exists():
                    private(directory, True)
                    shutil.rmtree(directory)
        print(f"Ember {state['release']['version']} is ready.")

    def reset_password(self, state, email):
        self.no_pending()
        require(email and not email.startswith("-") and "\n" not in email, "Pass an email address.")
        self.managed_container(state, required=True)
        require(sys.stdin.isatty(), "Run reset-password from a terminal; passwords are read privately and never accepted as arguments.")
        password = getpass.getpass("New password: ")
        require(len(password) >= 8 and "\n" not in password and len(password.encode()) <= 4096, "Password must contain at least eight characters and fit within 4096 bytes.")
        require(password == getpass.getpass("Repeat new password: "), "Passwords do not match.")
        run(["docker", "exec", "-i", "ember", "ember", "reset-password", email], input=password, capture=False)
        self.restart(state)
        print("Password updated; existing sessions and live connections for this user were revoked.")


def parser():
    result = argparse.ArgumentParser(description="Manage a fresh Ember installation (run as root).")
    commands = result.add_subparsers(dest="command", required=True)
    for internal in ("install", "preflight"):
        install = commands.add_parser(internal, help=argparse.SUPPRESS)
        install.add_argument("--bundle-dir", type=Path, required=True)
        install.add_argument("--domain")
        install.add_argument("--runtime-env", type=Path, default=os.environ.get("EMBER_INSTALL_RUNTIME_ENV") or None)
        install.add_argument("--test-ca", type=Path, default=os.environ.get("EMBER_INSTALL_TEST_CA") or None)
    for name in ("status", "logs", "restart", "setup-link", "backup"):
        commands.add_parser(name)
    commands.add_parser("update").add_argument("version", nargs="?")
    restore = commands.add_parser("restore")
    restore.add_argument("backup")
    restore.add_argument("--accept-data-loss", action="store_true")
    commands.add_parser("reset-password").add_argument("email")
    return result


def main(argv=None):
    args = parser().parse_args(argv)
    require(os.geteuid() == 0, "Run emberctl as root (for example sudo emberctl status).")
    os.umask(0o077)
    manager = Manager()
    if args.command == "preflight":
        validate_manifest(json_file(args.bundle_dir / "install.json"), bundle=False)
        print(manager.preflight(args.domain))
        return
    with manager.lock():
        if args.command == "install":
            manager.install(args)
            return
        state = manager.state(recovery=args.command in {"status", "restore", "restart"})
        if args.command == "status":
            container = manager.managed_container(state, exact=False)
            print(f"Ember {state['release']['version']} at https://{state['domain']}; container: {'running' if container and container['State']['Running'] else 'stopped/absent'}")
            if manager.journal_file.exists():
                journal = json_file(manager.journal_file)
                print(f"ATTENTION: {journal['operation']} recovery pending ({journal.get('phase')}); backup: {journal.get('backup', 'none')}")
        elif args.command == "logs":
            manager.managed_container(state, required=True)
            run(["docker", "logs", "--tail", "200", "ember"], capture=False)
        elif args.command == "setup-link":
            manager.setup_link(state)
        elif args.command == "restart":
            manager.restart(state)
        elif args.command == "backup":
            print(f"Full backup: {manager.backup(state)}")
        elif args.command == "update":
            manager.update(state, args.version)
        elif args.command == "restore":
            manager.restore(state, args.backup, args.accept_data_loss)
        elif args.command == "reset-password":
            manager.reset_password(state, args.email)


if __name__ == "__main__":
    try:
        main()
    except (Failure, OSError, ValueError, KeyError, tarfile.TarError) as error:
        print(f"emberctl: {error}", file=sys.stderr)
        sys.exit(1)
