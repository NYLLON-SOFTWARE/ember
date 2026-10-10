"""Fast transport contracts; full certificate and Docker acceptance runs in QEMU."""
import importlib.util
from pathlib import Path
import subprocess
import sys
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


if __name__ == "__main__":
    unittest.main()
