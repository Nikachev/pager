//! Low-level raw NVMC Flash operations for Web OTA Update

#[link_section = ".data"]
#[inline(never)]
pub unsafe fn copy_and_reset(src_addr: u32, dest_addr: u32, len_bytes: u32) -> ! {
    cortex_m::interrupt::disable();

    let nvmc_ready = 0x4001E400 as *mut u32;
    let nvmc_config = 0x4001E504 as *mut u32;
    let nvmc_erasepage = 0x4001E508 as *mut u32;

    // 1. Erase destination pages (4KB per page) in Active Bank
    let page_size = 4096;
    let num_pages = (len_bytes + page_size - 1) / page_size;
    for page_idx in 0..num_pages {
        let page_addr = dest_addr + page_idx * page_size;

        while core::ptr::read_volatile(nvmc_ready) == 0 {}
        core::ptr::write_volatile(nvmc_config, 2); // Enable Erase
        core::ptr::write_volatile(nvmc_erasepage, page_addr);
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
    }

    // 2. Copy staged binary word by word (4 bytes / 32-bit words)
    let count_words = (len_bytes + 3) / 4;
    let src_ptr = src_addr as *const u32;
    let dest_ptr = dest_addr as *mut u32;

    for i in 0..count_words {
        let val = core::ptr::read_volatile(src_ptr.offset(i as isize));

        while core::ptr::read_volatile(nvmc_ready) == 0 {}
        core::ptr::write_volatile(nvmc_config, 1); // Enable Write
        core::ptr::write_volatile(dest_ptr.offset(i as isize), val);
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
    }

    // 3. System Reset via AIRCR
    while core::ptr::read_volatile(nvmc_ready) == 0 {}
    core::ptr::write_volatile(nvmc_config, 0); // Read-Only mode
    while core::ptr::read_volatile(nvmc_ready) == 0 {}

    let aircr = 0xE000ED0C as *mut u32;
    core::ptr::write_volatile(aircr, 0x05FA0004);

    loop {}
}

pub unsafe fn raw_flash_erase(start_addr: u32, len_bytes: u32) {
    let nvmc_ready = 0x4001E400 as *mut u32;
    let nvmc_config = 0x4001E504 as *mut u32;
    let nvmc_erasepage = 0x4001E508 as *mut u32;

    let page_size = 4096;
    let num_pages = (len_bytes + page_size - 1) / page_size;
    for page_idx in 0..num_pages {
        let page_addr = start_addr + page_idx * page_size;
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
        core::ptr::write_volatile(nvmc_config, 2); // Enable Erase
        core::ptr::write_volatile(nvmc_erasepage, page_addr);
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
    }
    core::ptr::write_volatile(nvmc_config, 0); // Return to Read mode
    while core::ptr::read_volatile(nvmc_ready) == 0 {}
}

pub unsafe fn raw_flash_write_block(dest_addr: u32, data: &[u8]) {
    let nvmc_ready = 0x4001E400 as *mut u32;
    let nvmc_config = 0x4001E504 as *mut u32;

    let count_words = (data.len() + 3) / 4;
    let src_ptr = data.as_ptr() as *const u32;
    let dest_ptr = dest_addr as *mut u32;

    for i in 0..count_words {
        let val = core::ptr::read_volatile(src_ptr.offset(i as isize));
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
        core::ptr::write_volatile(nvmc_config, 1); // Enable Write
        core::ptr::write_volatile(dest_ptr.offset(i as isize), val);
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
    }
    core::ptr::write_volatile(nvmc_config, 0); // Return to Read mode
    while core::ptr::read_volatile(nvmc_ready) == 0 {}
}
