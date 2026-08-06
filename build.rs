use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    // Put `memory.x` in our output directory and ensure it's on the linker search path.
    let out = &PathBuf::from(env::var("OUT_DIR").unwrap());
    let memory = include_bytes!("memory.x").as_slice();
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(memory)
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg=-Tdefmt.x");

    // Ensure compilation re-runs if linker configuration or layout changes.
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=layout.json");
    let skip_trial_confirm = env::var("PAGER_SKIP_TRIAL_CONFIRM").unwrap_or_default();
    println!("cargo:rustc-env=PAGER_SKIP_TRIAL_CONFIRM={skip_trial_confirm}");
    let skip_watchdog_feed = env::var("PAGER_SKIP_WATCHDOG_FEED").unwrap_or_default();
    println!("cargo:rustc-env=PAGER_SKIP_WATCHDOG_FEED={skip_watchdog_feed}");
}
