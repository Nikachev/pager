#![no_std]
#![no_main]

mod boot_control;
mod manifest;
mod public_key;

use cortex_m_rt::entry;

const SLOT_A_MANIFEST: u32 = 0x0000_8000;
const SLOT_A_IMAGE: u32 = 0x0000_9000;
const SLOT_B_MANIFEST: u32 = 0x0008_2000;
const SLOT_B_IMAGE: u32 = 0x0008_3000;
const SLOT_IMAGE_CAPACITY: usize = 484 * 1024;
const SCB_VTOR: *mut u32 = 0xE000_ED08 as *mut u32;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}

#[entry]
fn main() -> ! {
    let slots = [read_slot(manifest::SLOT_A), read_slot(manifest::SLOT_B)];
    let (control, control_page) = boot_control::read();

    let confirmed = slot_matches(&slots, control.confirmed_slot, control.confirmed_version);
    let chosen = if control.trial_slot != boot_control::NONE {
        // A reset before confirmation is a rollback. A later OTA package with
        // a higher version in that same failed slot is an explicit retry and
        // replaces the stale trial record instead of making the slot permanent
        // unusable until SWD recovery.
        if let Some(candidate) = newer_than(&slots, control.trial_version) {
            unsafe {
                boot_control::write(
                    boot_control::seal(control.trial(candidate.0, candidate.1)),
                    control_page,
                )
            };
            candidate.0
        } else {
            required_slot(confirmed.or_else(|| newest_slot(&slots)))
        }
    } else if let Some(candidate) = newer_than(&slots, control.confirmed_version) {
        // Record the trial before jumping. If it does not confirm before the
        // next reset, the branch above launches the previously confirmed slot.
        unsafe {
            boot_control::write(
                boot_control::seal(control.trial(candidate.0, candidate.1)),
                control_page,
            )
        };
        candidate.0
    } else {
        required_slot(confirmed.or_else(|| newest_slot(&slots)))
    };

    jump(slot_image_start(chosen));
}

fn read_slot(slot: u32) -> Option<(u32, u32)> {
    let start = slot_manifest_start(slot);
    let manifest = unsafe { core::ptr::read_unaligned(start as *const manifest::Manifest) };
    if manifest.target_slot != slot || manifest.image_len as usize > SLOT_IMAGE_CAPACITY {
        return None;
    }
    let image = unsafe {
        core::slice::from_raw_parts(
            slot_image_start(slot) as *const u8,
            manifest.image_len as usize,
        )
    };
    (manifest.verifies(image, SLOT_IMAGE_CAPACITY)
        && valid_vector_table(image, slot_image_start(slot)))
    .then_some((slot, manifest.version))
}

fn valid_vector_table(image: &[u8], image_start: u32) -> bool {
    if image.len() < 8 {
        return false;
    }
    let initial_sp = u32::from_le_bytes(image[..4].try_into().unwrap());
    let reset = u32::from_le_bytes(image[4..8].try_into().unwrap());
    (0x2000_0000..=0x2004_0000).contains(&initial_sp)
        && reset & 1 == 1
        && (image_start..image_start + image.len() as u32).contains(&(reset & !1))
}

fn slot_matches(slots: &[Option<(u32, u32)>; 2], slot: u32, version: u32) -> Option<u32> {
    slots
        .iter()
        .flatten()
        .find(|(s, v)| *s == slot && *v == version)
        .map(|(s, _)| *s)
}

fn newest_slot(slots: &[Option<(u32, u32)>; 2]) -> Option<u32> {
    slots
        .iter()
        .flatten()
        .max_by_key(|(_, version)| *version)
        .map(|(slot, _)| *slot)
}

fn newer_than(slots: &[Option<(u32, u32)>; 2], version: u32) -> Option<(u32, u32)> {
    slots
        .iter()
        .flatten()
        .filter(|(_, candidate)| *candidate > version)
        .max_by_key(|(_, candidate)| *candidate)
        .copied()
}

fn slot_manifest_start(slot: u32) -> u32 {
    if slot == manifest::SLOT_A {
        SLOT_A_MANIFEST
    } else {
        SLOT_B_MANIFEST
    }
}

fn slot_image_start(slot: u32) -> u32 {
    if slot == manifest::SLOT_A {
        SLOT_A_IMAGE
    } else {
        SLOT_B_IMAGE
    }
}

fn no_firmware() -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}

fn required_slot(slot: Option<u32>) -> u32 {
    match slot {
        Some(slot) => slot,
        None => no_firmware(),
    }
}

fn jump(image_start: u32) -> ! {
    unsafe {
        core::ptr::write_volatile(SCB_VTOR, image_start);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        cortex_m::asm::bootload(image_start as *const u32)
    }
}
