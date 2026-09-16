#!/usr/bin/env python3
"""Send Art-Net or sACN, so the network stages of BRINGUP can be done without a
lighting console. Standard library only.

    # universe -> port mapping. --leds is the real strip length: without it a
    # chase walks all 170 pixels a universe can hold and is mostly invisible on
    # a short strip.
    python tools/dmxsend.py --target 192.168.86.69 --universe 0:0:1 --count 2 --leds 64

    # other-Net traffic: the node on Net 0 must ignore this and log once
    python tools/dmxsend.py --target 192.168.86.69 --universe 1:0:1 --leds 64

    # sACN, multicast, universes 1 and 2
    python tools/dmxsend.py --protocol sacn --universe 1 --count 2 --leds 64

    # throughput: 32 universes at 44 Hz for 30 s, then the achieved rate
    python tools/dmxsend.py --target 192.168.86.69 --universe 0:0:1 \\
        --count 32 --rate 44 --seconds 30

Ctrl-C stops early and still prints the statistics.

Timing. Windows sleeps in ~15.6 ms steps unless a process asks for 1 ms, so a
naive 44 Hz loop alternates 15.6 and 31.2 ms gaps that *average* 22.7 - two
bursts land close together, then a long pause. That halved the apparent
per-burst survival on the 2026-09-16 bring-up and looked like a receiver
problem. This tool now requests the 1 ms timer, sleeps to ~1.5 ms before each
deadline and spins the rest, and reports the measured spacing of its own
frames at the end. Read that block before blaming the node.
"""

from __future__ import annotations

import argparse
import os
import random
import socket
import struct
import sys
import time

ART_NET_ID = b"Art-Net\x00"
ART_NET_PORT = 6454
OP_DMX = 0x5000
PROTOCOL_VERSION = 14
SACN_PORT = 5568
UNIVERSE_SIZE = 512


# --------------------------------------------------------------- addressing
def parse_universe(text: str) -> int:
    """`net:sub:uni` or a plain 15-bit Port-Address."""
    if ":" in text:
        parts = text.split(":")
        if len(parts) != 3:
            raise argparse.ArgumentTypeError("use net:sub:uni, or a plain number")
        net, sub, uni = (int(p, 0) for p in parts)
        for name, value, top in (("net", net, 127), ("sub", sub, 15), ("uni", uni, 15)):
            if not 0 <= value <= top:
                raise argparse.ArgumentTypeError(f"{name} must be 0..{top}")
        return (net << 8) | (sub << 4) | uni
    value = int(text, 0)
    if not 0 <= value <= 0x7FFF:
        raise argparse.ArgumentTypeError("port address must be 0..32767")
    return value


def universe_text(addr: int) -> str:
    return f"{addr >> 8}:{(addr >> 4) & 0x0F}:{addr & 0x0F}"


# ------------------------------------------------------------------ patterns
def make_frame(pattern: str, colour: tuple[int, int, int], step: int, index: int,
               leds: int, channels: int = UNIVERSE_SIZE) -> bytes:
    """One DMX frame of `channels` slots.

    `index` is which universe in the span, so a multi-universe run does not send
    identical data to every port. `leds` is how many RGB pixels are actually
    connected: a chase across the full 170 a universe can hold spends most of
    its time past the end of a 64-LED strip, which looks like a dead output.
    """
    if pattern == "off":
        return bytes(channels)
    if pattern == "ramp":
        # Throughput pattern: fill the frame, the strip length is beside the point.
        return bytes((i + step) & 0xFF for i in range(channels))

    data = bytearray(channels)
    lit = min(leds, channels // 3)
    if pattern == "solid":
        data[:lit * 3] = bytes(colour) * lit
    elif pattern == "chase":
        # One lit pixel walking the strip, offset per universe so the ports are
        # told apart at a glance.
        pos = (step + index * max(1, lit // 4)) % max(1, lit)
        data[pos * 3:pos * 3 + 3] = bytes(colour)
    else:
        raise ValueError(pattern)
    return bytes(data)


# ------------------------------------------------------------------ Art-Net
def artnet_packet(addr: int, sequence: int, data: bytes) -> bytes:
    """ArtDmx. Byte 14 is SubUni (sub_net<<4 | universe), byte 15 is Net —
    matching `parse_port_address` in common/src/artnet/tiny_artnet."""
    header = bytearray()
    header += ART_NET_ID
    header += struct.pack("<H", OP_DMX)
    header += bytes([0, PROTOCOL_VERSION])     # ProtVerHi, ProtVerLo
    header += bytes([sequence, 0])             # Sequence, Physical
    header += bytes([addr & 0xFF, (addr >> 8) & 0x7F])   # SubUni, Net
    header += struct.pack(">H", len(data))     # Length, big endian
    return bytes(header) + data


# --------------------------------------------------------------------- sACN
def sacn_packet(universe: int, sequence: int, data: bytes, cid: bytes,
                source_name: str, priority: int) -> bytes:
    """E1.31 data packet: root + framing + DMP layers, 638 bytes for 512 slots."""
    values = bytes([0x00]) + data              # DMX start code, then the slots
    total = 126 + len(values) - 1              # 638 for a full universe

    pkt = bytearray()
    pkt += struct.pack(">HH", 0x0010, 0x0000)                       # preamble/postamble
    pkt += b"ASC-E1.17\x00\x00\x00"                                 # ACN packet identifier
    pkt += struct.pack(">H", 0x7000 | (total - 16))                 # root flags & length
    pkt += struct.pack(">I", 0x00000004)                            # VECTOR_ROOT_E131_DATA
    pkt += cid                                                      # 16-byte sender CID

    pkt += struct.pack(">H", 0x7000 | (total - 38))                 # framing flags & length
    pkt += struct.pack(">I", 0x00000002)                            # VECTOR_E131_DATA_PACKET
    pkt += source_name.encode("utf-8")[:63].ljust(64, b"\x00")
    pkt += bytes([priority])
    pkt += struct.pack(">H", 0)                                     # synchronization address
    pkt += bytes([sequence, 0])                                     # sequence, options
    pkt += struct.pack(">H", universe)

    pkt += struct.pack(">H", 0x7000 | (total - 115))                # DMP flags & length
    pkt += bytes([0x02, 0xA1])                                      # SET_PROPERTY, addr/data type
    pkt += struct.pack(">HHH", 0x0000, 0x0001, len(values))         # first addr, increment, count
    pkt += values
    assert len(pkt) == total, f"built {len(pkt)} bytes, computed {total}"
    return bytes(pkt)


def sacn_group(universe: int) -> str:
    return f"239.255.{(universe >> 8) & 0xFF}.{universe & 0xFF}"


# ----------------------------------------------------------------- transport
def local_ip() -> str:
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("8.8.8.8", 80))
        ip = s.getsockname()[0]
        s.close()
        return ip
    except OSError:
        return "0.0.0.0"


def build_socket(protocol: str, target: str) -> socket.socket:
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    if protocol == "sacn":
        # Pin multicast to the LAN interface. Windows otherwise picks by routing
        # table, which on a box with a VPN or virtual adapter sends it nowhere
        # useful - the same trap that made the ArtPoll broadcast fail.
        ip = local_ip()
        try:
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton(ip))
        except OSError as e:
            print(f"  (could not pin multicast to {ip}: {e})")
        sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 8)
    elif target.endswith(".255") or target == "255.255.255.255":
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    return sock


def _windows_timer(on: bool) -> None:
    """Ask Windows for 1 ms scheduler granularity (a no-op elsewhere)."""
    if os.name != "nt":
        return
    try:
        import ctypes
        fn = ctypes.windll.winmm.timeBeginPeriod if on else ctypes.windll.winmm.timeEndPeriod
        fn(1)
    except Exception:
        pass


def _wait_until(t: float) -> None:
    """Sleep until ~1.5 ms before `t`, then spin: accurate to well under 100 us
    without burning a core for the whole period."""
    while True:
        remaining = t - time.perf_counter()
        if remaining <= 0:
            return
        if remaining > 0.0015:
            time.sleep(remaining - 0.0015)
        # else spin


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--protocol", choices=("artnet", "sacn"), default="artnet")
    ap.add_argument("--target", default="",
                    help="Art-Net destination. Default: subnet broadcast. Unicast is "
                         "what a real controller does once it has seen an ArtPollReply, "
                         "and it is what the throughput test should use. Ignored for sACN, "
                         "which is multicast by definition.")
    ap.add_argument("--universe", type=parse_universe, default=parse_universe("0:0:1"),
                    help="first universe: net:sub:uni for Art-Net, or a plain number")
    ap.add_argument("--count", type=int, default=1, help="how many consecutive universes")
    ap.add_argument("--pattern", choices=("chase", "solid", "ramp", "off"), default="chase")
    ap.add_argument("--color", default="255,255,255", help="R,G,B for solid and chase")
    ap.add_argument("--channels", type=int, default=UNIVERSE_SIZE,
                    help="DMX slots per packet (default 512). Art-Net allows any even "
                         "count from 2 to 512. Shrinking this keeps the packet *rate* the "
                         "same while cutting the bytes, which is how you tell a per-packet "
                         "cost from a per-byte one.")
    ap.add_argument("--leds", type=int, default=UNIVERSE_SIZE // 3,
                    help="RGB pixels actually connected per universe (default 170, the "
                         "most a universe holds). Set this to the real strip length or a "
                         "chase spends most of its time past the end of it.")
    ap.add_argument("--rate", type=float, default=44.0, help="frames per second per universe")
    ap.add_argument("--seconds", type=float, default=0.0, help="0 = until Ctrl-C")
    ap.add_argument("--priority", type=int, default=100, help="sACN priority")
    args = ap.parse_args()

    try:
        colour = tuple(int(c) for c in args.color.split(","))
        if len(colour) != 3 or not all(0 <= c <= 255 for c in colour):
            raise ValueError
    except ValueError:
        print("--color wants R,G,B with each 0..255", file=sys.stderr)
        return 2
    if args.count < 1:
        print("--count must be at least 1", file=sys.stderr)
        return 2
    if not 1 <= args.leds <= UNIVERSE_SIZE // 3:
        print(f"--leds must be 1..{UNIVERSE_SIZE // 3}", file=sys.stderr)
        return 2
    if not 2 <= args.channels <= UNIVERSE_SIZE or args.channels % 2:
        print("--channels must be even and 2..512 (Art-Net requires an even Length)",
              file=sys.stderr)
        return 2

    target = args.target or ".".join(local_ip().split(".")[:3] + ["255"])
    sock = build_socket(args.protocol, target)
    cid = bytes(random.getrandbits(8) for _ in range(16))
    sequence = [1] * args.count

    first, last = args.universe, args.universe + args.count - 1
    if args.protocol == "artnet":
        where = f"{target}:{ART_NET_PORT}"
        span = f"{universe_text(first)} .. {universe_text(last)}"
    else:
        where = f"{sacn_group(first)} .. {sacn_group(last)}:{SACN_PORT}"
        span = f"{first} .. {last}"
    print(f"{args.protocol} -> {where}")
    print(f"  universes {span}  ({args.count})")
    leds_note = "" if args.pattern in ("ramp", "off") else f"  {args.leds} LED/universe"
    if args.channels != UNIVERSE_SIZE:
        leds_note += f"  {args.channels} slots/packet"
    print(f"  pattern {args.pattern}  colour {colour}  rate {args.rate:g} Hz/universe{leds_note}")
    print(f"  {'until Ctrl-C' if args.seconds <= 0 else f'{args.seconds:g} s'}")
    print()

    period = 1.0 / args.rate
    _windows_timer(True)
    started = time.perf_counter()
    deadline = started + args.seconds if args.seconds > 0 else float("inf")
    next_frame = started
    frames = 0
    packets = 0
    payload = 0
    step = 0
    late = 0
    sent_at: list[float] = []

    try:
        while time.perf_counter() < deadline:
            _wait_until(next_frame)
            now = time.perf_counter()
            if now > next_frame + 0.002:
                late += 1          # more than 2 ms behind schedule
            sent_at.append(now)
            for i in range(args.count):
                data = make_frame(args.pattern, colour, step, i, args.leds, args.channels)
                if args.protocol == "artnet":
                    pkt = artnet_packet(first + i, sequence[i], data)
                    dest = (target, ART_NET_PORT)
                else:
                    # E1.31 carries the slot count in its property-value field, so a
                    # short frame is legal there too.
                    pkt = sacn_packet(first + i, sequence[i], data, cid,
                                      "DMX bring-up", args.priority)
                    dest = (sacn_group(first + i), SACN_PORT)
                try:
                    sock.sendto(pkt, dest)
                except OSError as e:
                    print(f"send failed: {e}", file=sys.stderr)
                    return 1
                packets += 1
                payload += len(pkt)
                sequence[i] = sequence[i] % 255 + 1
            frames += 1
            step += 1
            next_frame += period
    except KeyboardInterrupt:
        print("(stopped)")
    finally:
        sock.close()
        _windows_timer(False)

    elapsed = time.perf_counter() - started
    if elapsed > 0:
        print()
        print(f"  {frames} frame(s) of {args.count} universe(s) in {elapsed:.1f} s")
        print(f"  {frames / elapsed:.1f} Hz achieved (asked {args.rate:g})")
        print(f"  {packets} packets, {payload / 1e6:.1f} MB, "
              f"{payload * 8 / elapsed / 1e6:.2f} Mbit/s on the wire")
        if len(sent_at) >= 3:
            gaps = sorted(b - a for a, b in zip(sent_at, sent_at[1:]))
            def q(p: float) -> float:
                return gaps[min(len(gaps) - 1, int(p * len(gaps)))] * 1e3
            within = sum(1 for g in gaps if abs(g - period) < 0.001)
            print(f"  frame spacing: median {q(0.5):.1f} ms  p10 {q(0.1):.1f}  p90 {q(0.9):.1f}  "
                  f"min {gaps[0]*1e3:.1f}  max {gaps[-1]*1e3:.1f}   (asked {period*1e3:.1f})")
            print(f"  {within}/{len(gaps)} gaps within 1 ms of schedule")
            if gaps[0] < period * 0.6 or gaps[-1] > period * 1.4:
                print("  !! irregular spacing: bursts are landing bunched, which overflows the")
                print("     receiver's buffer regardless of its size. Fix the sender first.")
        if late:
            print(f"  !! {late} frame(s) more than 2 ms behind schedule - this PC could not")
            print("     keep the rate, so a shortfall above is the sender, not the node.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
