#![no_std]
#![no_main]
#[cfg(any(
    all(feature = "board-nice-nano-v2", feature = "board-xiao-nrf52840"),
    not(any(feature = "board-nice-nano-v2", feature = "board-xiao-nrf52840"))
))]
compile_error!("select exactly one board");
use core::ptr::{read_volatile, write_volatile};
use cortex_m_rt::entry;
use pager_bootloader_updater::{Error, Flash, Target, BOOTLOADER_END};
include!(concat!(env!("OUT_DIR"), "/target.rs"));
#[used]
#[no_mangle]
#[link_section = ".vector_table.interrupts"]
static __INTERRUPTS: [u32; 48] = [0; 48];
static IMAGE: &[u8] = include_bytes!(env!("PAGER_UPDATER_BOOTLOADER"));
#[cfg(feature = "board-nice-nano-v2")]
const BOARD: u32 = 1;
#[cfg(feature = "board-xiao-nrf52840")]
const BOARD: u32 = 2;
struct Nvmc;
// NVMC READY is a mapped, word-aligned nRF52840 register.
fn ready() {
    while unsafe { read_volatile(0x4001E400 as *const u32) } == 0 {
        feed();
    }
}
// Eight WDT reload registers are the complete hardware array.
fn feed() {
    unsafe {
        if read_volatile(0x40010400 as *const u32) != 0 {
            let enabled = read_volatile(0x40010508 as *const u32);
            for i in 0..8u32 {
                if enabled & (1 << i) != 0 {
                    write_volatile((0x40010600 + i * 4) as *mut u32, 0x6E524635);
                }
            }
        }
    }
}
impl Flash for Nvmc {
    fn accessible(&self, end: u32) -> bool {
        for i in 0..8u32 {
            let base = 0x4001E800 + i * 16;
            // Eight 16-byte ACL entries are mapped in the NVMC register block.
            unsafe {
                if pager_bootloader_updater::acl_blocks(
                    read_volatile(base as *const u32),
                    read_volatile((base + 4) as *const u32),
                    read_volatile((base + 8) as *const u32),
                    end,
                ) {
                    return false;
                }
            }
        }
        true
    }
    // replace() only supplies aligned pages in [0, BOOTLOADER_END).
    fn erase_page(&mut self, address: u32) -> Result<(), Error> {
        unsafe {
            write_volatile(0x4001E504 as *mut u32, 2);
            ready();
            write_volatile(0x4001E508 as *mut u32, address);
            ready();
        }
        Ok(())
    }
    // replace_page() supplies aligned words in the validated boot partition.
    fn write_word(&mut self, address: u32, value: u32) -> Result<(), Error> {
        unsafe {
            write_volatile(0x4001E504 as *mut u32, 1);
            ready();
            write_volatile(address as *mut u32, value);
            ready();
        }
        Ok(())
    }
    // All replace()/replace_page() readback loops stay in that same partition.
    fn read_word(&self, address: u32) -> u32 {
        unsafe { read_volatile(address as *const u32) }
    }
    fn feed_watchdog(&mut self) {
        feed();
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    fail()
}
fn fail() -> ! {
    loop {
        feed();
        cortex_m::asm::nop();
    }
}
#[entry]
fn main() -> ! {
    // FICR DEVICEID contains two mapped, aligned 32-bit words.
    let serial = unsafe {
        (read_volatile(0x10000064 as *const u32) as u64) << 32
            | read_volatile(0x10000060 as *const u32) as u64
    };
    let target = Target {
        board: BOARD,
        serial: u64::from_le_bytes(DESCRIPTOR[8..16].try_into().unwrap()),
        bootloader_end: BOOTLOADER_END,
        digest: DESCRIPTOR[16..48].try_into().unwrap(),
    };
    if u32::from_le_bytes(DESCRIPTOR[48..52].try_into().unwrap()) != BOARD
        || u32::from_le_bytes(DESCRIPTOR[52..56].try_into().unwrap()) as usize != IMAGE.len()
    {
        fail();
    }
    let mut flash = Nvmc;
    if pager_bootloader_updater::preflight(IMAGE, &target, BOARD, serial, &flash).is_err() {
        fail();
    }
    unsafe {
        cortex_m::interrupt::disable();
        write_volatile(0xE000E010 as *mut u32, 0);
        for i in 0..8u32 {
            write_volatile((0xE000E180 + i * 4) as *mut u32, u32::MAX);
            write_volatile((0xE000E280 + i * 4) as *mut u32, u32::MAX);
        }
        write_volatile(0x40027504 as *mut u32, 0);
        write_volatile(0x40027500 as *mut u32, 0);
    }
    if pager_bootloader_updater::replace(IMAGE, &target, BOARD, serial, &mut flash).is_err() {
        fail();
    }
    unsafe {
        write_volatile(0x4001E504 as *mut u32, 0);
    }
    ready();
    feed();
    unsafe {
        write_volatile(0x4000051C as *mut u32, 0xB1);
    }
    cortex_m::peripheral::SCB::sys_reset()
}
