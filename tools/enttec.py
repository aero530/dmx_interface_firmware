#!/usr/bin/env python3
"""Drive the board's Enttec DMX USB Pro widget directly, with no console in the
loop. Standard library only (Windows COM ports via ctypes).

Built to answer "which DMX channel lights which pixel component", which a
lighting console cannot answer because you never see the bytes it sends.

    python tools/enttec.py --list                      # find the port
    python tools/enttec.py --port COM5 --params        # prove the round trip
    python tools/enttec.py --port COM5 --map 12        # walk channels 1..12
    python tools/enttec.py --port COM5 --channel 1 --value 255
    python tools/enttec.py --port COM5 --pattern chase --leds 64
    python tools/enttec.py --port COM5 --monitor       # watch DMX the board receives

# Which mode does what

**Anything that sends DMX needs the board in `USB>DMX` mode.** The firmware
accepts label 6 only in that mode, on purpose - a PC plugged in while the rig
runs off wired DMX or Art-Net must not be able to clobber the live universe
(`enttec_widget.rs`, `handle_message`). In any other mode those frames are
dropped silently, so `--map`, `--channel` and `--pattern` appear to do nothing.
That is the design, not a fault, and this tool warns rather than leaving you to
guess.

`--params` and `--serial` answer in **every** mode, which makes them the way to
prove the port and the framing before blaming anything else.

`--monitor` is the counterpart for the other direction: outside `USB>DMX` the
widget forwards whatever the board is receiving - wired DMX, Art-Net or sACN,
whichever the mode selects - as label 5 on change. That is the one test that
exercises the widget without switching modes.

In `USB>DMX` the board is an FT232R and Windows' FTDI VCP driver gives it a COM
port. In every other mode it is a composite CDC device and the widget is the
*first* of its three CDC ports (widget, console, log).

# The framing

`0x7E, label, len_lo, len_hi, payload, 0xE7` (`common/src/enttec_protocol.rs`).
Label 6 is Output DMX and its payload is **a start code byte followed by the
channels** - so channel 1 is payload byte 1, not byte 0. `--no-start-code`
sends the channels alone, which is what some hosts do; if that changes which
pixel lights, the host and the firmware disagree about this byte and that is
the bug.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wintypes
import os
import sys
import time

START_DELIMITER = 0x7E
END_DELIMITER = 0xE7
LABEL_GET_PARAMS = 3
# What the board sends unprompted when the DMX it receives changes.
LABEL_RECEIVE_ON_CHANGE_REPLY = 5
LABEL_OUTPUT_DMX = 6
LABEL_GET_SERIAL = 10
UNIVERSE = 512


# ------------------------------------------------------------------ framing
def warn_needs_usb_dmx() -> None:
    """Say it before the test rather than after it fails.

    The firmware drops label 6 outside `USB>DMX` and says nothing, so a wrong
    mode and a broken cable look identical from here.
    """
    print("NOTE: sending DMX needs the board in USB>DMX mode. In any other mode the")
    print("      firmware drops these frames on purpose and nothing will light.")
    print("      `--params` still answers in every mode; use it to prove the port.\n")


def frame(label: int, payload: bytes) -> bytes:
    n = len(payload)
    return bytes([START_DELIMITER, label, n & 0xFF, (n >> 8) & 0xFF]) + payload + bytes([END_DELIMITER])


def unframe(buf: bytes) -> list[tuple[int, bytes]]:
    """Pull every complete message out of `buf`. Tolerant of noise between them."""
    out, i = [], 0
    while True:
        try:
            i = buf.index(START_DELIMITER, i)
        except ValueError:
            return out
        if i + 4 > len(buf):
            return out
        label = buf[i + 1]
        n = buf[i + 2] | (buf[i + 3] << 8)
        end = i + 4 + n
        if end >= len(buf):
            return out
        if buf[end] == END_DELIMITER:
            out.append((label, buf[i + 4:end]))
            i = end + 1
        else:
            i += 1


# ------------------------------------------------------- Windows serial port
class DCB(ctypes.Structure):
    _fields_ = [
        ("DCBlength", wintypes.DWORD), ("BaudRate", wintypes.DWORD),
        ("fBits", wintypes.DWORD),
        ("wReserved", wintypes.WORD), ("XonLim", wintypes.WORD), ("XoffLim", wintypes.WORD),
        ("ByteSize", ctypes.c_byte), ("Parity", ctypes.c_byte), ("StopBits", ctypes.c_byte),
        ("XonChar", ctypes.c_char), ("XoffChar", ctypes.c_char), ("ErrorChar", ctypes.c_char),
        ("EofChar", ctypes.c_char), ("EvtChar", ctypes.c_char), ("wReserved1", wintypes.WORD),
    ]


class COMMTIMEOUTS(ctypes.Structure):
    _fields_ = [
        ("ReadIntervalTimeout", wintypes.DWORD),
        ("ReadTotalTimeoutMultiplier", wintypes.DWORD),
        ("ReadTotalTimeoutConstant", wintypes.DWORD),
        ("WriteTotalTimeoutMultiplier", wintypes.DWORD),
        ("WriteTotalTimeoutConstant", wintypes.DWORD),
    ]


class Serial:
    """Just enough of a serial port for this: open, write, read-with-timeout.

    ctypes rather than pyserial so the bench tools stay dependency-free, which
    is the same rule `dmxsend.py` and `artpoll.py` follow.
    """

    def __init__(self, port: str, baud: int = 250000, read_timeout_ms: int = 300):
        if os.name != "nt":
            # POSIX: a plain file handle is enough for an FTDI VCP, since the
            # baud rate is meaningless over USB anyway.
            self.fd = os.open(port, os.O_RDWR | os.O_NOCTTY)
            self.handle = None
            return
        self.fd = None
        k32 = ctypes.WinDLL("kernel32", use_last_error=True)
        self.k32 = k32
        handle = k32.CreateFileW(
            f"\\\\.\\{port}", 0x80000000 | 0x40000000, 0, None, 3, 0, None
        )
        if handle == -1 or handle == ctypes.c_void_p(-1).value:
            err = ctypes.get_last_error()
            raise OSError(f"cannot open {port}: Windows error {err}"
                          + (" (in use - close QLC+ and any other app holding it)" if err == 5 else ""))
        self.handle = wintypes.HANDLE(handle)

        dcb = DCB()
        dcb.DCBlength = ctypes.sizeof(DCB)
        if not k32.GetCommState(self.handle, ctypes.byref(dcb)):
            raise OSError(f"GetCommState failed: {ctypes.get_last_error()}")
        dcb.BaudRate = baud
        dcb.ByteSize = 8
        dcb.Parity = 0        # none
        dcb.StopBits = 2      # 2 stop bits, as DMX uses
        dcb.fBits = 0x0001    # fBinary, no flow control
        if not k32.SetCommState(self.handle, ctypes.byref(dcb)):
            raise OSError(f"SetCommState failed: {ctypes.get_last_error()}")

        timeouts = COMMTIMEOUTS(0, 0, read_timeout_ms, 0, 1000)
        k32.SetCommTimeouts(self.handle, ctypes.byref(timeouts))

    def write(self, data: bytes) -> None:
        if self.handle is None:
            os.write(self.fd, data)
            return
        written = wintypes.DWORD(0)
        if not self.k32.WriteFile(self.handle, data, len(data), ctypes.byref(written), None):
            raise OSError(f"WriteFile failed: {ctypes.get_last_error()}")

    def read(self, count: int = 1024) -> bytes:
        if self.handle is None:
            return os.read(self.fd, count)
        buf = ctypes.create_string_buffer(count)
        got = wintypes.DWORD(0)
        if not self.k32.ReadFile(self.handle, buf, count, ctypes.byref(got), None):
            raise OSError(f"ReadFile failed: {ctypes.get_last_error()}")
        return buf.raw[:got.value]

    def close(self) -> None:
        if self.handle is None:
            os.close(self.fd)
        else:
            self.k32.CloseHandle(self.handle)


def list_ports() -> list[tuple[str, str]]:
    """Every COM port Windows knows about, with the device that owns it."""
    if os.name != "nt":
        import glob
        return [(p, "") for p in sorted(glob.glob("/dev/ttyUSB*") + glob.glob("/dev/ttyACM*"))]
    import winreg
    found = []
    try:
        key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"HARDWARE\DEVICEMAP\SERIALCOMM")
    except OSError:
        return found
    i = 0
    while True:
        try:
            device, port, _ = winreg.EnumValue(key, i)
        except OSError:
            break
        found.append((port, device))
        i += 1
    return found


# ------------------------------------------------------------------ payloads
def dmx_payload(channels: dict[int, int], start_code: bool) -> bytes:
    """A full universe with `channels` (1-based) set, everything else dark."""
    data = bytearray(UNIVERSE)
    for ch, value in channels.items():
        if 1 <= ch <= UNIVERSE:
            data[ch - 1] = value & 0xFF
    return (bytes([0x00]) if start_code else b"") + bytes(data)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--list", action="store_true", help="list serial ports and exit")
    ap.add_argument("--port", help="e.g. COM5, or /dev/ttyUSB0")
    ap.add_argument("--params", action="store_true", help="GET_PARAMS and print the reply")
    ap.add_argument("--serial", action="store_true", help="GET_SERIAL and print the reply")
    ap.add_argument("--map", type=int, metavar="N",
                    help="walk channels 1..N one at a time, full brightness, pausing between "
                         "each so you can note which pixel and colour lights. The mapping test.")
    ap.add_argument("--dwell", type=float, default=1.5, help="seconds per channel in --map")
    ap.add_argument("--channel", type=int, help="set one channel")
    ap.add_argument("--value", type=int, default=255, help="value for --channel (default 255)")
    ap.add_argument("--pattern", choices=("chase", "solid", "off"), help="animate instead")
    ap.add_argument("--leds", type=int, default=64, help="RGB pixels connected (default 64)")
    ap.add_argument("--color", default="255,255,255", help="R,G,B for the patterns")
    ap.add_argument("--rate", type=float, default=30.0, help="frames/s for --pattern")
    ap.add_argument("--seconds", type=float, default=0.0, help="0 = until Ctrl-C")
    ap.add_argument("--monitor", action="store_true",
                    help="decode the label-5 frames the board sends when the DMX it "
                         "receives changes. Works in every mode except USB>DMX, where the "
                         "host is the source and there is nothing to forward.")
    ap.add_argument("--channels", type=int, default=16, metavar="N",
                    help="channels to show per --monitor update (default 16)")
    ap.add_argument("--no-start-code", action="store_true",
                    help="send 512 channel bytes with no leading start code. Some hosts do "
                         "this; if it changes which pixel lights, that byte is the disagreement.")
    args = ap.parse_args()

    if args.list:
        ports = list_ports()
        if not ports:
            print("no serial ports found")
            return 1
        print(f"{'port':8s} device")
        for port, device in ports:
            print(f"{port:8s} {device}")
        print("\nThe board in USB>DMX mode appears as an FTDI device (VID_0403+PID_6001).")
        return 0

    if not args.port:
        print("need --port (try --list)", file=sys.stderr)
        return 2

    start_code = not args.no_start_code
    ser = Serial(args.port)
    print(f"{args.port} open, start code {'on' if start_code else 'OFF'}")

    try:
        if args.params or args.serial:
            label = LABEL_GET_PARAMS if args.params else LABEL_GET_SERIAL
            body = bytes([0, 0]) if args.params else b""
            ser.write(frame(label, body))
            time.sleep(0.2)
            reply = ser.read(1024)
            if not reply:
                print("  no reply. The widget answers GET_PARAMS in every mode, so silence here")
                print("  means the port is not the widget, or nothing is reading it.")
                return 1
            print(f"  {len(reply)} bytes back: {reply[:32].hex()}")
            for lbl, payload in unframe(reply):
                print(f"  label {lbl}: {payload.hex()}")
                if lbl == LABEL_GET_PARAMS and len(payload) >= 5:
                    print(f"    firmware {payload[1]}.{payload[0]}, break {payload[2]}, "
                          f"MAB {payload[3]}, refresh {payload[4]} Hz")
            return 0

        if args.monitor:
            print("watching for label-5 frames (Ctrl-C to stop).")
            print("Silence means the received universe is not changing, or the board is in")
            print("USB>DMX mode where the host is the source and nothing is forwarded.\n")
            buf = b""
            frames = 0
            try:
                while True:
                    buf += ser.read(4096)
                    msgs = unframe(buf)
                    if msgs:
                        # Keep only what follows the last complete message.
                        buf = b""
                    for lbl, payload in msgs:
                        if lbl != LABEL_RECEIVE_ON_CHANGE_REPLY:
                            print(f"  label {lbl}: {payload[:16].hex()}")
                            continue
                        frames += 1
                        # Payload is a start code then the channels, same as
                        # label 6 - so channel 1 is payload[1].
                        chans = payload[1:1 + args.channels]
                        shown = " ".join(f"{b:3d}" for b in chans)
                        print(f"  [{frames:5d}] start {payload[0]:#04x} | {shown}")
                    time.sleep(0.05)
            except KeyboardInterrupt:
                print(f"\n(stopped after {frames} frames)")
            return 0

        if args.map:
            warn_needs_usb_dmx()
            print(f"walking channels 1..{args.map}, {args.dwell:g}s each, value 255.")
            print("Note which pixel and which colour lights for each.\n")
            for ch in range(1, args.map + 1):
                ser.write(frame(LABEL_OUTPUT_DMX, dmx_payload({ch: 255}, start_code)))
                print(f"  channel {ch:3d} -> ", end="", flush=True)
                time.sleep(args.dwell)
                print("(next)")
            ser.write(frame(LABEL_OUTPUT_DMX, dmx_payload({}, start_code)))
            print("\ndone, universe blacked out.")
            print("Expected: channel 1 = pixel 1 red, 2 = pixel 1 green, 3 = pixel 1 blue,")
            print("          4 = pixel 2 red, and so on in threes.")
            return 0

        if args.channel:
            warn_needs_usb_dmx()
            ser.write(frame(LABEL_OUTPUT_DMX, dmx_payload({args.channel: args.value}, start_code)))
            print(f"  channel {args.channel} = {args.value}, all others 0")
            return 0

        if args.pattern:
            warn_needs_usb_dmx()
            colour = tuple(int(c) for c in args.color.split(","))
            period = 1.0 / args.rate
            started = time.perf_counter()
            deadline = started + args.seconds if args.seconds > 0 else float("inf")
            step = 0
            print(f"  {args.pattern} on {args.leds} pixels at {args.rate:g} Hz, Ctrl-C to stop")
            try:
                while time.perf_counter() < deadline:
                    channels: dict[int, int] = {}
                    if args.pattern == "solid":
                        for p in range(args.leds):
                            for c in range(3):
                                channels[p * 3 + c + 1] = colour[c]
                    elif args.pattern == "chase":
                        p = step % args.leds
                        for c in range(3):
                            channels[p * 3 + c + 1] = colour[c]
                    ser.write(frame(LABEL_OUTPUT_DMX, dmx_payload(channels, start_code)))
                    step += 1
                    time.sleep(period)
            except KeyboardInterrupt:
                print("\n  (stopped)")
            ser.write(frame(LABEL_OUTPUT_DMX, dmx_payload({}, start_code)))
            return 0

        print("nothing to do: pick --params, --map, --channel or --pattern", file=sys.stderr)
        return 2
    finally:
        ser.close()


if __name__ == "__main__":
    raise SystemExit(main())
