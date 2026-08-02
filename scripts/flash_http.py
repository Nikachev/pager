#!/usr/bin/env python3
import http.client
import socket
import sys
import time
import zlib
from urllib.parse import urlparse

def flash_http(url, bin_path):
    print(f"Reading binary: {bin_path}")
    with open(bin_path, "rb") as f:
        binary_data = f.read()

    parsed = urlparse(url)
    host = parsed.hostname or "192.168.42.1"
    port = parsed.port or 80
    path = parsed.path or "/update"

    print(f"Connecting to {host}:{port}{path} (Size: {len(binary_data)} bytes)...")
    connected = False
    start_time = time.time()
    while time.time() - start_time < 20:
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(10)
            s.connect((host, port))
            s.settimeout(60)
            connected = True
            break
        except Exception:
            time.sleep(0.5)

    if not connected:
        print(f"[-] Could not connect to {host}:{port}. Is the board online?")
        return 1

    headers = (
        f"POST {path} HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        f"Content-Type: application/octet-stream\r\n"
        f"Content-Length: {len(binary_data)}\r\n"
        f"X-Pager-CRC32: {zlib.crc32(binary_data):08x}\r\n"
        f"Connection: close\r\n\r\n"
    ).encode("utf-8")

    s.sendall(headers)

    chunk_size = 4096
    total = len(binary_data)
    sent = 0

    print("[*] Streaming payload chunks...")
    try:
        for i in range(0, total, chunk_size):
            chunk = binary_data[i:i + chunk_size]
            s.sendall(chunk)
            sent += len(chunk)
            print(f"\rTransferred {sent}/{total} bytes ({sent * 100 // total}%)...", end="", flush=True)
            time.sleep(0.002)
    except (socket.error, BrokenPipeError, ConnectionResetError) as e:
        print(f"\n[-] Connection lost during upload at {sent}/{total} bytes: {e}")
        s.close()
        return 1

    print("\nUpload complete! Waiting for board response...")
    resp = s.recv(1024).decode("utf-8", errors="ignore")
    s.close()
    print(f"Server response: {resp.strip()}")
    if "200" in resp:
        print("[+] HTTP OTA update succeeded! Board will self-flash and reboot.")
        return 0
    else:
        print(f"[-] HTTP OTA update failed with response: {resp}")
        return 1

if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <HTTP_URL> <BIN_PATH>")
        sys.exit(1)
    sys.exit(flash_http(sys.argv[1], sys.argv[2]))
