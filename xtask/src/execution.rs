use super::*;

pub(super) fn command_text(
    root: &std::path::Path,
    program: &str,
    args: &[&str],
) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("run {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{program} {args:?} failed ({})", output.status));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|_| format!("{program} returned non-UTF-8 output"))
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn run_command(command: &mut Command, purpose: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("{purpose}: {error}"))?;
    if !status.success() {
        return Err(format!("{purpose} failed ({status})"));
    }
    Ok(())
}

pub(super) fn preflight_build() -> Result<(), String> {
    for program in ["cargo", "rust-objcopy"] {
        if !Command::new(program)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return Err(format!(
                "{program} unavailable; install required build tools"
            ));
        }
    }
    Ok(())
}

pub(super) fn extract_binary(
    elf_path: &std::path::Path,
    output_path: &std::path::Path,
) -> Result<(), String> {
    run_command(
        Command::new("rust-objcopy")
            .args(["-O", "binary"])
            .arg(elf_path)
            .arg(output_path),
        "extract binary",
    )
}

pub(super) fn elf_binary_size(elf: &[u8]) -> Option<usize> {
    if elf.len() < 52 || &elf[..4] != b"\x7fELF" {
        return None;
    }
    let phoff = u32::from_le_bytes(elf[0x1C..0x20].try_into().ok()?) as usize;
    let phentsize = u16::from_le_bytes(elf[0x2A..0x2C].try_into().ok()?) as usize;
    let phnum = u16::from_le_bytes(elf[0x2C..0x2E].try_into().ok()?) as usize;

    let mut total_size = 0;
    for i in 0..phnum {
        let off = phoff + i * phentsize;
        if off + 32 > elf.len() {
            break;
        }
        let p_type = u32::from_le_bytes(elf[off..off + 4].try_into().ok()?);
        if p_type == 1 {
            let p_filesz = u32::from_le_bytes(elf[off + 16..off + 20].try_into().ok()?) as usize;
            total_size += p_filesz;
        }
    }
    if total_size > 0 {
        Some(total_size)
    } else {
        None
    }
}

pub(super) fn binary_size(path: &std::path::Path) -> usize {
    let bytes = fs::read(path).unwrap_or_else(|_| panic!("read {}", path.display()));
    if bytes.starts_with(b"\x7fELF") {
        elf_binary_size(&bytes).unwrap_or(bytes.len())
    } else {
        bytes.len()
    }
}

pub(super) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

pub(super) fn target_directory(root: &std::path::Path, fallback: &str) -> PathBuf {
    match env::var_os("CARGO_TARGET_DIR") {
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        }
        None => root.join(fallback),
    }
}

/// Sum mapped ELF32 RAM segments, checking each range against nRF52840 RAM.
pub(super) fn elf_ram_size(bytes: &[u8]) -> Result<usize, String> {
    if bytes.len() < 52 || &bytes[..4] != b"\x7fELF" || bytes[4] != 1 || bytes[5] != 1 {
        return Err("expected little-endian ELF32".into());
    }
    let offset = u32::from_le_bytes(bytes[28..32].try_into().unwrap()) as usize;
    let stride = u16::from_le_bytes(bytes[42..44].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(bytes[44..46].try_into().unwrap()) as usize;
    if stride < 32 {
        return Err("invalid ELF program-header size".into());
    }
    let mut total = 0usize;
    for index in 0..count {
        let start = offset
            .checked_add(index.checked_mul(stride).ok_or("ELF offset overflow")?)
            .ok_or("ELF offset overflow")?;
        let header = bytes
            .get(start..start.checked_add(32).ok_or("ELF offset overflow")?)
            .ok_or("truncated ELF program header")?;
        let word = |n| u32::from_le_bytes(header[n..n + 4].try_into().unwrap());
        if word(0) != 1 {
            continue;
        }
        let address = word(8);
        let size = word(20);
        if (0x20000000..0x20040000).contains(&address) {
            if address.checked_add(size).is_none_or(|end| end > 0x20040000) {
                return Err("ELF RAM segment exceeds physical RAM".into());
            }
            total = total
                .checked_add(size as usize)
                .ok_or("RAM size overflow")?;
        }
    }
    Ok(total)
}
