"""Fast transport contracts; full certificate and Docker acceptance runs in QEMU."""
import contextlib
import importlib.util
import io
from pathlib import Path
import ssl
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("guest_acceptance", Path(__file__).parents[1] / "guests.py")
guests = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guests)


class InteractiveCommand(unittest.TestCase):
    def test_prompt_split_across_output_chunks_receives_exactly_one_reply(self):
        source = """
import select, sys, time
prompt = "Hostname pointing to this server (for example chat.example.com): "
sys.stdout.write(prompt[:20]); sys.stdout.flush()
time.sleep(0.03)
sys.stdout.write(prompt[20:]); sys.stdout.flush()
print("hostname=" + sys.stdin.readline().strip(), flush=True)
sys.stdout.write(prompt); sys.stdout.flush()
assert not select.select([sys.stdin], [], [], 0.05)[0], "Hostname was sent twice"
print("one-response", flush=True)
"""
        result = guests.run_interactive([sys.executable, "-u", "-c", source], [(guests.HOSTNAME_PROMPT, guests.DOMAIN)], timeout=5)
        self.assertEqual(result.returncode, 0)
        self.assertIn("hostname=" + guests.DOMAIN, result.stdout)
        self.assertIn("one-response", result.stdout)

    def test_no_prompt_does_not_receive_unsolicited_input(self):
        source = "import select,sys; assert not select.select([sys.stdin],[],[],0.05)[0]; print('already managed')"
        result = guests.run_interactive([sys.executable, "-u", "-c", source], [(guests.HOSTNAME_PROMPT, guests.DOMAIN)], timeout=5)
        self.assertEqual(result.stdout.strip(), "already managed")

    def test_failure_never_attaches_private_transcript_to_exception(self):
        source = "print('https://chat.example.test/first_run/access#' + 'tok' + 'en=private'); raise SystemExit(7)"
        with self.assertRaises(subprocess.CalledProcessError) as failure:
            guests.run_interactive([sys.executable, "-u", "-c", source], [], timeout=5)
        self.assertEqual(failure.exception.returncode, 7)
        self.assertIsNone(failure.exception.output)
        self.assertNotIn("token=", str(failure.exception))

    def test_timeout_is_bounded_and_does_not_attach_output(self):
        source = "import time; print('private setup transcript',flush=True); time.sleep(5)"
        with self.assertRaises(subprocess.TimeoutExpired) as failure:
            guests.run_interactive([sys.executable, "-u", "-c", source], [], timeout=0.05)
        self.assertIsNone(failure.exception.output)

    def test_guest_requests_real_terminal_and_runs_literal_advertised_command(self):
        guest = guests.Guest.__new__(guests.Guest)
        guest.key = Path("/temporary/ssh-key")
        with mock.patch.object(guests, "run_interactive", return_value=subprocess.CompletedProcess([], 0, "", "")) as interactive:
            guest.install()
        command = interactive.call_args.args[0]
        self.assertIn("-tt", command)
        prefix, actual = command[-1].split("; ", 1)
        self.assertEqual(actual, "curl -fsSL https://get.nyllon.com/ember | sh --")
        self.assertIn("EMBER_INSTALL_RUNTIME_ENV=/root/ember-runtime.env", prefix)
        self.assertIn("EMBER_INSTALL_TEST_CA=/root/ember-test-ca.pem", prefix)
        self.assertNotIn("--domain", command[-1])
        self.assertEqual(interactive.call_args.args[1], [(guests.HOSTNAME_PROMPT, guests.DOMAIN)])


class PreflightDiagnostics(unittest.TestCase):
    def test_expected_refusal_passes_without_logging_any_output(self):
        result = subprocess.CompletedProcess([], 1, "private stdout", "Conflicting package runc is installed")
        self.assertIsNone(guests.require_preflight_refusal(result, "Conflicting package runc", "package conflict"))

    def test_unexpected_failure_reports_exit_and_bounded_redacted_output(self):
        result = subprocess.CompletedProcess([], 7,
            "\x1b[31mSECRET_KEY_BASE=private-secret\n"
            "VAPID_PRIVATE_KEY=private-vapid\n"
            "EMBER_SETUP_TOKEN=private-env-token\n"
            "https://chat.ember.test/first_run/access#token=private-link-token\n"
            "Cookie: session=private-cookie\nAuthorization: Bearer private-authorization\n",
            "TCP port 80 is unavailable\n")
        with self.assertRaises(guests.PreflightFailure) as failure:
            guests.require_preflight_refusal(result, "Conflicting package runc", "package conflict")
        diagnostic = str(failure.exception)
        self.assertIn("exit=7", diagnostic)
        self.assertIn("TCP port 80 is unavailable", diagnostic)
        self.assertIn("[redacted]", diagnostic)
        for private in ("private-secret", "private-vapid", "private-env-token", "private-link-token", "private-cookie", "private-authorization", "\x1b"):
            self.assertNotIn(private, diagnostic)
        self.assertLess(len(guests.sanitized_output("x" * 20000)), 4200)

    def test_successful_install_does_not_satisfy_a_refusal_gate(self):
        result = subprocess.CompletedProcess([], 0, "installed", "Conflicting package runc")
        with self.assertRaises(guests.PreflightFailure):
            guests.require_preflight_refusal(result, "Conflicting package runc", "package conflict")


class MirrorCertificates(unittest.TestCase):
    def test_mirror_chain_passes_strict_verification_for_all_https_names(self):
        with tempfile.TemporaryDirectory() as temporary:
            certs = Path(temporary)
            guests.mirror_certificates(certs)
            server = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            server.load_cert_chain(certs / "server.pem", certs / "server.key")
            client = ssl.create_default_context(cafile=str(certs / "ca.pem"))
            # This is a default in Python 3.13; require it on older developer runtimes too.
            client.verify_flags |= ssl.VERIFY_X509_STRICT
            for hostname in ("github.com", "get.nyllon.com", "pebble.ember.test"):
                with self.subTest(hostname=hostname):
                    client_in, client_out, server_in, server_out = (ssl.MemoryBIO() for _ in range(4))
                    connections = (client.wrap_bio(client_in, client_out, server_hostname=hostname),
                                   server.wrap_bio(server_in, server_out, server_side=True))
                    completed = set()
                    for _ in range(20):
                        for connection in connections:
                            try:
                                connection.do_handshake()
                                completed.add(connection)
                            except ssl.SSLWantReadError:
                                pass
                        server_in.write(client_out.read())
                        client_in.write(server_out.read())
                        if len(completed) == 2:
                            break
                    self.assertEqual(len(completed), 2, "Strict TLS handshake must complete")


class BrowserDiagnostics(unittest.TestCase):
    def test_playwright_failure_redacts_private_url_and_preserves_exit_code(self):
        token = 'c' * 64
        result = subprocess.CompletedProcess(['node'], 7, 'Browser setup started\n',
            'page.goto: net::ERR_CONNECTION_REFUSED\nCall log:\n'
            '  - navigating to "https://chat.ember.test/first_run/access#token=' + token + '"\n'
            "Actual received value: '" + token + "'\n")
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.object(guests, 'run', return_value=result) as run, contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            with self.assertRaises(subprocess.CalledProcessError) as failure:
                guests.browser('setup', token)
        self.assertEqual(failure.exception.returncode, 7)
        self.assertIsNone(failure.exception.output)
        self.assertIsNone(failure.exception.stderr)
        self.assertNotIn(token, str(failure.exception))
        self.assertEqual(stdout.getvalue(), 'Browser setup started\n')
        self.assertIn('ERR_CONNECTION_REFUSED', stderr.getvalue())
        self.assertIn('#token=[redacted]', stderr.getvalue())
        self.assertNotIn(token, stderr.getvalue())
        self.assertTrue(run.call_args.kwargs['capture'])
        self.assertFalse(run.call_args.kwargs['check'])
        self.assertEqual(run.call_args.kwargs['timeout'], 300)

    def test_success_reports_sanitized_useful_output(self):
        result = subprocess.CompletedProcess(['node'], 0, 'Browser persist: checks passed\n', 'Cookie: private-session\n')
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.object(guests, 'run', return_value=result), contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            guests.browser('persist')
        self.assertEqual(stdout.getvalue(), 'Browser persist: checks passed\n')
        self.assertEqual(stderr.getvalue(), 'Cookie: [redacted]\n')

    def test_timeout_redacts_captured_output_and_drops_original_transcript(self):
        token = 'd' * 64
        error = subprocess.TimeoutExpired(['node'], 300,
            output=('private setup fragment ' + token).encode(),
            stderr=('page.goto: https://chat.ember.test/first_run/access#token=' + token).encode())
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.object(guests, 'run', side_effect=error), contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            with self.assertRaises(subprocess.TimeoutExpired) as failure:
                guests.browser('setup', token)
        self.assertEqual(failure.exception.timeout, 300)
        self.assertIsNone(failure.exception.output)
        self.assertIsNone(failure.exception.stderr)
        self.assertTrue(failure.exception.__suppress_context__)
        self.assertNotIn(token, stdout.getvalue() + stderr.getvalue() + str(failure.exception))
        self.assertIn('[redacted]', stdout.getvalue())
        self.assertIn('#token=[redacted]', stderr.getvalue())


class BootDiagnostics(unittest.TestCase):
    def test_initial_boot_has_a_separate_deadline_and_short_ssh_probes(self):
        guest = guests.Guest.__new__(guests.Guest)
        guest.process = mock.Mock()
        guest.process.poll.return_value = None

        def probe(action, timeout):
            self.assertEqual(timeout, 1200)
            return action()

        with mock.patch.object(guests, "wait_for", side_effect=probe) as wait, mock.patch.object(guest, "ssh") as ssh:
            guest.wait_for_ssh()
        wait.assert_called_once_with(mock.ANY, timeout=guests.GUEST_BOOT_TIMEOUT)
        ssh.assert_called_once_with("true", timeout=15)

    def test_ssh_failure_emits_bounded_console_tail_and_final_error(self):
        guest = guests.Guest.__new__(guests.Guest)
        guest.process = mock.Mock()
        guest.process.poll.return_value = None
        failure = subprocess.CalledProcessError(255, ["ssh"], stderr=b"ssh: connect: Connection refused\n")
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            (work / "diagnostics").mkdir()
            (work / "diagnostics/console.log").write_text("x" * 30000 + "\nfinal kernel message\x1b[0m\n")
            output = io.StringIO()
            with mock.patch.object(guests, "WORK", work), mock.patch.object(guests, "wait_for", side_effect=failure), contextlib.redirect_stderr(output):
                with self.assertRaises(subprocess.CalledProcessError) as raised:
                    guest.wait_for_ssh()
            diagnostic = (work / "diagnostics/boot.log").read_text()
            self.assertIs(raised.exception, failure)
            self.assertEqual(output.getvalue(), diagnostic + "\n")
            self.assertIn("QEMU exit status: None", diagnostic)
            self.assertIn("Connection refused", diagnostic)
            self.assertIn("final kernel message", diagnostic)
            self.assertNotIn("\x1b", diagnostic)
            self.assertLess(len(diagnostic), 17000)

    def test_early_qemu_exit_emits_its_status_without_attempting_ssh(self):
        guest = guests.Guest.__new__(guests.Guest)
        guest.process = mock.Mock()
        guest.process.poll.return_value = 1
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            (work / "diagnostics").mkdir()
            with mock.patch.object(guests, "WORK", work), mock.patch.object(guest, "ssh") as ssh, contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaisesRegex(RuntimeError, "QEMU exited"):
                    guest.wait_for_ssh()
            ssh.assert_not_called()
            self.assertIn("QEMU exit status: 1", (work / "diagnostics/boot.log").read_text())


if __name__ == "__main__":
    unittest.main()
