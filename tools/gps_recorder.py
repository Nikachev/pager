#!/usr/bin/env python3
"""Incremental GPS evidence recorder; arm before a user-controlled power cycle."""

import argparse
import json
import os
from pathlib import Path
import sys
import time
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
import usb.core
from pager_tools.hardware import hardware_lock
from pager_tools.protocol import ProtocolError
from pager_tools.protocol_spec import COMMANDS
from pager_tools.usb import PagerUsbClient, find_application, parse_info
from tools.flash_uf2 import get_backend


def sample(client):
    def call(name, suffix=b""):
        return parse_info(client.call(bytes([COMMANDS[name]]) + suffix, timeout_ms=1500).decode())

    state = call("get_gps")
    startup = call("get_gps_startup")
    sentences = []
    for index in range(24):
        entry = call("get_gps_sentence", bytes([index]))
        if entry:
            sentences.append(entry)
    return dict(state=state, startup=startup, sentences=sentences)


def record(args, discover=None, clock=time.monotonic, sleep=time.sleep, client_type=PagerUsbClient):
    backend = get_backend() if discover is None else None
    discover = discover or (lambda: find_application(backend, args.serial))
    started = clock()
    removed = not args.cycle
    connected_at = None
    client = None
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as output:

        def emit(event, **data):
            row = dict(
                event=event,
                elapsed_s=clock() - started,
                host_utc=datetime.now(timezone.utc).isoformat(),
                **data,
            )
            output.write(json.dumps(row) + "\n")
            output.flush()
            os.fsync(output.fileno())
            print(json.dumps(row), flush=True)

        emit(
            "armed",
            serial=args.serial,
            cycle=args.cycle,
            duration_s=args.duration,
            timing="USB absence/presence brackets only; physical power controlled by user",
        )
        try:
            while True:
                now = clock()
                if connected_at is None and now - started >= args.wait:
                    emit("wait_timeout")
                    return 2
                if connected_at is not None and now - connected_at >= args.duration:
                    emit("complete")
                    return 0
                try:
                    if client is None:
                        device = discover()
                        if device is None:
                            if not removed:
                                removed = True
                                emit("usb_absent")
                            sleep(0.25)
                            continue
                        if not removed:
                            sleep(0.25)
                            continue
                        client = client_type(device)
                        client.__enter__()
                        info = parse_info(client.get_info())
                        if info.get("board") != "xiao-nrf52840":
                            raise ValueError("selected board is not XIAO")
                        if connected_at is None:
                            connected_at = clock()
                        emit("connected", info=info)
                    emit("sample", **sample(client))
                    sleep(args.interval)
                except (usb.core.USBError, OSError, ProtocolError) as error:
                    emit("transport_error", error=str(error))
                    if client:
                        try:
                            client.close()
                        except usb.core.USBError, OSError:
                            pass
                    client = None
                    sleep(0.25)
        except KeyboardInterrupt:
            emit("interrupted")
            return 130
        finally:
            if client:
                client.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cycle", action="store_true", help="require USB absence before recording")
    parser.add_argument("--wait", type=float, default=600)
    parser.add_argument("--duration", type=float, default=900)
    parser.add_argument("--interval", type=float, default=2)
    args = parser.parse_args()
    if min(args.wait, args.duration, args.interval) <= 0:
        parser.error("all timing limits must be positive")
    with hardware_lock(args.serial):
        return record(args)


if __name__ == "__main__":
    raise SystemExit(main())
