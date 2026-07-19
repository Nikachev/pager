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

use embedded_storage_async::nor_flash::NorFlash;

pub struct OtaWriter<'a, F: NorFlash> {
    flash: &'a mut F,
    offset: u32,
    write_buffer: [u8; 256],
    write_buf_len: usize,
}

impl<'a, F: NorFlash> OtaWriter<'a, F> {
    pub fn new(flash: &'a mut F, start_addr: u32) -> Self {
        Self {
            flash,
            offset: start_addr,
            write_buffer: [0u8; 256],
            write_buf_len: 0,
        }
    }

    pub async fn erase(&mut self, size: usize) -> Result<(), F::Error> {
        let page_size = 4096;
        let erase_size = (size + page_size - 1) & !(page_size - 1);
        crate::log_msg!("Erasing staging partition: {} bytes...", erase_size);
        self.flash.erase(self.offset, self.offset + erase_size as u32).await
    }

    pub async fn write_chunk(&mut self, data: &[u8]) -> Result<(), F::Error> {
        let mut data_idx = 0;
        while data_idx < data.len() {
            let chunk_size = core::cmp::min(data.len() - data_idx, self.write_buffer.len() - self.write_buf_len);
            self.write_buffer[self.write_buf_len..self.write_buf_len + chunk_size]
                .copy_from_slice(&data[data_idx..data_idx + chunk_size]);
            self.write_buf_len += chunk_size;
            data_idx += chunk_size;

            if self.write_buf_len == self.write_buffer.len() {
                self.flash.write(self.offset, &self.write_buffer).await?;
                self.offset += self.write_buffer.len() as u32;
                self.write_buf_len = 0;
            }
        }
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<(), F::Error> {
        if self.write_buf_len > 0 {
            self.flash.write(self.offset, &self.write_buffer[..self.write_buf_len]).await?;
            self.offset += self.write_buf_len as u32;
            self.write_buf_len = 0;
        }
        Ok(())
    }
}


