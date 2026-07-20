import serial
import time
import os
import shutil

PORT = "/dev/cu.usbmodem123456803"
UF2_PATH = "dist/pager.uf2"
VOLUME = "/Volumes/NICENANO"

def flash():
    print(f"[*] Connecting to serial port {PORT}...")
    try:
        s = serial.Serial(PORT, 115200, timeout=3)
        s.write(b"bootloader\n")
        s.close()
        print("[+] Sent bootloader command.")
    except Exception as e:
        print(f"[-] Serial port error: {e}")
        # Continue anyway in case it is already in bootloader mode
        pass

    print("[*] Waiting for NICENANO volume to mount...")
    for _ in range(15):
        time.sleep(1)
        if os.path.exists(VOLUME):
            print("[+] Volume found! Copying UF2...")
            try:
                # Use standard cp -X command or shutil copy
                shutil.copy(UF2_PATH, VOLUME)
                print("[+] Copy completed.")
            except OSError as e:
                # Expect disk disconnect error (Errno 5)
                print(f"[~] Copy finished with expected disconnect event: {e}")
            break
    else:
        print("[-] Timeout: NICENANO volume not found.")
        return False

    print("[*] Waiting for board to reboot...")
    time.sleep(5.0)
    print("[+] Flash process finished!")
    return True

if __name__ == "__main__":
    flash()
