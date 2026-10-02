use super::*;

pub(super) fn check_protocol() -> Result<(), String> {
    let root = repo_root();
    run_command(
        Command::new(root.join(".venv/bin/python"))
            .arg(root.join("tools/generate_protocol.py"))
            .arg("--check")
            .current_dir(&root),
        "generated protocol check",
    )?;
    run_command(
        Command::new("node")
            .arg(root.join("tests/protocol_vectors.cjs"))
            .current_dir(&root),
        "JS protocol vectors",
    )
}

pub(super) fn check_ui() -> Result<(), String> {
    let root = repo_root();
    run_command(
        Command::new(root.join(".venv/bin/python"))
            .arg(root.join("tools/build_ui.py"))
            .arg("--check")
            .current_dir(&root),
        "standalone UI freshness",
    )?;
    for name in ["webusb_client.html", "ble_client.html"] {
        let page = fs::read_to_string(root.join(name)).map_err(|e| e.to_string())?;
        if !page.contains("<!doctype html>") || !page.contains("<style>") {
            return Err(format!("{name} must embed standalone HTML/CSS"));
        }
        if page.contains("http://") || page.contains("https://") || page.contains("<script src=") {
            return Err(format!("{name} must not depend on network resources"));
        }
    }
    run_command(
        Command::new("node")
            .arg("--test")
            .arg(root.join("tests/ui_behavior.cjs"))
            .current_dir(&root),
        "mock WebUSB/UI behaviour",
    )
}

pub(super) fn build_ui() -> Result<(), String> {
    let root = repo_root();
    run_command(
        Command::new(root.join(".venv/bin/python"))
            .arg(root.join("tools/build_ui.py"))
            .current_dir(&root),
        "autonomous HTML build",
    )
}
