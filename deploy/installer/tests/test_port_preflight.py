"""Real TCP sockets distinguish listeners from recently closed connections."""
import errno
import importlib.util
from pathlib import Path
import socket
import unittest

spec = importlib.util.spec_from_file_location("emberctl_ports", Path(__file__).parents[1] / "emberctl.py")
manager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(manager)


class PortPreflight(unittest.TestCase):
    def listener(self, family=socket.AF_INET):
        listener = socket.socket(family, socket.SOCK_STREAM)
        self.addCleanup(listener.close)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if family == socket.AF_INET6:
            listener.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
        listener.bind(("0.0.0.0" if family == socket.AF_INET else "::", 0))
        listener.listen(1)
        listener.settimeout(5)
        return listener

    def test_closed_accepted_connection_does_not_block_preflight(self):
        listener = self.listener()
        port = listener.getsockname()[1]
        with socket.create_connection(("127.0.0.1", port), timeout=5) as client:
            accepted, _address = listener.accept()
            with accepted:
                # The server actively closes, leaving its local port in TIME_WAIT.
                accepted.shutdown(socket.SHUT_WR)
            self.assertEqual(client.recv(1), b"")
        listener.close()
        with socket.socket() as old_probe:
            with self.assertRaises(OSError) as failure:
                old_probe.bind(("0.0.0.0", port))
            self.assertEqual(failure.exception.errno, errno.EADDRINUSE)
        manager.require_available_port(port)

    def test_active_ipv4_listener_is_still_refused(self):
        listener = self.listener()
        port = listener.getsockname()[1]
        with self.assertRaisesRegex(manager.Failure, f"TCP port {port} is already occupied"):
            manager.require_available_port(port)

    def test_active_ipv6_listener_is_still_refused(self):
        try:
            listener = self.listener(socket.AF_INET6)
        except OSError as error:
            if error.errno in {errno.EAFNOSUPPORT, errno.EADDRNOTAVAIL, errno.EPROTONOSUPPORT}:
                self.skipTest("IPv6 is disabled on this host")
            raise
        port = listener.getsockname()[1]
        with self.assertRaisesRegex(manager.Failure, f"TCP port {port} is already occupied"):
            manager.require_available_port(port)


if __name__ == "__main__":
    unittest.main()
