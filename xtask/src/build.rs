use super::*;

pub(super) fn build_firmware(release: bool) -> Result<(), String> {
    build_application(release, None)
}

pub(super) fn build_application(
    release: bool,
    updater: Option<serde_json::Value>,
) -> Result<(), String> {
    let root = repo_root();
    if release {
        if env::var_os("PAGER_USB_TRACE").is_some() {
            return Err("USB progress tracing is forbidden in release builds".into());
        }
        if env::var_os("PAGER_FAULT_INJECTION").is_some() {
            return Err("fault injection is forbidden in release builds".into());
        }
        if env::var_os("PAGER_SKIP_WATCHDOG_FEED").is_some() {
            return Err("watchdog override is forbidden in release builds".into());
        }
        assert_release_tree(&root)?;
    }
    preflight_build()?;
    let board = selected_board()?;
    let layout = Layout::load(&root)?;
    let signing_key = load_signing_key(&root, release)?;
    let version = build_version(&root)?;
    let board_feature = format!("board-{board}");
    let mut command = Command::new("cargo");
    command
        .args([
            "build",
            "--locked",
            "--release",
            "--target",
            "thumbv7em-none-eabihf",
            "--no-default-features",
            "--features",
            &board_feature,
        ])
        .env("PAGER_BUILD_VERSION", &version.display)
        .current_dir(&root);
    if updater.is_some() {
        command.args(["--manifest-path", "bootloader-updater/Cargo.toml"]);
    }
    run_command(&mut command, "firmware compilation")?;
    let kind = if updater.is_some() { "updater" } else { "app" };
    let stage = staging_dir(&root, &board, release, kind);
    let (fallback, binary) = if updater.is_some() {
        ("bootloader-updater/target", "pager-bootloader-updater")
    } else {
        ("target", "pager")
    };
    let elf = target_directory(&root, fallback)
        .join("thumbv7em-none-eabihf/release")
        .join(binary);
    fs::copy(&elf, stage.join("pager.elf")).expect("archive exact app ELF");
    let raw_path = stage.join("pager-raw.bin");
    extract_binary(&elf, &raw_path)?;
    let raw = fs::read(&raw_path).expect("read raw image");
    assert!(
        raw.len() + layout.manifest_size <= layout.application_partition_size(),
        "application exceeds its partition"
    );
    assert_valid_vector(
        &raw,
        layout.firmware_start + layout.manifest_size as u32,
        "Pager application",
    );
    let digest: [u8; 32] = Sha256::digest(&raw).into();
    let mut signed = Vec::new();
    signed.extend_from_slice(&MANIFEST_MAGIC);
    signed.extend_from_slice(&version.numeric.to_le_bytes());
    signed.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    signed.extend_from_slice(&digest);
    let manifest = ManifestHeader {
        magic: MANIFEST_MAGIC,
        version: version.numeric,
        image_len: raw.len() as u32,
        digest,
        signature: signing_key.sign(&signed).to_bytes(),
    };
    let mut image = manifest.to_bytes().to_vec();
    image.resize(layout.manifest_size, 0xFF);
    image.extend_from_slice(&raw);
    let uf2 = encode_uf2(&image, layout.firmware_start);
    fs::write(stage.join("pager-signed.bin"), &image).unwrap();
    fs::write(stage.join("pager.uf2"), &uf2).unwrap();
    fs::write(stage.join("version.txt"), format!("{}\n", version.display)).unwrap();
    verify_uf2_path(&stage.join("pager.uf2"))?;
    if updater.is_some() {
        fs::copy(
            env::var_os("PAGER_BOOTLOADER_BIN").unwrap(),
            stage.join("embedded-bootloader.bin"),
        )
        .unwrap();
    }
    write_metadata(
        &stage,
        serde_json::json!({
            "schema": 1, "kind": "application", "board": board, "updater": updater,
            "mode": if release { "release" } else { "dev" }, "version": version.display,
            "numeric_version": version.numeric, "protocol": 5,
            "layout": serde_json::from_slice::<serde_json::Value>(&fs::read(root.join("layout.json")).unwrap()).unwrap(),
            "image_sha256": hex(&digest), "uf2_sha256": hex(&Sha256::digest(&uf2)),
            "key_fingerprint": hex(&Sha256::digest(signing_key.verifying_key().to_bytes())),
        }),
    )?;
    let output = publish_package(&stage, kind);
    println!(
        "Published {}: {} bytes, version {}",
        output.display(),
        image.len(),
        version.display
    );
    Ok(())
}

pub(super) fn build_bootloader(release: bool) -> Result<(), String> {
    let board = selected_board()?;
    build_bootloader_for(release, &board).map(|_| ())
}

pub(super) fn build_bootloader_for(release: bool, board: &str) -> Result<PathBuf, String> {
    let repo_root = repo_root();
    if release {
        assert_release_tree(&repo_root)?;
    }
    preflight_build()?;
    if !release {
        let _ = ensure_dev_signing_key(&repo_root);
    }
    let mut command = Command::new("cargo");
    let board_feature = match board {
        "nice-nano-v2" => "board-nice-nano-v2",
        "xiao-nrf52840" => "board-xiao-nrf52840",
        _ => panic!("PAGER_BOARD must be nice-nano-v2 or xiao-nrf52840"),
    };
    let kind = if release {
        "release-bootloader"
    } else {
        "dev-bootloader"
    };
    let features = format!("{kind},{board_feature}");
    let version = format!(
        "0.3.0-{}",
        command_text(&repo_root, "date", &["-u", "+%Y%m%d%H%M%S"])?
    );
    command
        .env("PAGER_BOOTLOADER_VERSION", &version)
        .args([
            "build",
            "--locked",
            "--manifest-path",
            "bootloader/Cargo.toml",
            "--release",
            "--no-default-features",
            "--features",
            &features,
        ])
        .current_dir(&repo_root);
    run_command(&mut command, "bootloader compilation")?;
    let stage = staging_dir(&repo_root, board, release, "bootloader");
    let elf = target_directory(&repo_root, "bootloader/target")
        .join("thumbv7em-none-eabihf/release/pager-bootloader");
    fs::copy(&elf, stage.join("bootloader.elf")).unwrap();
    extract_binary(&elf, &stage.join("bootloader.bin"))?;
    let binary = fs::read(stage.join("bootloader.bin")).unwrap();
    assert!(
        binary.len() <= Layout::load(&repo_root)?.bootloader_size,
        "bootloader exceeds partition"
    );
    assert_valid_vector(&binary, 0, "Pager bootloader");
    let mut partition = binary.clone();
    partition.resize(Layout::load(&repo_root)?.bootloader_size, 0xFF);
    let mut trusted = vec![hex(&Sha256::digest(
        decode_hex_32(
            &fs::read_to_string(repo_root.join("bootloader/firmware_signing_public.hex")).unwrap(),
        )
        .unwrap(),
    ))];
    if !release {
        trusted.push(hex(&Sha256::digest(
            ensure_dev_signing_key(&repo_root)
                .verifying_key()
                .to_bytes(),
        )));
    }
    write_metadata(
        &stage,
        serde_json::json!({"schema": 1, "kind": "bootloader", "version": version,
        "partition_sha256": hex(&Sha256::digest(&partition)), "trusted_key_fingerprints": trusted, "board": board,
        "mode": if release { "release" } else { "dev" }, "image_sha256": hex(&Sha256::digest(&binary))}),
    )?;
    Ok(publish_package(&stage, "bootloader"))
}

pub(super) fn build_updater() -> Result<(), String> {
    let board = selected_board()?;
    let serial = env::var("PAGER_USB_SERIAL")
        .map_err(|_| "PAGER_USB_SERIAL is required for updater")?
        .to_uppercase();
    if serial.len() != 16 || !serial.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid chip serial".into());
    }
    let boot = build_bootloader_for(false, &board)?.canonicalize().unwrap();
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(boot.join("package.json")).unwrap()).unwrap();
    let image = fs::read(boot.join("bootloader.bin")).unwrap();
    env::set_var("PAGER_BOOTLOADER_BIN", boot.join("bootloader.bin"));
    env::set_var("PAGER_BOOTLOADER_HASH", hex(&Sha256::digest(&image)));
    build_application(
        false,
        Some(serde_json::json!({
            "schema": 1, "serial": serial, "bootloader": metadata,
            "embedded_sha256": hex(&Sha256::digest(&image)), "embedded_len": image.len()
        })),
    )
}

pub(super) fn build_xiao_installer(release: bool) -> Result<(), String> {
    let root = repo_root();
    if release {
        if env::var_os("PAGER_FAULT_INJECTION").is_some() {
            return Err("fault injection is forbidden in release builds".into());
        }
        if env::var_os("PAGER_SKIP_WATCHDOG_FEED").is_some() {
            return Err("watchdog override is forbidden in release builds".into());
        }
        assert_release_tree(&root)?;
    }
    preflight_build()?;
    let dist = staging_dir(&root, "xiao-nrf52840", release, "installer");

    println!("==================================================");
    println!("     Building stock-XIAO bootloader installer     ");
    println!("==================================================");

    let bootloader_package = build_bootloader_for(release, "xiao-nrf52840")?;
    let bootloader_elf = bootloader_package.join("bootloader.elf");
    let bootloader_bin = dist.join("pager-bootloader-xiao.bin");
    extract_binary(&bootloader_elf, &bootloader_bin)?;
    let bootloader = fs::read(&bootloader_bin).expect("read XIAO bootloader binary");
    let layout = Layout::load(&root)?;
    assert!(
        bootloader.len() <= layout.bootloader_size,
        "XIAO bootloader exceeds its partition"
    );
    assert_valid_vector(&bootloader, 0, "Pager bootloader");

    let installer_target = target_directory(&root, "target/xiao-installer");
    let mut command = Command::new("cargo");
    command
        .args([
            "build",
            "--locked",
            "--manifest-path",
            "xiao-installer/Cargo.toml",
            "--release",
            "--target",
            "thumbv7em-none-eabihf",
            "--target-dir",
        ])
        .arg(&installer_target)
        .env("PAGER_BOOTLOADER_BIN", &bootloader_bin)
        .current_dir(&root);
    run_command(&mut command, "XIAO installer compilation")?;

    let installer_elf = installer_target.join("thumbv7em-none-eabihf/release/pager-xiao-installer");
    let installer_bin = dist.join("pager-xiao-installer.bin");
    extract_binary(&installer_elf, &installer_bin)?;
    let installer = fs::read(&installer_bin).expect("read XIAO installer binary");
    assert_valid_vector(&installer, XIAO_FACTORY_APPLICATION_START, "XIAO installer");
    assert!(
        XIAO_FACTORY_APPLICATION_START as usize + installer.len()
            <= XIAO_FACTORY_BOOTLOADER_START as usize,
        "XIAO installer overlaps the factory bootloader"
    );
    assert!(
        installer
            .windows(bootloader.len())
            .any(|candidate| candidate == bootloader),
        "XIAO installer does not contain the exact Pager bootloader binary"
    );

    let uf2 = encode_xiao_installer_uf2(&installer);
    let uf2_path = dist.join("pager-xiao-installer.uf2");
    fs::write(&uf2_path, &uf2).expect("write XIAO installer UF2");
    fs::copy(&installer_elf, dist.join("installer.elf")).unwrap();
    write_metadata(
        &dist,
        serde_json::json!({"schema": 1, "kind": "factory-installer", "board": "xiao-nrf52840",
        "mode": if release { "release" } else { "dev" }, "bootloader_sha256": hex(&Sha256::digest(&bootloader)),
        "uf2_sha256": hex(&Sha256::digest(&uf2))}),
    )?;
    let published = publish_package(&dist, "installer");
    println!("Installer: {} ({} bytes)", published.display(), uf2.len());
    println!(
        "Embedded {} bootloader: {} bytes",
        if release { "release" } else { "development" },
        bootloader.len()
    );
    Ok(())
}
