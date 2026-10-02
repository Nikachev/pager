use super::*;

pub(super) fn artifact_dir(
    root: &std::path::Path,
    board: &str,
    release: bool,
    kind: &str,
) -> PathBuf {
    root.join("dist")
        .join(board)
        .join(if release { "release" } else { "dev" })
        .join(kind)
}

pub(super) fn staging_dir(
    root: &std::path::Path,
    board: &str,
    release: bool,
    kind: &str,
) -> PathBuf {
    let base = artifact_dir(root, board, release, kind)
        .parent()
        .unwrap()
        .to_path_buf();
    let stage = base.join(format!(".stage-{kind}-{}", std::process::id()));
    if stage.exists() {
        fs::remove_dir_all(&stage).expect("remove previous staging dir");
    }
    fs::create_dir_all(&stage).expect("create staging dir");
    stage
}

pub(super) fn publish_package(stage: &std::path::Path, kind: &str) -> PathBuf {
    let base = stage.parent().unwrap();
    let builds = base.join("builds");
    fs::create_dir_all(&builds).expect("create immutable package directory");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("{kind}-{nonce}");
    let immutable = builds.join(&name);
    fs::rename(stage, &immutable).expect("publish immutable package");
    let temporary_link = base.join(format!(".link-{kind}-{}", std::process::id()));
    std::os::unix::fs::symlink(PathBuf::from("builds").join(name), &temporary_link)
        .expect("create package link");
    let destination = base.join(kind);
    fs::rename(&temporary_link, &destination).expect("atomically replace package link");
    destination
}

pub(super) fn write_metadata(
    stage: &std::path::Path,
    mut metadata: serde_json::Value,
) -> Result<(), String> {
    let mut hashes = serde_json::Map::new();
    for entry in fs::read_dir(stage).expect("read package files") {
        let path = entry.unwrap().path();
        if path.is_file() {
            hashes.insert(
                path.file_name().unwrap().to_str().unwrap().to_owned(),
                serde_json::Value::String(hex(&Sha256::digest(fs::read(&path).unwrap()))),
            );
        }
    }
    metadata["fault_injection"] =
        serde_json::Value::Bool(env::var("PAGER_FAULT_INJECTION").as_deref() == Ok("1"));
    metadata["usb_trace"] = serde_json::Value::Bool(
        metadata["kind"] == "application"
            && metadata["updater"].is_null()
            && env::var("PAGER_USB_TRACE").as_deref() == Ok("1"),
    );
    metadata["artifacts"] = hashes.into();
    let root = repo_root();
    metadata["commit"] = command_text(&root, "git", &["rev-parse", "HEAD"])?.into();
    metadata["dirty"] =
        (!command_text(&root, "git", &["status", "--porcelain"])?.is_empty()).into();
    metadata["source_tree_sha256"] = source_fingerprint(&root)?.into();
    metadata["build_utc"] = command_text(&root, "date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])?.into();
    fs::write(
        stage.join("package.json"),
        serde_json::to_vec_pretty(&metadata).unwrap(),
    )
    .map_err(|error| format!("write package metadata: {error}"))?;
    Ok(())
}

pub(super) fn encode_uf2(image: &[u8], base_address: u32) -> Vec<u8> {
    let count = image.len().div_ceil(256) as u32;
    let mut output = Vec::with_capacity(count as usize * 512);
    for (number, payload) in image.chunks(256).enumerate() {
        let address = base_address
            .checked_add(number as u32 * 256)
            .expect("UF2 address overflow");
        output.extend_from_slice(
            &Uf2Block::encode(address, number as u32, count, payload).expect("valid UF2 payload"),
        );
    }
    output
}

pub(super) fn verify_uf2() -> Result<(), String> {
    verify_uf2_path(&artifact_dir(&repo_root(), &selected_board()?, false, "app").join("pager.uf2"))
}

pub(super) fn verify_uf2_path(path: &std::path::Path) -> Result<(), String> {
    let root = repo_root();
    let layout = Layout::load(&root)?;
    let uf2 = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if uf2.is_empty()
        || !uf2.len().is_multiple_of(512)
        || uf2.len() / 512 > pager_bootloader_core::MAX_TRACKED_BLOCKS
    {
        return Err("invalid UF2 length/count".into());
    }
    let count = uf2.len() / 512;
    let mut image = vec![0; count * 256];
    let mut seen = vec![false; count];
    let mut lengths = vec![0; count];
    for bytes in uf2.chunks_exact(512) {
        let block = Uf2Block::parse(bytes.try_into().unwrap())
            .map_err(|error| format!("invalid UF2 header: {error:?}"))?;
        let number = block.block_no as usize;
        let payload = block.payload();
        if block.num_blocks as usize != count
            || number >= count
            || seen[number]
            || block.target_addr != layout.firmware_start + number as u32 * 256
            || block
                .target_addr
                .checked_add(payload.len() as u32)
                .is_none_or(|end| end > layout.storage_start)
            || (number + 1 != count && payload.len() != 256)
        {
            return Err("invalid UF2 numbering/address/payload".into());
        }
        image[number * 256..number * 256 + payload.len()].copy_from_slice(payload);
        lengths[number] = payload.len();
        seen[number] = true;
    }
    let manifest =
        ManifestHeader::parse(&image).map_err(|error| format!("invalid manifest: {error:?}"))?;
    let full_len = layout
        .manifest_size
        .checked_add(manifest.image_len as usize)
        .ok_or("signed length overflow")?;
    if manifest.image_len < 8
        || !manifest.image_len.is_multiple_of(4)
        || full_len > layout.application_partition_size()
        || full_len > image.len()
        || count != full_len.div_ceil(256)
        || lengths[count - 1] != full_len - (count - 1) * 256
        || image[ManifestHeader::SIZE..layout.manifest_size]
            .iter()
            .any(|byte| *byte != 255)
    {
        return Err("invalid signed image length/padding/final payload".into());
    }
    let raw = &image[layout.manifest_size..full_len];
    pager_bootloader_core::codec::validate_vector(
        raw,
        layout.firmware_start + layout.manifest_size as u32,
    )
    .map_err(|_| "invalid application vector")?;
    if <[u8; 32]>::from(Sha256::digest(raw)) != manifest.digest {
        return Err("signed image digest mismatch".into());
    }
    let signature = Signature::from_bytes(&manifest.signature);
    let trusted = [
        root.join("bootloader/firmware_signing_public.hex"),
        root.join("keys/dev_signing_public.hex"),
    ]
    .iter()
    .filter_map(|path| {
        let text = fs::read_to_string(path).ok()?;
        VerifyingKey::from_bytes(&decode_hex_32(&text)?).ok()
    })
    .collect::<Vec<_>>();
    if !trusted.iter().any(|key| {
        key.verify_strict(&manifest.signed_message(), &signature)
            .is_ok()
    }) {
        return Err("UF2 signature is not trusted by the local key set".into());
    }
    println!(
        "{}: signature, digest, layout and {} blocks OK",
        path.display(),
        count
    );
    Ok(())
}

pub(super) fn encode_xiao_installer_uf2(installer: &[u8]) -> Vec<u8> {
    // Factory Adafruit 0.6.x rejects blocks with payloadSize != 256, including
    // the last block. Pad only this installer; Pager's signed image encoding
    // has its own length semantics.
    let mut padded = installer.to_vec();
    padded.resize(installer.len().div_ceil(256) * 256, 0xFF);
    assert!(
        XIAO_FACTORY_APPLICATION_START as usize + padded.len()
            <= XIAO_FACTORY_BOOTLOADER_START as usize,
        "padded XIAO installer overlaps the factory bootloader"
    );
    encode_uf2(&padded, XIAO_FACTORY_APPLICATION_START)
}
