"""Fast transport contracts; full certificate and Docker acceptance runs in QEMU."""
import contextlib
import importlib.util
import io
from pathlib import Path
import ssl
import subprocess
import sys
import tempfile
import threading
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


class InstallerProgress(unittest.TestCase):
    def test_real_timeout_retains_only_allowlisted_progress_after_split_prompt_and_marker(self):
        source = """
import sys, time
time.sleep(0.6)
sys.stdout.write("Reading pack"); sys.stdout.flush()
time.sleep(0.03)
sys.stdout.write("age lists\\nhttps://chat.ember.test/first_run/access#token=private-fragment\\n"); sys.stdout.flush()
prompt = "Hostname pointing to this server (for example chat.example.com): "
sys.stdout.write(prompt[:20]); sys.stdout.flush()
time.sleep(0.03)
sys.stdout.write(prompt[20:]); sys.stdout.flush()
print("received=" + sys.stdin.readline().strip(), flush=True)
time.sleep(30)
"""
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            (work / 'diagnostics').mkdir()
            output = io.StringIO()
            with mock.patch.object(guests, 'WORK', work), contextlib.redirect_stderr(output):
                with self.assertRaises(guests.InteractiveCommandTimeout) as failure:
                    guests.run_interactive([sys.executable, '-u', '-c', source],
                        [(guests.HOSTNAME_PROMPT, 'private-reply')], timeout=5, progress=guests.report_install_progress)
            diagnostic = (work / 'diagnostics/install.log').read_text()
        error = failure.exception
        self.assertEqual(error.timeout, 5)
        self.assertIsNone(error.output)
        self.assertEqual(error.cmd, ['interactive command omitted'])
        self.assertTrue(error.__suppress_context__)
        snapshots = [guests.json.loads(line.split(': ', 1)[1]) for line in output.getvalue().splitlines()]
        for snapshot in snapshots:
            self.assertEqual(set(snapshot), {'observed_output', 'prompt_observed', 'reply_sent', 'bytes_received',
                                            'elapsed_seconds', 'last_activity_seconds', 'quiet_seconds'})
            self.assertIsInstance(snapshot['bytes_received'], int)
            self.assertIsInstance(snapshot['prompt_observed'], bool)
            self.assertIsInstance(snapshot['reply_sent'], bool)
            for key in ('elapsed_seconds', 'last_activity_seconds', 'quiet_seconds'):
                self.assertIsInstance(snapshot[key], (int, float))
                self.assertGreaterEqual(snapshot[key], 0)
        self.assertEqual(snapshots[0]['observed_output'], 'unknown')
        self.assertTrue(any(item['prompt_observed'] and not item['reply_sent'] for item in snapshots))
        self.assertEqual(snapshots[-1]['observed_output'], 'package-manager-output')
        self.assertTrue(snapshots[-1]['prompt_observed'] and snapshots[-1]['reply_sent'])
        self.assertGreater(snapshots[-1]['bytes_received'], len(guests.HOSTNAME_PROMPT))
        self.assertGreaterEqual(snapshots[-1]['elapsed_seconds'], 5)
        self.assertIn('package-manager-output', error.stderr)
        self.assertIn('package-manager-output', diagnostic)
        evidence = output.getvalue() + diagnostic + str(error) + error.stderr
        for private in ('private-fragment', 'private-reply', '#token=', 'sys.stdout', 'received='):
            self.assertNotIn(private, evidence)

    def test_terminal_eof_wait_timeout_retains_configured_deadline_and_safe_progress(self):
        source = "import os,time; time.sleep(0.6); print('private-closed-terminal',flush=True); os.close(1); os.close(2); time.sleep(30)"
        reports = []
        with self.assertRaises(guests.InteractiveCommandTimeout) as failure:
            guests.run_interactive([sys.executable, '-u', '-c', source], [], timeout=5, progress=reports.append)
        error = failure.exception
        self.assertEqual(error.timeout, 5)
        self.assertIsNone(error.output)
        self.assertTrue(error.__suppress_context__)
        self.assertIn('Installer interactive progress:', error.stderr)
        self.assertGreater(reports[-1]['bytes_received'], 0)
        self.assertNotIn('private-closed-terminal', str(error) + error.stderr)

    def test_quiet_unknown_output_emits_liveness_at_sixty_second_intervals(self):
        reports, now = [], [0.0]
        with mock.patch.object(guests.time, 'monotonic', side_effect=lambda: now[0]):
            progress = guests.InteractiveProgress(reports.append)
            progress.emit()
            now[0] = 59.0
            progress.emit()
            self.assertEqual(len(reports), 1)
            now[0] = 60.0
            progress.emit()
            now[0] = 61.0
            progress.observe(b'unrecognized private output')
            progress.emit()
            self.assertEqual(len(reports), 2)
            now[0] = 120.0
            progress.emit()
        self.assertEqual([report['elapsed_seconds'] for report in reports], [0.0, 60.0, 120.0])
        self.assertEqual(reports[-1]['observed_output'], 'unknown')
        self.assertEqual(reports[-1]['last_activity_seconds'], 61.0)
        self.assertEqual(reports[-1]['quiet_seconds'], 59.0)
        self.assertEqual(reports[-1]['bytes_received'], len(b'unrecognized private output'))
        self.assertNotIn('private', guests.interactive_progress_detail(reports[-1]))

    def test_guest_nonzero_exit_keeps_safe_progress_and_original_exit_when_reporting_fails(self):
        source = "print('Setting up docker-ce private-adjacent'); print('private-transcript'); raise SystemExit(7)"
        guest = guests.Guest.__new__(guests.Guest)
        for broken_report in (False, True):
            with self.subTest(broken_report=broken_report), tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
                work = Path(temporary)
                (work / 'diagnostics').mkdir()
                stack.enter_context(mock.patch.object(guests, 'WORK', work))
                stack.enter_context(mock.patch.object(guest, 'ssh_command', return_value=[sys.executable, '-u', '-c', source]))
                output = stack.enter_context(contextlib.redirect_stderr(io.StringIO()))
                if broken_report:
                    stack.enter_context(mock.patch.object(Path, 'write_text', side_effect=OSError('No space left on device')))
                    stack.enter_context(mock.patch('builtins.print', side_effect=OSError('Broken pipe')))
                with self.assertRaises(guests.InteractiveCommandFailure) as failure:
                    guest.install()
                error = failure.exception
                self.assertEqual(error.returncode, 7)
                self.assertIsNone(error.output)
                self.assertEqual(error.cmd, ['interactive command omitted'])
                self.assertIn('docker-package-output', error.stderr)
                evidence = str(error) + error.stderr + output.getvalue()
                if not broken_report:
                    evidence += (work / 'diagnostics/install.log').read_text()
                for private in ('private-adjacent', 'private-transcript', 'raise SystemExit'):
                    self.assertNotIn(private, evidence)

    def test_guest_transport_failure_and_timeout_preserve_safe_interactive_metadata(self):
        progress = guests.InteractiveProgress(None).snapshot()
        errors = (guests.InteractiveCommandFailure(255, progress), guests.InteractiveCommandTimeout(19, progress))
        guest = guests.Guest.__new__(guests.Guest)
        guest.key = Path('/temporary/ssh-key')
        for error in errors:
            with self.subTest(error=type(error).__name__), mock.patch.object(guests, 'run_interactive', side_effect=error) as run:
                with self.assertRaises((guests.SSHTransportFailure, guests.SSHTransportTimeout)) as failure:
                    guest.install('private-command')
            safe = failure.exception
            self.assertEqual(run.call_count, 1)
            self.assertIsNone(safe.output)
            self.assertIn('Installer interactive progress:', safe.stderr)
            self.assertIn('"observed_output": "unknown"', safe.stderr)
            self.assertNotIn('private-command', str(safe) + safe.stderr)


class InstallDeadlines(unittest.TestCase):
    def test_only_explicit_fresh_arm64_install_has_longer_deadline(self):
        guest = guests.Guest.__new__(guests.Guest)
        guest.key = Path('/temporary/ssh-key')
        result = subprocess.CompletedProcess([], 0, '', '')
        for arch, expected in (('arm64', 3600), ('amd64', 900)):
            with self.subTest(arch=arch), mock.patch.dict(guests.os.environ, {'GUEST_ARCH': arch}), mock.patch.object(guests, 'run_interactive', return_value=result) as interactive:
                guest.install(fresh=True)
                guest.install()
                guest.install(check=False)
            self.assertEqual([call.kwargs['timeout'] for call in interactive.call_args_list], [expected, 900, 900])
            self.assertEqual([call.kwargs['check'] for call in interactive.call_args_list], [True, True, False])
            for call in interactive.call_args_list:
                self.assertIs(call.kwargs['progress'], guests.report_install_progress)

    def test_acceptance_marks_first_install_fresh_and_managed_rerun_keeps_default(self):
        guest = mock.Mock()
        image = 'ghcr.io/nyllon-software/ember@sha256:' + 'a' * 64
        manifest = {'version': '1.0.0', 'image': image}
        runtime = {'ACME_DIRECTORY': 'https://pebble.ember.test:14000/dir', 'SSL_CERT_FILE': '/run/ember-test-ca.pem'}
        container = {'Config': {'Image': image, 'User': '1000:1000', 'Env': [key + '=' + value for key, value in runtime.items()]},
                     'Mounts': [{'Source': '/etc/ember/test-ca.pem', 'Destination': '/run/ember-test-ca.pem', 'RW': False}]}
        stop = RuntimeError('stop at managed rerun')
        guest.install.side_effect = [subprocess.CompletedProcess([], 0, guests.HOSTNAME_PROMPT + '/first_run/access#token=' + 'a' * 64, ''), stop]

        def ssh(command, **kwargs):
            if command == 'command -v docker':
                return subprocess.CompletedProcess([], 1, '', '')
            outputs = {'cat /etc/ember/state.json': guests.json.dumps({'domain': guests.DOMAIN, 'release': manifest}),
                       'cat /etc/ember/runtime.json': guests.json.dumps(runtime),
                       'docker container inspect ember': guests.json.dumps([container])}
            return subprocess.CompletedProcess([], 0, outputs.get(command, '123'), '')

        guest.ssh.side_effect = ssh
        with tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
            work = Path(temporary)
            (work / 'release.json').write_text(guests.json.dumps(manifest))
            stack.enter_context(mock.patch.object(guests, 'WORK', work))
            for name in ('require_privilege_refusal', 'require_preflight_refusal', 'browser'):
                stack.enter_context(mock.patch.object(guests, name))
            output = stack.enter_context(contextlib.redirect_stderr(io.StringIO()))
            with self.assertRaises(RuntimeError) as failure:
                guests.acceptance(guest, work, work, {}, public=True)
        snapshots = [guests.json.loads(line.split(': ', 1)[1]) for line in output.getvalue().splitlines()]
        self.assertEqual([item['stage'] for item in snapshots], ['preflight', 'first-install', 'browser-setup', 'managed-rerun'])
        self.assertNotIn('a' * 64, output.getvalue())
        self.assertNotIn(guests.DOMAIN, output.getvalue())
        self.assertIs(failure.exception, stop)
        self.assertEqual(guest.install.call_args_list, [mock.call(fresh=True), mock.call()])


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


class SSHTransportDiagnostics(unittest.TestCase):
    def setUp(self):
        self.guest = guests.Guest.__new__(guests.Guest)
        self.guest.key = Path('/temporary/ssh-key')

    def test_guest_transport_budget_keeps_outer_command_deadline(self):
        result = subprocess.CompletedProcess(['ssh'], 0, 'done', '')
        with mock.patch.object(guests, 'run', return_value=result) as run:
            self.assertIs(self.guest.ssh('true'), result)
        command = run.call_args.args
        for option in ('ConnectTimeout=30', 'ServerAliveInterval=15', 'ServerAliveCountMax=4'):
            self.assertIn(option, command)
        self.assertEqual(run.call_args.kwargs, {'capture': True, 'check': False, 'timeout': 900})

    def test_transport_exit_cannot_be_accepted_by_unchecked_command(self):
        command = 'printf private-command'
        stderr = ('ssh: Connection timed out\n' + command + '\n'
                  'SECRET_KEY_BASE=private-secret\nEMBER_SETUP_TOKEN=private-token\n'
                  'https://chat.ember.test/first_run/access#token=private-fragment\n'
                  'Cookie: private-cookie\nAuthorization: private-authorization\n')
        result = subprocess.CompletedProcess(['ssh', command], 255, 'private stdout', stderr)
        for check in (True, False):
            with self.subTest(check=check), mock.patch.object(guests, 'run', return_value=result) as run:
                with self.assertRaises(guests.SSHTransportFailure) as failure:
                    self.guest.ssh(command, check=check)
            self.assertEqual(run.call_count, 1, 'Mutating commands must never be retried')
            error = failure.exception
            self.assertEqual(error.returncode, 255)
            self.assertEqual(error.cmd, ['ssh', '[remote command omitted]'])
            self.assertIsNone(error.output)
            self.assertIn('Connection timed out', error.stderr)
            for private in ('private-command', 'private stdout', 'private-secret', 'private-token',
                            'private-fragment', 'private-cookie', 'private-authorization'):
                self.assertNotIn(private, str(error) + error.stderr)

    def test_timeout_keeps_only_bounded_redacted_stderr(self):
        command = 'printf private-command'
        timeout = subprocess.TimeoutExpired(['ssh', command], 17, output=b'private stdout',
            stderr=(('x' * 20000) + '\nVAPID_PRIVATE_KEY=private-vapid\n' + command + '\nssh: handshake stalled').encode())
        with mock.patch.object(guests, 'run', side_effect=timeout) as run:
            with self.assertRaises(guests.SSHTransportTimeout) as failure:
                self.guest.ssh(command, timeout=17)
        error = failure.exception
        self.assertEqual(run.call_count, 1)
        self.assertEqual(error.timeout, 17)
        self.assertIsNone(error.output)
        self.assertTrue(error.__suppress_context__)
        self.assertIn('handshake stalled', error.stderr)
        self.assertLess(len(error.stderr), 4200)
        for private in ('private stdout', 'private-vapid', 'private-command'):
            self.assertNotIn(private, str(error) + error.stderr)

    def test_interactive_transport_errors_withhold_terminal_transcript(self):
        command = 'printf private-command'
        errors = (subprocess.CalledProcessError(255, ['ssh', command], output='private stdout', stderr='private stderr'),
                  subprocess.TimeoutExpired(['ssh', command], 19, output=b'private stdout', stderr=b'private stderr'))
        for error in errors:
            with self.subTest(error=type(error).__name__), mock.patch.object(guests, 'run_interactive', side_effect=error) as run:
                with self.assertRaises((guests.SSHTransportFailure, guests.SSHTransportTimeout)) as failure:
                    self.guest.install(command)
            safe = failure.exception
            self.assertEqual(run.call_count, 1)
            self.assertIsNone(safe.output)
            self.assertTrue(safe.__suppress_context__)
            self.assertIn('terminal output intentionally omitted', safe.stderr)
            for private in ('private-command', 'private stdout', 'private stderr'):
                self.assertNotIn(private, str(safe) + safe.stderr)

    def test_unchecked_interactive_transport_exit_is_still_a_failure(self):
        result = subprocess.CompletedProcess(['ssh'], 255, 'private stdout', 'private stderr')
        with mock.patch.object(guests, 'run_interactive', return_value=result) as run:
            with self.assertRaises(guests.SSHTransportFailure) as failure:
                self.guest.install(check=False)
        self.assertEqual(run.call_count, 1)
        self.assertIsNone(failure.exception.output)
        self.assertNotIn('private', failure.exception.stderr)

    def test_remote_nonzero_exit_retains_expected_command_semantics(self):
        result = subprocess.CompletedProcess(['ssh', 'test -e /missing'], 1, '', 'missing')
        with mock.patch.object(guests, 'run', return_value=result):
            self.assertIs(self.guest.ssh('test -e /missing', check=False), result)
            with self.assertRaises(subprocess.CalledProcessError) as failure:
                self.guest.ssh('test -e /missing')
        self.assertNotIsInstance(failure.exception, guests.SSHTransportFailure)
        self.assertEqual(failure.exception.returncode, 1)

    def test_diagnostic_collection_preserves_original_transport_failure(self):
        original = guests.SSHTransportFailure('ssh: original connection reset', 'private command')
        cleanup_errors = (guests.SSHTransportFailure('later SSH failure', 'docker logs'),
                          guests.SSHTransportTimeout(15, 'later timeout', 'docker logs'),
                          OSError('later process spawn failure'))
        for cleanup_error in cleanup_errors:
            with self.subTest(cleanup=type(cleanup_error).__name__), tempfile.TemporaryDirectory() as temporary:
                work = Path(temporary)
                (work / 'diagnostics').mkdir()
                output = io.StringIO()
                with mock.patch.object(guests, 'WORK', work), mock.patch.object(self.guest, 'ssh', side_effect=cleanup_error) as ssh, contextlib.redirect_stderr(output):
                    guests.failure_diagnostics(self.guest, original)
                diagnostic = (work / 'diagnostics/transport.log').read_text()
                self.assertIn('exit=255', diagnostic)
                self.assertIn('original connection reset', diagnostic)
                self.assertNotIn('later', diagnostic)
                self.assertNotIn('private command', diagnostic)
                self.assertEqual(output.getvalue(), diagnostic + '\n')
                self.assertFalse((work / 'diagnostics/application.log').exists())
                ssh.assert_called_once_with('docker logs --tail 100 ember', check=False, timeout=15)

    def test_timeout_diagnostic_reports_deadline_without_output_or_command(self):
        error = guests.SSHTransportTimeout(17, 'ssh: timeout\nCookie: private-cookie', 'private command')
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            (work / 'diagnostics').mkdir()
            with mock.patch.object(guests, 'WORK', work), mock.patch.object(self.guest, 'ssh', return_value=subprocess.CompletedProcess([], 0, 'app log', '')), contextlib.redirect_stderr(io.StringIO()):
                guests.failure_diagnostics(self.guest, error)
            diagnostic = (work / 'diagnostics/transport.log').read_text()
            self.assertIn('timeout=17s', diagnostic)
            self.assertIn('ssh: timeout', diagnostic)
            self.assertNotIn('private-cookie', diagnostic)
            self.assertNotIn('private command', diagnostic)
            self.assertEqual((work / 'diagnostics/application.log').read_text(), 'app log')

    def test_boot_transport_failure_reaches_top_level_diagnostics_even_when_disk_is_full(self):
        original = guests.SSHTransportFailure('ssh: cloud-init connection reset\nEMBER_SETUP_TOKEN=private-token', 'private command')
        for write_failure in (False, True):
            with self.subTest(write_failure=write_failure), tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
                work = Path(temporary) / 'fresh-work'
                guest = mock.Mock()
                guest.boot.side_effect = original
                guest.ssh.side_effect = OSError('diagnostic-only failure')
                stack.enter_context(mock.patch.object(guests, 'WORK', work))
                stack.enter_context(mock.patch.object(guests.argparse.ArgumentParser, 'parse_args', return_value=guests.argparse.Namespace(assets=Path(temporary), benchmark_disposable_runner=False, accel='tcg')))
                stack.enter_context(mock.patch.object(guests.os, 'geteuid', return_value=0))
                stack.enter_context(mock.patch.object(guests.platform, 'system', return_value='Linux'))
                stack.enter_context(mock.patch.object(guests.platform, 'machine', return_value='aarch64'))
                stack.enter_context(mock.patch.dict(guests.os.environ, {'GITHUB_ACTIONS': 'true', 'GUEST_ARCH': 'arm64', 'GUEST_OS': 'ubuntu-24.04'}))
                for name in ('network', 'certificates', 'mirror'):
                    stack.enter_context(mock.patch.object(guests, name))
                stack.enter_context(mock.patch.object(guests, 'Guest', return_value=guest))
                acceptance = stack.enter_context(mock.patch.object(guests, 'acceptance'))
                if write_failure:
                    stack.enter_context(mock.patch.object(Path, 'write_text', side_effect=OSError('No space left on device')))
                output = stack.enter_context(contextlib.redirect_stderr(io.StringIO()))
                with self.assertRaises(guests.SSHTransportFailure) as failure:
                    guests.main()
                self.assertIs(failure.exception, original)
                self.assertIn('cloud-init connection reset', output.getvalue())
                self.assertNotIn('private-token', output.getvalue())
                self.assertNotIn('private command', output.getvalue())
                if not write_failure:
                    self.assertIn('cloud-init connection reset', (work / 'diagnostics/transport.log').read_text())
                acceptance.assert_not_called()
                guest.boot.assert_called_once_with('ubuntu-24.04', 'arm64')

    def test_diagnostic_file_and_stderr_failures_are_both_best_effort(self):
        with mock.patch.object(Path, 'write_text', side_effect=OSError('No space left on device')) as write, mock.patch('builtins.print', side_effect=OSError('Broken pipe')) as emit:
            guests.write_diagnostic('transport.log', 'safe original cause', emit=True)
        write.assert_called_once_with('safe original cause')
        emit.assert_called_once_with('safe original cause', file=guests.sys.stderr, flush=True)


class PrivilegeRefusal(unittest.TestCase):
    def test_actual_installer_and_sudo_privilege_refusals_pass(self):
        messages = ('Run this installer as root (or install sudo).',
                    'sudo: a terminal is required to read the password; use -S or an askpass helper',
                    'sudo: a password is required',
                    'nobody is not in the sudoers file.',
                    'Sorry, user nobody is not allowed to execute this command as root.')
        for stderr in messages:
            with self.subTest(stderr=stderr):
                result = subprocess.CompletedProcess([], 1, '', stderr)
                self.assertIsNone(guests.require_privilege_refusal(result))

    def test_unrelated_failure_success_or_transport_cannot_prove_privilege_refusal(self):
        cases = ((1, 'curl: Could not resolve host: get.nyllon.com'),
                 (1, 'sudo: unable to execute installer: No such file or directory'),
                 (0, 'Run this installer as root (or install sudo).'),
                 (255, 'Run this installer as root (or install sudo).'),
                 (255, 'sudo: a password is required'))
        for code, stderr in cases:
            with self.subTest(code=code, stderr=stderr):
                with self.assertRaises(guests.PreflightFailure):
                    guests.require_privilege_refusal(subprocess.CompletedProcess([], code, '', stderr))


class GuestReboot(unittest.TestCase):
    def test_reboot_requests_once_and_requires_changed_boot_id_after_disconnect(self):
        errors = (None, guests.SSHTransportFailure('Connection closed', 'systemctl reboot'),
                  guests.SSHTransportTimeout(15, '', 'systemctl reboot'))
        command = 'cat /proc/sys/kernel/random/boot_id'
        for error in errors:
            with self.subTest(disconnect=type(error).__name__):
                guest = guests.Guest.__new__(guests.Guest)
                results = [subprocess.CompletedProcess([], 0, 'old-boot\n', ''),
                           error or subprocess.CompletedProcess([], 0, '', ''),
                           subprocess.CompletedProcess([], 0, 'old-boot\n', ''),
                           subprocess.CompletedProcess([], 0, 'new-boot\n', '')]

                def poll(action, timeout):
                    self.assertEqual(timeout, 1200)
                    with self.assertRaisesRegex(AssertionError, 'has not rebooted'):
                        action()
                    action()

                with mock.patch.object(guest, 'ssh', side_effect=results) as ssh, mock.patch.object(guests, 'wait_for', side_effect=poll) as wait, mock.patch.object(guests.time, 'sleep') as sleep, mock.patch.object(guests, 'report_stage'):
                    guest.reboot()
                self.assertEqual(ssh.call_args_list, [mock.call(command), mock.call('systemctl reboot', timeout=15),
                                                     mock.call(command, timeout=15), mock.call(command, timeout=15)])
                wait.assert_called_once_with(mock.ANY, timeout=guests.GUEST_BOOT_TIMEOUT)
                sleep.assert_called_once_with(10)

    def test_unchanged_or_empty_boot_id_fails_at_boot_deadline_without_reboot_retry(self):
        command = 'cat /proc/sys/kernel/random/boot_id'
        for after in ('old-boot', ''):
            with self.subTest(after=after):
                guest = guests.Guest.__new__(guests.Guest)
                results = [subprocess.CompletedProcess([], 0, 'old-boot', ''),
                           subprocess.CompletedProcess([], 0, '', ''),
                           subprocess.CompletedProcess([], 0, after, ''),
                           subprocess.CompletedProcess([], 0, after, '')]
                with mock.patch.object(guest, 'ssh', side_effect=results) as ssh, mock.patch.object(guests, 'report_stage'), mock.patch.object(guests.time, 'monotonic', side_effect=[0, 1199, 1200]), mock.patch.object(guests.time, 'sleep') as sleep:
                    with self.assertRaisesRegex(AssertionError, 'Guest has not rebooted'):
                        guest.reboot()
                self.assertEqual(ssh.call_args_list, [mock.call(command), mock.call('systemctl reboot', timeout=15),
                                                     mock.call(command, timeout=15), mock.call(command, timeout=15)])
                self.assertEqual(sleep.call_args_list, [mock.call(10), mock.call(3)])

    def test_failed_boot_readiness_preserves_original_transport_error(self):
        guest = guests.Guest.__new__(guests.Guest)
        command = 'cat /proc/sys/kernel/random/boot_id'
        original = guests.SSHTransportTimeout(15, 'Connection timed out', command)
        results = [subprocess.CompletedProcess([], 0, 'old-boot', ''),
                   subprocess.CompletedProcess([], 0, '', ''), original, original]
        with mock.patch.object(guest, 'ssh', side_effect=results) as ssh, mock.patch.object(guests, 'report_stage'), mock.patch.object(guests.time, 'monotonic', side_effect=[0, 1199, 1200]), mock.patch.object(guests.time, 'sleep'):
            with self.assertRaises(guests.SSHTransportTimeout) as failure:
                guest.reboot()
        self.assertIs(failure.exception, original)
        self.assertEqual(ssh.call_args_list, [mock.call(command), mock.call('systemctl reboot', timeout=15),
                                             mock.call(command, timeout=15), mock.call(command, timeout=15)])

    def test_real_reboot_command_refusal_fails_without_waiting_or_retrying(self):
        guest = guests.Guest.__new__(guests.Guest)
        refusal = subprocess.CalledProcessError(1, ['ssh', 'systemctl reboot'], stderr='Access denied')
        with mock.patch.object(guest, 'ssh', side_effect=[subprocess.CompletedProcess([], 0, 'old-boot\n', ''), refusal]) as ssh, mock.patch.object(guests, 'wait_for') as wait, mock.patch.object(guests.time, 'sleep') as sleep:
            with self.assertRaises(subprocess.CalledProcessError) as failure:
                guest.reboot()
        self.assertIs(failure.exception, refusal)
        self.assertEqual(ssh.call_count, 2)
        wait.assert_not_called()
        sleep.assert_not_called()


class PostRebootChecks(unittest.TestCase):
    def setUp(self):
        self.guest = guests.Guest.__new__(guests.Guest)
        self.guest.key = Path('/temporary/ssh-key')
        self.certs = Path('/temporary/certificates')
        self.https = ['curl', '-fsS', '--connect-timeout', '3', '--max-time', '10',
                      '--cacert', str(self.certs / 'combined.pem'), '--noproxy', guests.DOMAIN,
                      '--resolve', guests.DOMAIN + ':443:' + guests.GUEST, 'https://' + guests.DOMAIN + '/up']
        self.baseline = 'private environment checksum'

    def test_direct_verified_https_precedes_one_checked_status_with_ordinary_command_budget(self):
        results = [subprocess.CompletedProcess([], 0, '', ''),
                   subprocess.CompletedProcess([], 0, 'informational manager status', ''),
                   subprocess.CompletedProcess([], 0, self.baseline, '')]
        with mock.patch.object(guests.subprocess, 'run', side_effect=results) as run, mock.patch.object(guests, 'wait_for', wraps=guests.wait_for) as wait, mock.patch.object(guests, 'report_stage') as stage, mock.patch.object(guests, 'browser') as browser:
            guests.verify_reboot_persistence(self.guest, self.baseline, self.certs)
        self.assertEqual([call.args[0] for call in run.call_args_list], [
            self.https, self.guest.ssh_command('emberctl status'),
            self.guest.ssh_command('sha256sum /etc/ember/app.env')])
        self.assertEqual([call.kwargs for call in run.call_args_list], [
            {'check': True, 'text': True, 'stdout': subprocess.PIPE, 'stderr': subprocess.PIPE, 'timeout': 15},
            {'check': False, 'text': True, 'stdout': subprocess.PIPE, 'stderr': subprocess.PIPE, 'timeout': 900},
            {'check': False, 'text': True, 'stdout': subprocess.PIPE, 'stderr': subprocess.PIPE, 'timeout': 900}])
        wait.assert_called_once_with(mock.ANY)
        self.assertEqual(stage.call_args_list, [mock.call('reboot-health'), mock.call('reboot-status'), mock.call('reboot-persistence')])
        browser.assert_called_once_with('persist')

    def test_https_failures_exhaust_unchanged_readiness_window_before_status(self):
        failures = [subprocess.CalledProcessError(code, self.https, output='private response', stderr='private curl detail')
                    for code in (22, 28, 35, 60)]
        failures.append(subprocess.TimeoutExpired(self.https, 15, output='private response', stderr='private curl detail'))
        for error in failures:
            with self.subTest(error=type(error).__name__, code=getattr(error, 'returncode', None)):
                with mock.patch.object(guests.subprocess, 'run', side_effect=error) as run, mock.patch.object(guests, 'report_stage') as stage, mock.patch.object(guests.time, 'monotonic', side_effect=[0, 0, 600]), mock.patch.object(guests.time, 'sleep') as sleep, mock.patch.object(guests, 'browser') as browser, mock.patch.object(guests, 'write_diagnostic') as diagnostic:
                    with self.assertRaises(type(error)) as failure:
                        guests.verify_reboot_persistence(self.guest, self.baseline, self.certs)
                self.assertIs(failure.exception, error)
                self.assertEqual(run.call_args_list, [mock.call(self.https, check=True, text=True,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15)] * 2)
                stage.assert_called_once_with('reboot-health')
                sleep.assert_called_once_with(3)
                browser.assert_not_called()
                outcome = ({'probe': 'reboot-https', 'kind': 'curl-exit', 'exit_code': error.returncode}
                           if isinstance(error, subprocess.CalledProcessError)
                           else {'probe': 'reboot-https', 'kind': 'process-timeout', 'timeout_seconds': 15})
                diagnostic.assert_called_once_with('transport.log', guests.json.dumps(outcome, sort_keys=True) + '\n', emit=True)
                self.assertNotIn('private response', str(failure.exception))
                self.assertNotIn('private curl detail', str(failure.exception))

    def test_terminal_https_diagnostic_contains_only_fixed_outcome_and_preserves_exception(self):
        private = 'private command output path and credentials'
        failures = ((subprocess.CalledProcessError(60, [private], output=private, stderr=private),
                     {'probe': 'reboot-https', 'kind': 'curl-exit', 'exit_code': 60}),
                    (subprocess.TimeoutExpired([private], 15, output=private, stderr=private),
                     {'probe': 'reboot-https', 'kind': 'process-timeout', 'timeout_seconds': 15}),
                    (OSError(private), {'probe': 'reboot-https', 'kind': 'os-error'}))
        for error, expected in failures:
            with self.subTest(kind=expected['kind']), tempfile.TemporaryDirectory() as temporary:
                work = Path(temporary)
                (work / 'diagnostics').mkdir()
                stderr = io.StringIO()
                with mock.patch.object(guests, 'WORK', work), mock.patch.object(guests.subprocess, 'run', side_effect=error) as run, mock.patch.object(guests, 'report_stage'), mock.patch.object(guests.time, 'monotonic', side_effect=[0, 600]), mock.patch.object(guests, 'browser') as browser, contextlib.redirect_stderr(stderr):
                    with self.assertRaises(type(error)) as failure:
                        guests.verify_reboot_persistence(self.guest, self.baseline, self.certs)
                self.assertIs(failure.exception, error)
                detail = (work / 'diagnostics/transport.log').read_text()
                self.assertEqual(guests.json.loads(detail), expected)
                self.assertEqual(stderr.getvalue().strip(), detail.strip())
                self.assertNotIn(private, detail + stderr.getvalue())
                run.assert_called_once()
                browser.assert_not_called()

    def test_https_diagnostic_write_and_print_failures_preserve_original_exception(self):
        error = subprocess.CalledProcessError(60, self.https, output='private response', stderr='private curl detail')
        with mock.patch.object(guests.subprocess, 'run', side_effect=error), mock.patch.object(guests, 'report_stage'), mock.patch.object(guests.time, 'monotonic', side_effect=[0, 600]), mock.patch.object(guests, 'browser') as browser, mock.patch.object(Path, 'write_text', side_effect=OSError('disk full')), mock.patch('builtins.print', side_effect=OSError('stderr closed')):
            with self.assertRaises(subprocess.CalledProcessError) as failure:
                guests.verify_reboot_persistence(self.guest, self.baseline, self.certs)
        self.assertIs(failure.exception, error)
        browser.assert_not_called()

    def test_status_failure_or_timeout_is_fatal_and_never_retried(self):
        failures = ((subprocess.CompletedProcess([], 7, '', 'manager refusal'), subprocess.CalledProcessError),
                    (subprocess.CompletedProcess([], 255, '', 'connection reset'), guests.SSHTransportFailure),
                    (subprocess.TimeoutExpired(['private command'], 900, output='private stdout'), guests.SSHTransportTimeout))
        for result, expected in failures:
            with self.subTest(failure=expected.__name__):
                results = [subprocess.CompletedProcess([], 0, '', ''), result]
                with mock.patch.object(guests.subprocess, 'run', side_effect=results) as run, mock.patch.object(guests, 'report_stage') as stage, mock.patch.object(guests.time, 'sleep') as sleep, mock.patch.object(guests, 'browser') as browser:
                    with self.assertRaises(expected) as failure:
                        guests.verify_reboot_persistence(self.guest, self.baseline, self.certs)
                self.assertEqual([call.args[0] for call in run.call_args_list], [self.https, self.guest.ssh_command('emberctl status')])
                self.assertEqual(run.call_args_list[1].kwargs, {'check': False, 'text': True,
                    'stdout': subprocess.PIPE, 'stderr': subprocess.PIPE, 'timeout': 900})
                self.assertEqual(stage.call_args_list, [mock.call('reboot-health'), mock.call('reboot-status')])
                sleep.assert_not_called()
                browser.assert_not_called()
                if expected is guests.SSHTransportTimeout:
                    self.assertEqual(failure.exception.timeout, 900)
                    self.assertIsNone(failure.exception.output)

    def test_probe_and_status_output_remain_private(self):
        private = 'private response and credentials'
        results = [subprocess.CompletedProcess([], 0, private, private),
                   subprocess.CompletedProcess([], 0, private, private),
                   subprocess.CompletedProcess([], 0, self.baseline, '')]
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.object(guests.subprocess, 'run', side_effect=results), mock.patch.object(guests, 'browser'), contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            guests.verify_reboot_persistence(self.guest, self.baseline, self.certs)
        self.assertEqual(stdout.getvalue(), '')
        self.assertNotIn(private, stderr.getvalue())
        self.assertNotIn(self.baseline, stderr.getvalue())
        self.assertIn('reboot-health', stderr.getvalue())
        self.assertIn('reboot-status', stderr.getvalue())
        self.assertIn('reboot-persistence', stderr.getvalue())

    def test_changed_environment_still_fails_before_browser_persistence(self):
        results = [subprocess.CompletedProcess([], 0, '', ''), subprocess.CompletedProcess([], 0, '', ''),
                   subprocess.CompletedProcess([], 0, 'different checksum', '')]
        with mock.patch.object(guests.subprocess, 'run', side_effect=results) as run, mock.patch.object(guests, 'report_stage'), mock.patch.object(guests, 'browser') as browser:
            with self.assertRaises(AssertionError):
                guests.verify_reboot_persistence(self.guest, self.baseline, self.certs)
        self.assertEqual(run.call_count, 3)
        browser.assert_not_called()


class PublicRequests(unittest.TestCase):
    def test_public_bundle_and_guide_use_identified_requests_with_default_tls(self):
        version = '1.2.3'
        bundle = b'tested public installer bundle'
        bootstrap = '#!/bin/sh\n# tested installer\n'
        bundle_url = 'https://github.com/NYLLON-SOFTWARE/ember/releases/download/v1.2.3/ember-installer-1.2.3.tar.gz'
        manifest = {'version': version, 'bundle': {'url': bundle_url, 'sha256': guests.hashlib.sha256(bundle).hexdigest()}}
        guide = (guests.ROOT / 'deploy/release/guide.html').read_bytes().replace(b'{{VERSION}}', version.encode())
        fetched = []

        def public_response(request, **kwargs):
            self.assertIsInstance(request, guests.urllib.request.Request)
            self.assertEqual(request.get_method(), 'GET')
            self.assertEqual(request.get_header('User-agent'), 'Ember-public-release-verification')
            self.assertNotIn('Python', request.get_header('User-agent'))
            # No SSL context or transport override: urllib retains normal certificate verification.
            self.assertEqual(kwargs, {'timeout': 30})
            fetched.append(request.full_url)
            response = mock.MagicMock()
            response.__enter__.return_value = response
            response.status = 200
            response.headers.get_content_type.return_value = 'text/html'
            response.read.return_value = bundle if request.full_url == bundle_url else guide
            return response

        with tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
            assets = Path(temporary)
            (assets / 'release.json').write_text(guests.json.dumps(manifest))
            (assets / 'bootstrap.sh').write_text(bootstrap)
            stack.enter_context(mock.patch.object(guests.http.server, 'ThreadingHTTPServer'))
            stack.enter_context(mock.patch.object(guests.ssl, 'SSLContext'))
            stack.enter_context(mock.patch.object(guests.threading, 'Thread'))
            stack.enter_context(mock.patch.object(guests.atexit, 'register'))
            stack.enter_context(mock.patch.object(guests, 'run', side_effect=[
                subprocess.CompletedProcess([], 0, bootstrap, ''),
                subprocess.CompletedProcess([], 0, guests.json.dumps(manifest), '')]))
            open_request = stack.enter_context(mock.patch.object(guests.urllib.request, 'urlopen', side_effect=public_response))
            self.assertEqual(guests.mirror(assets, assets, public=True), {})
        self.assertEqual(fetched, [bundle_url, 'https://get.nyllon.com/'])
        self.assertEqual(open_request.call_count, 2)


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


class VerificationProgress(unittest.TestCase):
    def test_stage_output_contains_only_allowlisted_label_and_elapsed_time(self):
        output = io.StringIO()
        with mock.patch.object(guests, 'VERIFICATION_STARTED', 100), mock.patch.object(guests.time, 'monotonic', return_value=112.3), contextlib.redirect_stderr(output):
            for stage in sorted(guests.VERIFICATION_STAGES):
                guests.report_stage(stage)
            before = output.getvalue()
            with self.assertRaisesRegex(ValueError, '^Unknown verification stage$'):
                guests.report_stage('https://private.example/first_run/access#token=private-token')
            self.assertEqual(output.getvalue(), before)
        snapshots = [guests.json.loads(line.split(': ', 1)[1]) for line in output.getvalue().splitlines()]
        self.assertEqual({item['stage'] for item in snapshots}, guests.VERIFICATION_STAGES)
        for item in snapshots:
            self.assertEqual(set(item), {'stage', 'elapsed_seconds'})
            self.assertEqual(item['elapsed_seconds'], 12.3)
        for private in ('private.example', 'private-token', '#token=', 'systemctl', 'stdout', 'stderr'):
            self.assertNotIn(private, output.getvalue())

    def test_stage_reporting_cannot_fail_when_stderr_is_unavailable(self):
        with mock.patch('builtins.print', side_effect=OSError('Broken pipe')):
            guests.report_stage('reboot-ssh')

    def test_boot_reports_captured_startup_boundaries_without_command_output(self):
        guest = guests.Guest.__new__(guests.Guest)
        image = b'disposable cloud image'
        with tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
            root = Path(temporary)
            work = root / 'work'
            (work / 'diagnostics').mkdir(parents=True)
            (root / 'deploy/verification').mkdir(parents=True)
            guest.key = work / 'ssh'
            guest.key.with_suffix('.pub').write_text('ssh-ed25519 disposable-test-public-key')
            image_lock = {'ubuntu-24.04/arm64': {'url': 'https://example.invalid/base.qcow2',
                'algorithm': 'sha256', 'checksum': guests.hashlib.sha256(image).hexdigest()}}
            (root / 'deploy/verification/guests.lock.json').write_text(guests.json.dumps(image_lock))

            def run(*args, **kwargs):
                if args[0] == 'curl':
                    (work / 'base.qcow2').write_bytes(image)
                return subprocess.CompletedProcess([], 0, 'private command output', '')

            stack.enter_context(mock.patch.object(guests, 'ROOT', root))
            stack.enter_context(mock.patch.object(guests, 'WORK', work))
            stack.enter_context(mock.patch.object(guests, 'run', side_effect=run))
            popen = stack.enter_context(mock.patch.object(guests.subprocess, 'Popen'))
            stack.enter_context(mock.patch.object(guests.atexit, 'register'))
            monitor_directory = stack.enter_context(mock.patch.object(guests.tempfile, 'TemporaryDirectory'))
            wait = stack.enter_context(mock.patch.object(guest, 'wait_for_ssh'))
            ssh = stack.enter_context(mock.patch.object(guest, 'ssh', return_value=subprocess.CompletedProcess([], 0, 'private SSH output', 'private stderr')))
            output = stack.enter_context(contextlib.redirect_stderr(io.StringIO()))
            guest.boot('ubuntu-24.04', 'arm64')
            popen.call_args.kwargs['stdout'].close()
            monitor_directory.assert_not_called()
        snapshots = [guests.json.loads(line.split(': ', 1)[1]) for line in output.getvalue().splitlines()]
        self.assertEqual([item['stage'] for item in snapshots], ['guest-boot', 'guest-ssh', 'guest-cloud-init', 'guest-packages'])
        self.assertNotIn('private', output.getvalue())
        wait.assert_called_once_with()
        self.assertEqual(ssh.call_args_list, [mock.call('cloud-init status --wait'),
            mock.call('apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y curl ca-certificates python3')])


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



class QmpBootDiagnostics(unittest.TestCase):
    @contextlib.contextmanager
    def boot_fixture(self, source):
        with tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
            root = Path(temporary) / ('long-checkout-' * 10)
            work = root / 'tmp/installer-verification'
            (work / 'diagnostics').mkdir(parents=True)
            (root / 'deploy/verification').mkdir(parents=True)
            guest = guests.Guest.__new__(guests.Guest)
            guest.accel, guest.benchmark = 'kvm', True
            guest.key = work / 'ssh'
            guest.key.with_suffix('.pub').write_text('ssh-ed25519 disposable-test-public-key')
            image = b'disposable cloud image'
            lock = {'ubuntu-24.04/arm64': {'url': 'https://example.invalid/base.qcow2',
                'algorithm': 'sha256', 'checksum': guests.hashlib.sha256(image).hexdigest()}}
            (root / 'deploy/verification/guests.lock.json').write_text(guests.json.dumps(lock))
            paths, permissions, children = [], [], []
            real_popen = subprocess.Popen

            def run(*args, **kwargs):
                if args[0] == 'curl':
                    (work / 'base.qcow2').write_bytes(image)
                return subprocess.CompletedProcess([], 0, '', '')

            def popen(command, **kwargs):
                path = Path(command[command.index('-qmp') + 1].removeprefix('unix:').split(',')[0])
                paths.append(path)
                permissions.append(path.parent.stat().st_mode & 0o777)
                child = real_popen([sys.executable, '-u', '-c', source, str(path)], **kwargs)
                children.append(child)
                return child

            stack.enter_context(mock.patch.object(guests, 'ROOT', root))
            stack.enter_context(mock.patch.object(guests, 'WORK', work))
            stack.enter_context(mock.patch.object(guests, 'run', side_effect=run))
            stack.enter_context(mock.patch.object(guests.subprocess, 'Popen', side_effect=popen))
            stack.enter_context(mock.patch.object(guests.atexit, 'register'))
            wait = stack.enter_context(mock.patch.object(guest, 'wait_for_ssh'))
            ssh = stack.enter_context(mock.patch.object(guest, 'ssh'))
            output = stack.enter_context(contextlib.redirect_stderr(io.StringIO()))
            try:
                yield guest, work, paths, permissions, wait, ssh, output
            finally:
                for child in children:
                    if child.poll() is None:
                        child.terminate()
                    child.wait(timeout=5)

    def test_long_checkout_uses_real_short_private_socket_and_cleans_after_proof(self):
        source = """
import json, socket, sys
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
    server.bind(sys.argv[1]); server.listen(1); server.settimeout(5)
    with server.accept()[0] as connection, connection.makefile('rwb') as stream:
        stream.write(b'{"QMP":{"version":{"qemu":{"major":10,"minor":2,"micro":1}}}}\\n'); stream.flush()
        assert json.loads(stream.readline()) == {'execute': 'qmp_capabilities'}
        stream.write(b'{"return":{}}\\n'); stream.flush()
        assert json.loads(stream.readline()) == {'execute': 'query-kvm'}
        stream.write(b'{"return":{"present":true,"enabled":true}}\\n'); stream.flush()
"""
        with self.boot_fixture(source) as (guest, work, paths, permissions, wait, ssh, output):
            self.assertGreater(len(guests.os.fsencode(work / 'qmp.sock')), 107)
            guest.boot('ubuntu-24.04', 'arm64')
            self.assertEqual(guest.acceleration, {'accelerator': 'kvm', 'kvm_enabled': True,
                'qemu_version': {'major': 10, 'minor': 2, 'micro': 1}})
            self.assertEqual(len(paths), 1)
            self.assertLess(len(guests.os.fsencode(paths[0])), 104)
            self.assertEqual(paths[0].parent.parent, Path('/tmp'))
            self.assertNotIn(str(work), str(paths[0]))
            self.assertEqual(permissions, [0o700])
            self.assertFalse(paths[0].parent.exists())
            wait.assert_called_once_with()
            self.assertEqual(ssh.call_count, 2)
            self.assertNotIn('readiness failed', output.getvalue())

    def test_real_child_early_exit_is_prompt_and_emits_bounded_redacted_boot_evidence(self):
        source = """
print('x' * 30000)
print('\\x1b[31mfirmware startup failed\\x1b[0m')
print('SECRET_KEY_BASE=private-secret')
print('VAPID_PRIVATE_KEY=private-vapid')
print('EMBER_SETUP_TOKEN=private-env-token')
print('https://chat.ember.test/first_run/access#token=private-fragment')
print('Cookie: session=private-cookie')
print('Authorization: Bearer private-authorization')
raise SystemExit(23)
"""
        with self.boot_fixture(source) as (guest, work, paths, permissions, wait, ssh, output):
            started = guests.time.monotonic()
            with self.assertRaisesRegex(RuntimeError, '^QEMU exited before QMP became available$') as raised:
                guest.boot('ubuntu-24.04', 'arm64')
            self.assertLess(guests.time.monotonic() - started, 3)
            wait.assert_not_called()
            ssh.assert_not_called()
            diagnostic = (work / 'diagnostics/boot.log').read_text()
            self.assertEqual(output.getvalue().split('Guest QMP readiness failed;', 1)[1], diagnostic.split('Guest QMP readiness failed;', 1)[1] + '\n')
            self.assertIn('Guest QMP readiness failed; QEMU exit status: 23', diagnostic)
            self.assertIn('firmware startup failed', diagnostic)
            self.assertLess(len(diagnostic), 17000)
            self.assertFalse(paths[0].parent.exists())
            for private in ('private-secret', 'private-vapid', 'private-env-token', 'private-fragment',
                            'private-cookie', 'private-authorization', '\x1b'):
                self.assertNotIn(private, diagnostic + output.getvalue() + str(raised.exception))

    def test_failed_proof_cleans_socket_directory_and_preserves_error_when_diagnostics_fail(self):
        failure = RuntimeError('QMP accelerator proof timed out')
        write_text = Path.write_text

        def write(path, *args, **kwargs):
            if path.name == 'boot.log':
                raise OSError('No space left on device')
            return write_text(path, *args, **kwargs)

        with self.boot_fixture('import time; time.sleep(30)') as (guest, work, paths, permissions, wait, ssh, output):
            with mock.patch.object(guests, 'qmp_acceleration', side_effect=failure) as proof, \
                    mock.patch.object(Path, 'write_text', new=write), \
                    mock.patch('builtins.print', side_effect=OSError('Broken pipe')):
                with self.assertRaises(RuntimeError) as raised:
                    guest.boot('ubuntu-24.04', 'arm64')
            self.assertIs(raised.exception, failure)
            self.assertFalse(paths[0].parent.exists())
            self.assertEqual(proof.call_args.args, ('kvm', paths[0], guest.process))
            wait.assert_not_called()
            ssh.assert_not_called()


class QemuAcceleration(unittest.TestCase):
    def test_default_tcg_keeps_existing_machines_cpu_firmware_and_resources(self):
        for arch, executable, machine in (('amd64', 'qemu-system-x86_64', 'q35'), ('arm64', 'qemu-system-aarch64', 'virt')):
            with self.subTest(arch=arch):
                command = guests.qemu_command(arch, 'tcg')
                self.assertEqual(command[:5], [executable, '-machine', machine, '-cpu', 'max'])
                self.assertEqual(command[command.index('-accel') + 1], 'tcg')
                self.assertEqual(command[command.index('-smp') + 1], '2')
                self.assertEqual(command[command.index('-m') + 1], '4096')
                self.assertNotIn('-qmp', command)
                self.assertIn('tap,id=net0,ifname=embertap0,script=no,downscript=no', command)
                if arch == 'arm64':
                    self.assertEqual(command[command.index('-bios') + 1], '/usr/share/qemu-efi-aarch64/QEMU_EFI.fd')

    def test_kvm_uses_only_one_accelerator_with_native_cpu_and_arm_host_gic(self):
        for arch in ('amd64', 'arm64'):
            with self.subTest(arch=arch):
                command = guests.qemu_command(arch, 'kvm', Path('/tmp/private-qmp/qmp.sock'))
                self.assertEqual(command.count('-accel'), 1)
                self.assertEqual(command[command.index('-accel') + 1], 'kvm')
                self.assertNotIn('tcg', command)
                self.assertEqual(command[command.index('-cpu') + 1], 'host')
                self.assertEqual(command[command.index('-machine') + 1], 'q35' if arch == 'amd64' else 'virt,gic-version=host')
                self.assertEqual(command[command.index('-qmp') + 1], 'unix:/tmp/private-qmp/qmp.sock,server=on,wait=off')
        for arch, accel in (('riscv64', 'kvm'), ('arm64', 'auto'), ('arm64', 'kvm:tcg')):
            with self.subTest(arch=arch, accel=accel), self.assertRaises(AssertionError):
                guests.qemu_command(arch, accel)

    def test_kvm_preflight_requires_native_linux_before_opening_device(self):
        for system, machine, arch in (('Darwin', 'aarch64', 'arm64'), ('Linux', 'x86_64', 'arm64'), ('Linux', 'aarch64', 'amd64')):
            with self.subTest(system=system, machine=machine, arch=arch), mock.patch.object(guests.platform, 'system', return_value=system), mock.patch.object(guests.platform, 'machine', return_value=machine), mock.patch('builtins.open') as opened:
                with self.assertRaisesRegex(AssertionError, 'native Linux'):
                    guests.require_kvm(arch)
                opened.assert_not_called()

    def test_kvm_probe_checks_api_and_creates_and_closes_a_vm(self):
        opened = mock.mock_open()
        with mock.patch.object(guests.platform, 'system', return_value='Linux'), mock.patch.object(guests.platform, 'machine', return_value='aarch64'), mock.patch('builtins.open', opened), mock.patch.object(guests.fcntl, 'ioctl', side_effect=[12, 37]) as ioctl, mock.patch.object(guests.os, 'close') as close:
            guests.require_kvm('arm64')
        opened.assert_called_once_with('/dev/kvm', 'rb+', buffering=0)
        self.assertEqual(ioctl.call_args_list, [mock.call(opened(), 0xAE00, 0), mock.call(opened(), 0xAE01, 0)])
        close.assert_called_once_with(37)
        opened().__exit__.assert_called_once()

    def test_denied_missing_unsupported_and_uncreatable_kvm_fail_without_fallback(self):
        for failure in (PermissionError('private permission detail'), FileNotFoundError('private device detail')):
            with self.subTest(failure=type(failure).__name__), mock.patch.object(guests.platform, 'system', return_value='Linux'), mock.patch.object(guests.platform, 'machine', return_value='aarch64'), mock.patch('builtins.open', side_effect=failure), mock.patch.object(guests.fcntl, 'ioctl') as ioctl:
                with self.assertRaisesRegex(RuntimeError, '^KVM is unavailable on this disposable runner$') as raised:
                    guests.require_kvm('arm64')
                self.assertNotIn('private', str(raised.exception))
                ioctl.assert_not_called()
        for replies, message in (([11], 'Unsupported KVM API version'), ([12, OSError('private ioctl detail')], 'KVM is unavailable on this disposable runner')):
            with self.subTest(message=message), mock.patch.object(guests.platform, 'system', return_value='Linux'), mock.patch.object(guests.platform, 'machine', return_value='aarch64'), mock.patch('builtins.open', mock.mock_open()), mock.patch.object(guests.fcntl, 'ioctl', side_effect=replies), mock.patch.object(guests.os, 'close') as close:
                with self.assertRaisesRegex(RuntimeError, '^' + message + '$'):
                    guests.require_kvm('arm64')
                close.assert_not_called()

    def test_real_qmp_socket_negotiates_and_rejects_fallback_without_exposing_messages(self):
        for enabled, requested in ((True, 'kvm'), (False, 'tcg'), (False, 'kvm')):
            with self.subTest(enabled=enabled, requested=requested), tempfile.TemporaryDirectory(prefix='ember-qmp-', dir='/tmp') as temporary:
                work = Path(temporary)
                requests, errors = [], []
                with guests.socket.socket(guests.socket.AF_UNIX, guests.socket.SOCK_STREAM) as server:
                    server.bind(str(work / 'qmp.sock'))
                    server.listen(1)
                    server.settimeout(5)

                    def serve():
                        try:
                            with server.accept()[0] as connection, connection.makefile('rwb') as stream:
                                stream.write(b'{"QMP":{"version":{"qemu":{"major":10,"minor":1,"micro":2},"package":"private-host-detail"},"capabilities":[]}}\n')
                                stream.flush()
                                requests.append(guests.json.loads(stream.readline()))
                                stream.write(b'{"return":{}}\n')
                                stream.flush()
                                requests.append(guests.json.loads(stream.readline()))
                                stream.write(b'{"event":"PRIVATE_EVENT","data":{"token":"private-token"}}\n')
                                stream.write((guests.json.dumps({'return': {'enabled': enabled, 'present': True, 'private': 'private-token'}}) + '\n').encode())
                                stream.flush()
                        except BaseException as error:
                            errors.append(error)

                    worker = threading.Thread(target=serve, daemon=True)
                    worker.start()
                    with mock.patch.object(guests, 'WORK', work):
                        if requested == 'kvm' and not enabled:
                            with self.assertRaisesRegex(RuntimeError, '^QEMU did not enable the requested accelerator$') as raised:
                                guests.qmp_acceleration(requested, work / 'qmp.sock')
                            self.assertNotIn('private', str(raised.exception))
                        else:
                            result = guests.qmp_acceleration(requested, work / 'qmp.sock')
                            self.assertEqual(result, {'accelerator': requested, 'kvm_enabled': enabled, 'qemu_version': {'major': 10, 'minor': 1, 'micro': 2}})
                    worker.join(5)
                    self.assertFalse(worker.is_alive())
                self.assertEqual(errors, [])
                self.assertEqual(requests, [{'execute': 'qmp_capabilities'}, {'execute': 'query-kvm'}])


    def test_real_qmp_absolute_deadline_bounds_continuous_trickling_at_each_read_phase(self):
        for phase in ('greeting', 'query'):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory(prefix='ember-qmp-', dir='/tmp') as temporary:
                work = Path(temporary)
                stop, errors = threading.Event(), []
                with guests.socket.socket(guests.socket.AF_UNIX, guests.socket.SOCK_STREAM) as server:
                    server.bind(str(work / 'qmp.sock'))
                    server.listen(1)
                    server.settimeout(2)

                    def serve():
                        try:
                            with server.accept()[0] as connection:
                                connection.settimeout(2)
                                if phase == 'query':
                                    connection.sendall(b'{"QMP":{"version":{"qemu":{"major":10,"minor":1,"micro":2}}}}\n')
                                    with connection.makefile('rb') as stream:
                                        self.assertEqual(guests.json.loads(stream.readline()), {'execute': 'qmp_capabilities'})
                                        connection.sendall(b'{"return":{}}\n')
                                        self.assertEqual(guests.json.loads(stream.readline()), {'execute': 'query-kvm'})
                                connection.sendall(b'{"private-token":"')
                                while not stop.wait(0.01):
                                    connection.sendall(b'x')
                        except (BrokenPipeError, ConnectionResetError):
                            pass
                        except BaseException as error:
                            errors.append(error)

                    worker = threading.Thread(target=serve, daemon=True)
                    worker.start()
                    output = io.StringIO()
                    started = guests.time.monotonic()
                    try:
                        with mock.patch.object(guests, 'WORK', work), contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                            with self.assertRaisesRegex(RuntimeError, '^QMP accelerator proof timed out$') as raised:
                                guests.qmp_acceleration('kvm', work / 'qmp.sock', timeout=0.12)
                    finally:
                        stop.set()
                        worker.join(2)
                    self.assertLess(guests.time.monotonic() - started, 1)
                    self.assertFalse(worker.is_alive())
                    self.assertEqual(errors, [])
                    self.assertEqual(output.getvalue(), '')
                    self.assertNotIn('private', str(raised.exception))

    def test_qmp_startup_retries_share_the_same_absolute_deadline(self):
        with tempfile.TemporaryDirectory(prefix='ember-qmp-', dir='/tmp') as temporary:
            started = guests.time.monotonic()
            with mock.patch.object(guests, 'WORK', Path(temporary)), self.assertRaisesRegex(RuntimeError, '^QMP accelerator proof timed out$'):
                guests.qmp_acceleration('kvm', Path(temporary) / 'qmp.sock', timeout=0.05)
            self.assertLess(guests.time.monotonic() - started, 0.5)


class DisposableBenchmark(unittest.TestCase):
    common = {'anonymous-install', 'advertised-command-real-terminal-hostname', 'private-browser-setup',
              'chat', 'upload', 'websocket', 'push-enrollment', 'rerun', 'offline-ca-reboot', 'complete-backup-restore'}
    extra = {'interrupted-download', 'registry-unavailable-preserves-running-app', 'update', 'failed-start-explicit-recovery'}

    def test_local_runner_requires_explicit_opt_in_and_still_requires_native_root_linux(self):
        for github, benchmark, accepted in ((False, False, False), (False, True, True), (True, False, True)):
            with self.subTest(github=github, benchmark=benchmark), mock.patch.dict(guests.os.environ, {'GUEST_ARCH': 'arm64', 'GITHUB_ACTIONS': 'true' if github else ''}), mock.patch.object(guests.os, 'geteuid', return_value=0), mock.patch.object(guests.platform, 'system', return_value='Linux'), mock.patch.object(guests.platform, 'machine', return_value='aarch64'):
                if accepted:
                    guests.require_runner(benchmark)
                else:
                    with self.assertRaisesRegex(AssertionError, 'benchmark-disposable-runner'):
                        guests.require_runner(benchmark)
        for uid, system, machine in ((1000, 'Linux', 'aarch64'), (0, 'Darwin', 'aarch64'), (0, 'Linux', 'x86_64')):
            with self.subTest(uid=uid, system=system, machine=machine), mock.patch.dict(guests.os.environ, {'GUEST_ARCH': 'arm64'}), mock.patch.object(guests.os, 'geteuid', return_value=uid), mock.patch.object(guests.platform, 'system', return_value=system), mock.patch.object(guests.platform, 'machine', return_value=machine), self.assertRaises(AssertionError):
                guests.require_runner(True)

    def test_cli_defaults_and_explicit_benchmark_flags(self):
        for flags, expected in (([], (False, 'tcg')), (['--benchmark-disposable-runner', '--accel', 'kvm'], (True, 'kvm'))):
            with self.subTest(flags=flags), mock.patch.object(guests.sys, 'argv', ['guests.py', '--assets', '/temporary/assets', *flags]), mock.patch.object(guests, 'require_runner', side_effect=RuntimeError('stop before host mutation')) as require:
                with self.assertRaisesRegex(RuntimeError, '^stop before host mutation$'):
                    guests.main()
                require.assert_called_once_with(expected[0])
        with mock.patch.object(guests.sys, 'argv', ['guests.py', '--assets', '/temporary/assets', '--accel', 'kvm:tcg']), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            guests.main()

    def test_main_passes_flags_and_checks_kvm_before_network_with_no_default_benchmark(self):
        for benchmark, accel in ((False, 'tcg'), (True, 'kvm')):
            with self.subTest(benchmark=benchmark, accel=accel), tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
                work = Path(temporary) / 'fresh-work'
                flags = ['--benchmark-disposable-runner', '--accel', accel] if benchmark else []
                stack.enter_context(mock.patch.object(guests.sys, 'argv', ['guests.py', '--assets', temporary, *flags]))
                stack.enter_context(mock.patch.object(guests, 'WORK', work))
                stack.enter_context(mock.patch.object(guests, 'BENCHMARK_TIMINGS', None))
                stack.enter_context(mock.patch.object(guests, 'require_runner'))
                stack.enter_context(mock.patch.dict(guests.os.environ, {'GUEST_OS': 'ubuntu-24.04', 'GUEST_ARCH': 'arm64', 'PUBLIC_INSTALLER': 'false'}))
                order = mock.Mock()
                kvm = stack.enter_context(mock.patch.object(guests, 'require_kvm'))
                network = stack.enter_context(mock.patch.object(guests, 'network'))
                order.attach_mock(kvm, 'kvm')
                order.attach_mock(network, 'network')
                stack.enter_context(mock.patch.object(guests, 'certificates'))
                stack.enter_context(mock.patch.object(guests, 'mirror'))
                factory = stack.enter_context(mock.patch.object(guests, 'Guest'))
                acceptance = stack.enter_context(mock.patch.object(guests, 'acceptance'))
                writer = stack.enter_context(mock.patch.object(guests, 'write_benchmark'))
                guests.main()
                factory.assert_called_once_with(accel, benchmark)
                acceptance.assert_called_once()
                if benchmark:
                    self.assertEqual(order.mock_calls, [mock.call.kvm('arm64'), mock.call.network(False)])
                    writer.assert_called_once_with(factory.return_value, Path(temporary).resolve(), False)
                    self.assertEqual(guests.BENCHMARK_TIMINGS, {})
                else:
                    kvm.assert_not_called()
                    writer.assert_not_called()
                    self.assertIsNone(guests.BENCHMARK_TIMINGS)

    def test_failed_kvm_preflight_prevents_network_workspace_and_acceptance(self):
        with tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
            work = Path(temporary) / 'fresh-work'
            stack.enter_context(mock.patch.object(guests.sys, 'argv', ['guests.py', '--assets', temporary, '--benchmark-disposable-runner', '--accel', 'kvm']))
            stack.enter_context(mock.patch.object(guests, 'WORK', work))
            stack.enter_context(mock.patch.object(guests, 'require_runner'))
            stack.enter_context(mock.patch.dict(guests.os.environ, {'GUEST_ARCH': 'arm64'}))
            stack.enter_context(mock.patch.object(guests, 'require_kvm', side_effect=RuntimeError('KVM unavailable')))
            network = stack.enter_context(mock.patch.object(guests, 'network'))
            acceptance = stack.enter_context(mock.patch.object(guests, 'acceptance'))
            with self.assertRaisesRegex(RuntimeError, '^KVM unavailable$'):
                guests.main()
            self.assertFalse(work.exists())
            network.assert_not_called()
            acceptance.assert_not_called()

    def test_completed_metrics_contain_only_bound_hashes_fixed_metadata_and_numeric_timings(self):
        for public in (False, True):
            with self.subTest(public=public), tempfile.TemporaryDirectory() as temporary:
                work = Path(temporary)
                (work / 'diagnostics').mkdir()
                (work / 'release.json').write_text('{"private":"manifest body is hashed only"}')
                result = {'os': 'ubuntu-24.04', 'arch': 'arm64', 'public': public, 'gates': sorted(self.common if public else self.common | self.extra)}
                result_file = work / 'diagnostics/result.json'
                result_file.write_text(guests.json.dumps(result))
                before = result_file.read_bytes()
                order = [stage for stage in guests.BENCHMARK_STAGE_ORDER if not public or stage not in ('update', 'explicit-recovery')]
                timings = {stage: index * 1.5 for index, stage in enumerate(order)}
                guest = mock.Mock(acceleration={'accelerator': 'kvm', 'kvm_enabled': True, 'qemu_version': {'major': 10, 'minor': 1, 'micro': 2}, 'token': 'private-setup-token', 'command': 'private command'})
                output = io.StringIO()
                with mock.patch.object(guests, 'WORK', work), mock.patch.object(guests, 'BENCHMARK_TIMINGS', timings), mock.patch.object(guests.platform, 'machine', return_value='aarch64'), mock.patch.dict(guests.os.environ, {'GUEST_OS': 'ubuntu-24.04', 'EMBER_SETUP_TOKEN': 'private-env-token'}), contextlib.redirect_stdout(output):
                    guests.write_benchmark(guest, work, public)
                metrics = guests.json.loads((work / 'diagnostics/benchmark.json').read_text())
                self.assertEqual(metrics['gate_count'], 10 if public else 14)
                self.assertTrue(metrics['success'] and metrics['kvm_enabled'])
                self.assertEqual(metrics['stage_elapsed_seconds'], timings)
                self.assertEqual(metrics['stage_duration_seconds'], {stage: 1.5 for stage in order[:-1]})
                self.assertEqual(metrics['result_sha256'], guests.hashlib.sha256(before).hexdigest())
                self.assertEqual(metrics['release_manifest_sha256'], guests.hashlib.sha256((work / 'release.json').read_bytes()).hexdigest())
                self.assertEqual(result_file.read_bytes(), before)
                self.assertEqual(metrics['guest_vcpus'], 2)
                self.assertEqual(metrics['guest_memory_mib'], 4096)
                self.assertNotIn('private', (work / 'diagnostics/benchmark.json').read_text() + output.getvalue())

    def test_incomplete_gates_bad_timings_or_fallback_cannot_write_success_metrics(self):
        for invalid in ('missing-gate', 'duplicate-gate', 'wrong-public', 'missing-stage', 'private-stage', 'nan-time', 'backward-time', 'fallback'):
            with self.subTest(invalid=invalid), tempfile.TemporaryDirectory() as temporary:
                work = Path(temporary)
                (work / 'diagnostics').mkdir()
                result = {'os': 'ubuntu-24.04', 'arch': 'arm64', 'public': False, 'gates': sorted(self.common | self.extra)}
                timings = {stage: float(index) for index, stage in enumerate(guests.BENCHMARK_STAGE_ORDER)}
                acceleration = {'accelerator': 'kvm', 'kvm_enabled': True, 'qemu_version': {'major': 10, 'minor': 1, 'micro': 2}}
                if invalid == 'missing-gate': result['gates'].pop()
                elif invalid == 'duplicate-gate': result['gates'][-1] = result['gates'][0]
                elif invalid == 'wrong-public': result['public'] = True
                elif invalid == 'missing-stage': timings.pop('complete')
                elif invalid == 'private-stage': timings['private-token'] = 3
                elif invalid == 'nan-time': timings['complete'] = float('nan')
                elif invalid == 'backward-time': timings['complete'] = 0
                elif invalid == 'fallback': acceleration['kvm_enabled'] = False
                (work / 'diagnostics/result.json').write_text(guests.json.dumps(result))
                output = io.StringIO()
                with mock.patch.object(guests, 'WORK', work), mock.patch.object(guests, 'BENCHMARK_TIMINGS', timings), mock.patch.object(guests.platform, 'machine', return_value='aarch64'), mock.patch.dict(guests.os.environ, {'GUEST_OS': 'ubuntu-24.04'}), contextlib.redirect_stdout(output), self.assertRaises(RuntimeError):
                    guests.write_benchmark(mock.Mock(acceleration=acceleration), work, False)
                self.assertFalse((work / 'diagnostics/benchmark.json').exists())
                self.assertEqual(output.getvalue(), '')


if __name__ == "__main__":
    unittest.main()
