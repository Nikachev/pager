use std::{env, fs};
use std::path::PathBuf;

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
}
