use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("memory.x"), include_bytes!("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-env-changed=PAGER_POWER_CUT_TEST");
    println!("cargo:rustc-check-cfg=cfg(power_cut_test)");
    if std::env::var("PAGER_POWER_CUT_TEST").as_deref() == Ok("1") {
        println!("cargo:rustc-cfg=power_cut_test");
    }

    let date_output = std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%d %H:%M:%S UTC"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "2026-08-08 21:30:00 UTC".to_string());
    println!("cargo:rustc-env=BUILD_DATE={}", date_output);
}
