use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    // Put `memory.x` in our output directory and ensure it's on the linker search path.
    let out = &PathBuf::from(env::var("OUT_DIR").unwrap());
    let slot = env::var("PAGER_SLOT").unwrap_or_else(|_| "A".into());
    let memory = match slot.as_str() {
        "A" => include_bytes!("memory_slot_a.x").as_slice(),
        "B" => include_bytes!("memory_slot_b.x").as_slice(),
        _ => panic!("PAGER_SLOT must be A or B"),
    };
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(memory)
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    // Ensure compilation re-runs if memory.x changes.
    println!("cargo:rerun-if-changed=memory_slot_a.x");
    println!("cargo:rerun-if-changed=memory_slot_b.x");
    println!("cargo:rerun-if-env-changed=PAGER_SLOT");
    println!("cargo:rerun-if-env-changed=PAGER_SKIP_TRIAL_CONFIRM");
    println!("cargo:rerun-if-env-changed=PAGER_SKIP_WATCHDOG_FEED");
    println!("cargo:rustc-env=PAGER_SLOT={slot}");
    let skip_trial_confirm = env::var("PAGER_SKIP_TRIAL_CONFIRM").unwrap_or_default();
    println!("cargo:rustc-env=PAGER_SKIP_TRIAL_CONFIRM={skip_trial_confirm}");
    let skip_watchdog_feed = env::var("PAGER_SKIP_WATCHDOG_FEED").unwrap_or_default();
    println!("cargo:rustc-env=PAGER_SKIP_WATCHDOG_FEED={skip_watchdog_feed}");
}
