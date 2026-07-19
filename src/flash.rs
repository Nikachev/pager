//! Low-level raw NVMC Flash operations for Web OTA Update

const NVMC_READY: *mut u32 = 0x4001E400 as *mut u32;
const NVMC_CONFIG: *mut u32 = 0x4001E504 as *mut u32;
const NVMC_ERASEPAGE: *mut u32 = 0x4001E508 as *mut u32;

#[link_section = ".data"]
#[inline(never)]
pub unsafe fn copy_and_reset(src_addr: u32, dest_addr: u32, len_bytes: u32) -> ! {
    cortex_m::interrupt::disable();

    // 1. Erase destination pages (4KB per page) in Active Bank
    let page_size = 4096;
    let num_pages = (len_bytes + page_size - 1) / page_size;
    for page_idx in 0..num_pages {
        let page_addr = dest_addr + page_idx * page_size;

        while core::ptr::read_volatile(NVMC_READY) == 0 {}
        core::ptr::write_volatile(NVMC_CONFIG, 2); // Enable Erase
        core::ptr::write_volatile(NVMC_ERASEPAGE, page_addr);
        while core::ptr::read_volatile(NVMC_READY) == 0 {}
    }

    // 2. Copy staged binary word by word (4 bytes / 32-bit words)
    let count_words = (len_bytes + 3) / 4;
    let src_ptr = src_addr as *const u32;
    let dest_ptr = dest_addr as *mut u32;

    for i in 0..count_words {
        let val = core::ptr::read_volatile(src_ptr.offset(i as isize));

        while core::ptr::read_volatile(NVMC_READY) == 0 {}
        core::ptr::write_volatile(NVMC_CONFIG, 1); // Enable Write
        core::ptr::write_volatile(dest_ptr.offset(i as isize), val);
        while core::ptr::read_volatile(NVMC_READY) == 0 {}
    }

    // 3. System Reset via AIRCR
    while core::ptr::read_volatile(NVMC_READY) == 0 {}
    core::ptr::write_volatile(NVMC_CONFIG, 0); // Read-Only mode
    while core::ptr::read_volatile(NVMC_READY) == 0 {}

    let aircr = 0xE000ED0C as *mut u32;
    core::ptr::write_volatile(aircr, 0x05FA0004);

    loop {}
}

