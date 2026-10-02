use super::*;

pub(super) fn print_memory_info() -> Result<(), String> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let signed_bin_path =
        artifact_dir(&repo_root, &selected_board()?, false, "app").join("pager-signed.bin");
    let layout = Layout::load(&repo_root)?;

    println!("==================================================");
    println!("     Pager Memory Partitioning & Utilization     ");
    println!("==================================================");

    let app_limit = layout.application_partition_size();
    let boot_limit = layout.bootloader_size;

    if signed_bin_path.exists() {
        let size = fs::metadata(&signed_bin_path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);
        let pct = (size as f64 / app_limit as f64) * 100.0;
        let bar_filled = ((pct / 5.0) as usize).min(20);
        let bar = "█".repeat(bar_filled) + &"░".repeat(20 - bar_filled);
        println!("📦 Application Partition (0x0C000..0xFE000):");
        println!("   Size: {} / {} bytes ({:.1}%)", size, app_limit, pct);
        println!("   [{}]", bar);
    } else {
        println!(
            "📦 Application Partition: {} not built yet (run 'make build')",
            signed_bin_path.display()
        );
    }

    let raw_boot_path =
        artifact_dir(&repo_root, &selected_board()?, false, "bootloader").join("bootloader.bin");
    let target_boot_path = target_directory(&repo_root, "bootloader/target")
        .join("thumbv7em-none-eabihf/release/pager-bootloader");
    let boot_path = if raw_boot_path.exists() {
        raw_boot_path
    } else {
        target_boot_path
    };

    if boot_path.exists() {
        let raw_size = fs::read(&boot_path)
            .map(|b| {
                if b.starts_with(b"\x7fELF") {
                    elf_binary_size(&b).unwrap_or(b.len())
                } else {
                    b.len()
                }
            })
            .unwrap_or(0);
        let pct = (raw_size as f64 / boot_limit as f64) * 100.0;
        let bar_filled = ((pct / 5.0) as usize).min(20);
        let bar = "█".repeat(bar_filled) + &"░".repeat(20 - bar_filled);
        println!("\n⚡ Bootloader Partition (0x00000..0x0C000):");
        println!("   Size: {} / {} bytes ({:.1}%)", raw_size, boot_limit, pct);
        println!("   [{}]", bar);
    } else {
        println!("\n⚡ Bootloader Partition: bootloader release binary not built yet (run 'make bootloader')");
    }

    println!(
        "\n💾 Storage Partition: {} bytes (Persistent BLE Bonds & Settings)",
        layout.storage_size
    );
    println!("==================================================");
    Ok(())
}

pub(super) fn check_layout() -> Result<(), String> {
    Layout::load(&repo_root())?;
    println!("layout.json: OK");
    Ok(())
}

pub(super) fn assert_valid_vector(image: &[u8], base: u32, name: &str) {
    pager_bootloader_core::codec::validate_vector(image, base)
        .unwrap_or_else(|error| panic!("{name} invalid vector: {error:?}"));
}

impl Layout {
    pub(super) fn load(root: &std::path::Path) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join("layout.json")).map_err(|error| format!("read layout: {error}"))?,
        )
        .map_err(|error| format!("parse layout: {error}"))?;
        let number = |key: &str| -> Result<u32, String> {
            let value = value[key]
                .as_u64()
                .ok_or_else(|| format!("layout missing numeric field {key}"))?;
            u32::try_from(value).map_err(|_| format!("layout {key} is outside u32 range"))
        };
        let shared = pager_bootloader_core::codec::Layout {
            page_size: number("flash_page_size")?,
            bootloader_size: number("bootloader_size")?,
            firmware_start: number("firmware_start")?,
            manifest_size: number("manifest_size")?,
            image_start: number("firmware_image_start")?,
            storage_start: number("storage_start")?,
            storage_size: number("storage_size")?,
        };
        shared
            .validate()
            .map_err(|error| format!("layout bounds/alignment/install envelope: {error:?}"))?;
        Ok(Self {
            bootloader_size: shared.bootloader_size as usize,
            firmware_start: shared.firmware_start,
            manifest_size: shared.manifest_size as usize,
            storage_start: shared.storage_start,
            storage_size: shared.storage_size as usize,
        })
    }

    pub(super) fn application_partition_size(self) -> usize {
        self.storage_start as usize - self.firmware_start as usize
    }
}
