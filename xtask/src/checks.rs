use super::*;

pub(super) fn check_size_budgets() -> Result<(), String> {
    let root = repo_root();
    let layout = Layout::load(&root)?;
    let board = selected_board()?;
    let app = artifact_dir(&root, &board, false, "app").join("pager-signed.bin");
    let boot = artifact_dir(&root, &board, false, "bootloader").join("bootloader.bin");
    let app_size = binary_size(&app);
    let boot_size = binary_size(&boot);
    let app_budget = layout.application_partition_size() - 16 * 1024;
    let boot_budget = layout.bootloader_size - 1024;
    assert!(
        app_size <= app_budget,
        "application size {app_size} exceeds budget {app_budget}"
    );
    assert!(
        boot_size <= boot_budget,
        "bootloader size {boot_size} exceeds budget {boot_budget}"
    );
    println!(
        "size budgets: application {app_size}/{app_budget}, bootloader {boot_size}/{boot_budget}"
    );
    let ram_budget = 256 * 1024 - 16 * 1024;
    for (kind, file) in [("app", "pager.elf"), ("bootloader", "bootloader.elf")] {
        let path = artifact_dir(&root, &board, false, kind).join(file);
        let bytes = fs::read(&path).map_err(|error| format!("read RAM budget ELF: {error}"))?;
        let size = elf_ram_size(&bytes)?;
        if size > ram_budget {
            return Err(format!(
                "{kind} static RAM {size} exceeds budget {ram_budget}"
            ));
        }
        println!("RAM budget: {kind} {size}/{ram_budget} bytes (16 KiB runtime margin)");
    }
    Ok(())
}
