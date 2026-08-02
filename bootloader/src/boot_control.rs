//! Two-page journal for selecting and confirming an A/B image.
//!
//! The next record is always written to the other page.  Therefore a reset
//! during erase/programming leaves the preceding, complete record intact.

pub const PAGE0: u32 = 0x000F_C000;
pub const PAGE1: u32 = 0x000F_D000;
pub const NONE: u32 = u32::MAX;
const MAGIC: [u8; 8] = *b"PGRAB001";

const NVMC_READY: *mut u32 = 0x4001_E400 as *mut u32;
const NVMC_CONFIG: *mut u32 = 0x4001_E504 as *mut u32;
const NVMC_ERASEPAGE: *mut u32 = 0x4001_E508 as *mut u32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Record {
    magic: [u8; 8],
    pub generation: u32,
    pub confirmed_slot: u32,
    pub confirmed_version: u32,
    pub trial_slot: u32,
    pub trial_version: u32,
    checksum: u32,
}

impl Record {
    pub const fn initial() -> Self {
        Self {
            magic: MAGIC,
            generation: 0,
            confirmed_slot: 0,
            confirmed_version: 0,
            trial_slot: NONE,
            trial_version: 0,
            checksum: 0,
        }
    }

    pub fn trial(self, slot: u32, version: u32) -> Self {
        let mut next = self;
        next.generation = next.generation.wrapping_add(1);
        next.trial_slot = slot;
        next.trial_version = version;
        next.checksum = 0;
        next
    }

    fn valid(&self) -> bool {
        self.magic == MAGIC
            && self.confirmed_slot <= 1
            && (self.trial_slot <= 1 || self.trial_slot == NONE)
            && self.checksum == checksum(self)
    }
}

pub fn read() -> (Record, u32) {
    let first = unsafe { core::ptr::read_unaligned(PAGE0 as *const Record) };
    let second = unsafe { core::ptr::read_unaligned(PAGE1 as *const Record) };
    match (first.valid(), second.valid()) {
        (true, true) if second.generation.wrapping_sub(first.generation) < 0x8000_0000 => {
            (second, PAGE1)
        }
        (true, _) => (first, PAGE0),
        (_, true) => (second, PAGE1),
        _ => (Record::initial(), PAGE1),
    }
}

pub fn seal(mut record: Record) -> Record {
    record.checksum = checksum(&record);
    record
}

/// Persist a complete record on the inactive journal page.
///
/// It must execute from RAM because nRF52 flash is unavailable while NVMC
/// erases/programs a page.
#[link_section = ".data"]
#[inline(never)]
pub unsafe fn write(record: Record, current_page: u32) {
    let target = if current_page == PAGE0 { PAGE1 } else { PAGE0 };

    while core::ptr::read_volatile(NVMC_READY) == 0 {}
    core::ptr::write_volatile(NVMC_CONFIG, 2);
    core::ptr::write_volatile(NVMC_ERASEPAGE, target);
    while core::ptr::read_volatile(NVMC_READY) == 0 {}

    #[cfg(power_cut_test)]
    loop {
        // HIL fixture: power-cycle here, after the inactive page is erased
        // but before any word is programmed. The preceding journal page must
        // still boot the confirmed image.
        cortex_m::asm::wfi();
    }

    let source = core::ptr::from_ref(&record).cast::<u32>();
    let destination = target as *mut u32;
    for index in 0..(core::mem::size_of::<Record>() / 4) {
        core::ptr::write_volatile(NVMC_CONFIG, 1);
        core::ptr::write_volatile(destination.add(index), core::ptr::read_volatile(source.add(index)));
        while core::ptr::read_volatile(NVMC_READY) == 0 {}
    }
    core::ptr::write_volatile(NVMC_CONFIG, 0);
}

fn checksum(record: &Record) -> u32 {
    let bytes = unsafe {
        core::slice::from_raw_parts(
            core::ptr::from_ref(record).cast::<u8>(),
            core::mem::size_of::<Record>() - core::mem::size_of::<u32>(),
        )
    };
    let mut value = 0x811C_9DC5u32;
    for byte in bytes {
        value = (value ^ u32::from(*byte)).wrapping_mul(0x0100_0193);
    }
    value
}
