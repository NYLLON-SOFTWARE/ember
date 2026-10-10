import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('guests', Path(__file__).parents[2] / 'verification/guests.py')
guests = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guests)


class AcceptanceTimeouts(unittest.TestCase):
    def test_commands_have_a_deadline_and_support_short_probes(self):
        with patch.object(guests.subprocess, 'run') as command:
            guests.run('probe')
            self.assertEqual(command.call_args.kwargs['timeout'], 600)
            guests.run('probe', timeout=15)
            self.assertEqual(command.call_args.kwargs['timeout'], 15)

    def test_timed_out_probes_retry_until_overall_deadline(self):
        failure = subprocess.TimeoutExpired('probe', 15)
        with patch.object(guests.time, 'monotonic', side_effect=[0, .1, 1.1]), \
             patch.object(guests.time, 'sleep') as sleep, \
             patch.object(guests, 'run', side_effect=failure) as action:
            with self.assertRaises(subprocess.TimeoutExpired):
                guests.wait_for(lambda: guests.run('probe'), timeout=1)
            self.assertEqual(action.call_count, 2)
            sleep.assert_called_once_with(3)

    def test_ssh_probes_bound_silent_connections(self):
        guest = guests.Guest.__new__(guests.Guest)
        guest.key = Path('/tmp/disposable-key')
        with patch.object(guests, 'run') as command:
            guest.ssh('true', timeout=15)
            self.assertEqual(command.call_args.kwargs['timeout'], 15)
            self.assertIn('ServerAliveInterval=5', command.call_args.args)
            self.assertIn('ServerAliveCountMax=3', command.call_args.args)
