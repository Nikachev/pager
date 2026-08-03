#!/usr/bin/env python3
"""Print the inactive A/B slot reported by a USB-connected Pager."""

import json
import sys
import urllib.request


def main() -> int:
    url = sys.argv[1] if len(sys.argv) > 1 else "http://192.168.42.1/health"
    try:
        with urllib.request.urlopen(url, timeout=5) as response:
            state = json.load(response)
        slot = state["ota_target_slot"]
        if slot not in (0, 1):
            raise ValueError(f"invalid ota_target_slot: {slot!r}")
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"cannot determine inactive slot from {url}: {error}", file=sys.stderr)
        return 2
    print("A" if slot == 0 else "B")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
