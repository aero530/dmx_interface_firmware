#!/usr/bin/env python3
"""Art-Net discovery, without installing a lighting console.

Broadcasts (or unicasts) an ArtPoll and decodes every ArtPollReply that comes
back, which is exactly what Stage 5 of docs/BRINGUP.md asks you to verify. No
dependencies beyond the standard library.

    python tools/artpoll.py                    # broadcast, find everything
    python tools/artpoll.py --target 192.168.86.36   # ask one node directly
    python tools/artpoll.py --wait 5 --raw     # longer listen, with hex dumps

A node answers one ArtPollReply per *bind* — at most four universes each — so a
healthy 8-port board bound to 32 universes replies eight times, BindIndex 1..8.
That is the property worth checking: controllers auto-patch from these replies,
so a node that under-reports its binds is a node whose later universes never get
patched.
"""

from __future__ import annotations

import argparse
import socket
import struct
import sys
import time

ART_NET_ID = b"Art-Net\x00"
ART_NET_PORT = 6454
OP_POLL = 0x2000
OP_POLL_REPLY = 0x2100
PROTOCOL_VERSION = 14

# Offsets into ArtPollReply. Verified against the firmware's serialiser in
# common/src/artnet/tiny_artnet/poll_reply.rs, which follows the Art-Net 4
# layout field for field.
OFF_IP = 10
OFF_PORT = 14
OFF_VERSION = 16
OFF_NET_SWITCH = 18
OFF_SUB_SWITCH = 19
OFF_OEM = 20
OFF_STATUS1 = 23
OFF_SHORT_NAME = 26
OFF_LONG_NAME = 44
OFF_NODE_REPORT = 108
OFF_NUM_PORTS = 172
OFF_PORT_TYPES = 174
OFF_GOOD_INPUT = 178
OFF_GOOD_OUTPUT = 182
OFF_SW_IN = 186
OFF_SW_OUT = 190
OFF_STYLE = 200
OFF_MAC = 201
OFF_BIND_IP = 207
OFF_BIND_INDEX = 211
OFF_STATUS2 = 212
MIN_REPLY_LEN = 213


def build_poll() -> bytes:
    """An ArtPoll. TalkToMe = 0: reply only to this poll, not on every change."""
    return ART_NET_ID + struct.pack("<H", OP_POLL) + bytes([0, PROTOCOL_VERSION, 0x00, 0x00])


def text(raw: bytes) -> str:
    return raw.split(b"\x00", 1)[0].decode("ascii", "replace").strip()


def mac_text(raw: bytes) -> str:
    return ":".join(f"{b:02x}" for b in raw)


class Reply:
    def __init__(self, data: bytes, source: str):
        self.source = source
        self.raw = data
        self.ip = ".".join(str(b) for b in data[OFF_IP:OFF_IP + 4])
        self.port = struct.unpack_from("<H", data, OFF_PORT)[0]
        self.version = struct.unpack_from(">H", data, OFF_VERSION)[0]
        self.net = data[OFF_NET_SWITCH]
        self.sub = data[OFF_SUB_SWITCH]
        self.oem = struct.unpack_from(">H", data, OFF_OEM)[0]
        self.status1 = data[OFF_STATUS1]
        self.short_name = text(data[OFF_SHORT_NAME:OFF_SHORT_NAME + 18])
        self.long_name = text(data[OFF_LONG_NAME:OFF_LONG_NAME + 64])
        self.node_report = text(data[OFF_NODE_REPORT:OFF_NODE_REPORT + 64])
        self.num_ports = struct.unpack_from(">H", data, OFF_NUM_PORTS)[0]
        self.port_types = data[OFF_PORT_TYPES:OFF_PORT_TYPES + 4]
        self.good_output = data[OFF_GOOD_OUTPUT:OFF_GOOD_OUTPUT + 4]
        self.sw_out = data[OFF_SW_OUT:OFF_SW_OUT + 4]
        self.style = data[OFF_STYLE]
        self.mac = mac_text(data[OFF_MAC:OFF_MAC + 6])
        self.bind_ip = ".".join(str(b) for b in data[OFF_BIND_IP:OFF_BIND_IP + 4])
        self.bind_index = data[OFF_BIND_INDEX]
        self.status2 = data[OFF_STATUS2]

    @property
    def universes(self) -> list[int]:
        """Full 15-bit Port-Addresses this reply claims to bind."""
        n = min(self.num_ports, 4)
        return [(self.net << 8) | (self.sub << 4) | (self.sw_out[i] & 0x0F) for i in range(n)]

    def universe_text(self) -> str:
        parts = [f"{self.net}:{self.sub}:{u & 0x0F}" for u in self.universes]
        return ", ".join(parts) if parts else "(none)"


def broadcast_targets() -> list[str]:
    """Where to send a discovery poll.

    255.255.255.255 is the obvious choice and often does not leave a Windows
    box that has more than one interface: it goes out whichever one the routing
    table picks, which may be a VPN or a virtual adapter. The subnet-directed
    broadcast (a.b.c.255) is routed by address instead, so it reaches the LAN
    the node is actually on. Sends to both.

    Assumes a /24, which covers the networks this gets used on; --target takes
    over for anything else.
    """
    targets = ["255.255.255.255"]
    addresses = set()
    try:
        probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        probe.connect(("8.8.8.8", 80))  # sends nothing; just picks the default route
        addresses.add(probe.getsockname()[0])
        probe.close()
    except OSError:
        pass
    try:
        for info in socket.getaddrinfo(socket.gethostname(), None, socket.AF_INET):
            addresses.add(info[4][0])
    except OSError:
        pass
    for addr in sorted(addresses):
        if addr.startswith("127."):
            continue
        parts = addr.split(".")
        if len(parts) == 4:
            directed = ".".join(parts[:3] + ["255"])
            if directed not in targets:
                targets.append(directed)
    return targets



def discover(target: str, wait: float, show_raw: bool) -> list[Reply]:
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    # Bind to the Art-Net port: the firmware replies to the address the poll
    # came *from* (correct per Art-Net 4, and what makes controllers unicast to
    # it), so the source port has to be one we are listening on.
    try:
        sock.bind(("0.0.0.0", ART_NET_PORT))
    except OSError as e:
        print(f"could not bind UDP {ART_NET_PORT}: {e}", file=sys.stderr)
        print("Something else is holding the Art-Net port - close any lighting "
              "software and try again.", file=sys.stderr)
        raise SystemExit(1)
    sock.settimeout(0.25)

    poll = build_poll()
    targets = [target] if target else broadcast_targets()
    for dest in targets:
        try:
            sock.sendto(poll, (dest, ART_NET_PORT))
        except OSError as e:
            print(f"  (could not send to {dest}: {e})")
    joined = ", ".join(targets)
    print(f"ArtPoll -> {joined} :{ART_NET_PORT}, listening {wait:.0f}s ..." + chr(10))

    replies: list[Reply] = []
    deadline = time.monotonic() + wait
    while time.monotonic() < deadline:
        try:
            data, addr = sock.recvfrom(2048)
        except socket.timeout:
            continue
        if not data.startswith(ART_NET_ID) or len(data) < 10:
            continue
        opcode = struct.unpack_from("<H", data, 8)[0]
        if opcode == OP_POLL:
            continue  # our own broadcast coming back to us
        if opcode != OP_POLL_REPLY:
            print(f"  (ignoring opcode {opcode:#06x} from {addr[0]})")
            continue
        if len(data) < MIN_REPLY_LEN:
            print(f"  !! short ArtPollReply from {addr[0]}: {len(data)} bytes, "
                  f"need {MIN_REPLY_LEN}")
            continue
        reply = Reply(data, addr[0])
        replies.append(reply)
        if show_raw:
            print(f"  raw {len(data)}B from {addr[0]}: {data.hex()}\n")
    sock.close()
    return replies


def report(replies: list[Reply]) -> int:
    if not replies:
        print("No ArtPollReply received." + chr(10))
        print("Work down these, stopping at the first that is wrong:")
        print("  1. CLOSE OTHER ART-NET SOFTWARE and try again. ArtNetominator, QLC+")
        print("     and friends hold UDP 6454 too. This tool sets SO_REUSEADDR so it")
        print("     can still bind, but Windows then delivers each unicast datagram to")
        print("     only ONE listener - and the node replies unicast, to whoever")
        print("     polled. The other app silently eats the answer.")
        print("  2. Can you ping the node? If not, that is the problem, not Art-Net.")
        print("  3. Windows Firewall blocks inbound UDP by default - allow python.exe.")
        print("     A reply from your OWN machine does not clear this: it never")
        print("     crossed the network.")
        print("  4. Is the node in an Art-Net mode? Check `info` on the USB console:")
        print("     mode=ArtNet or ArtNetToDmx. In DMX or sACN mode it does not answer.")
        print("  5. Same subnet? Art-Net broadcast does not cross routers.")
        print("     --target <node ip> unicasts the poll and sidesteps that.")
        return 1

    nodes: dict[str, list[Reply]] = {}
    for r in replies:
        nodes.setdefault(r.mac or r.source, []).append(r)

    problems = 0
    for key, group in nodes.items():
        group.sort(key=lambda r: r.bind_index)
        head = group[0]
        print("=" * 72)
        print(f"{head.long_name or head.short_name}  @ {head.ip}")
        print(f"  MAC {head.mac}   OEM {head.oem:#06x}   firmware {head.version:#06x}")
        print(f"  replies {len(group)}   ports claimed {sum(r.num_ports for r in group)}")
        print()
        print(f"  {'Bind':>4}  {'Ports':>5}  {'Universes (net:sub:uni)':<28} {'SwOut':<14} BindIp")
        for r in group:
            swout = " ".join(f"{b:02x}" for b in r.sw_out[:max(1, min(r.num_ports, 4))])
            print(f"  {r.bind_index:>4}  {r.num_ports:>5}  {r.universe_text():<28} {swout:<14} {r.bind_ip}")
        print()

        if sum(r.num_ports for r in group) == 0:
            print("  (No output ports - this is a controller or monitor, not a node.)")
            print()
            continue

        # The checks that actually matter for auto-patching.
        indices = [r.bind_index for r in group]
        expected = list(range(1, len(group) + 1))
        # A node with a single bind may legitimately say 0 or 1: the spec only
        # requires the index to be meaningful once a node binds more than one.
        single_ok = len(group) == 1 and indices[0] in (0, 1)
        if indices != expected and not single_ok:
            print(f"  !! BindIndex is {indices}, expected {expected}.")
            print("     Controllers key binds on this; gaps or repeats mean universes")
            print("     get patched twice or not at all.")
            problems += 1
        if any(r.num_ports > 4 for r in group):
            print("  !! A reply claims more than 4 ports. One ArtPollReply carries at")
            print("     most 4, so anything above that will be truncated by receivers.")
            problems += 1
        if len({r.bind_ip for r in group}) > 1:
            print("  !! BindIp differs between replies; they should all carry the")
            print("     node's own address so a controller can group them.")
            problems += 1
        all_universes = [u for r in group for u in r.universes]
        if len(all_universes) != len(set(all_universes)):
            dupes = sorted({u for u in all_universes if all_universes.count(u) > 1})
            print(f"  !! Universes repeat across binds: {dupes}")
            problems += 1
        if head.node_report:
            print(f"  NodeReport: {head.node_report}")

        total = sum(r.num_ports for r in group)
        print(f"  => {total} universe(s) bound, {all_universes[0] if all_universes else '?'} "
              f"first. Compare with `Univ Bound` on the node's Main page.")
        print()

    print("=" * 72)
    print(f"{len(nodes)} node(s), {len(replies)} reply/replies.")
    return 1 if problems else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--target", default="",
                    help="where to send the poll. Default: broadcast, both limited "
                         "(255.255.255.255) and subnet-directed. Give the node's IP to "
                         "bypass broadcast entirely.")
    ap.add_argument("--wait", type=float, default=3.0,
                    help="seconds to listen for replies (default: 3)")
    ap.add_argument("--raw", action="store_true", help="hex dump each reply")
    args = ap.parse_args()
    return report(discover(args.target, args.wait, args.raw))


if __name__ == "__main__":
    raise SystemExit(main())
