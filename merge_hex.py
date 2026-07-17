import os
import sys

def merge():
    sd_path = "s140_nrf52_7.3.0_softdevice.hex"
    app_path = "target/thumbv7em-none-eabihf/release/pager.hex"
    output_path = "combined.hex"
    
    if not os.path.exists(sd_path):
        print(f"Error: {sd_path} not found. Please make sure the SoftDevice HEX is present in the repository root.")
        sys.exit(1)
    if not os.path.exists(app_path):
        print(f"Error: {app_path} not found. Run cargo build first.")
        sys.exit(1)

    print("Reading SoftDevice HEX...")
    with open(sd_path, "r") as f:
        sd_lines = f.readlines()

    # Filter out the Intel HEX EOF record ":00000001FF"
    sd_clean = [line for line in sd_lines if line.strip() != ":00000001FF"]
    print(f"Loaded SoftDevice: {len(sd_lines)} lines.")

    print("Reading Application HEX...")
    with open(app_path, "r") as f:
        app_lines = f.readlines()
    print(f"Loaded Application: {len(app_lines)} lines.")

    print(f"Merging into {output_path}...")
    with open(output_path, "w") as f:
        f.writelines(sd_clean)
        f.writelines(app_lines)
    print("Merge successfully completed!")

if __name__ == "__main__":
    merge()
