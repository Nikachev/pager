MEMORY
{
  /* Bare-metal SWD flashing: starting at 0x00000000 (no bootloader / S140).
     Reserve last 16K (0xFC000..0xFFFFF) for MPSL/SDC flash storage. */
  FLASH   : ORIGIN = 0x00000000, LENGTH = 1008K
  STORAGE : ORIGIN = 0x000FC000, LENGTH = 16K
  /* Full 256K RAM starting at 0x20000000 */
  RAM     : ORIGIN = 0x20000000, LENGTH = 256K
}

__storage_start = ORIGIN(STORAGE);
__storage_end = ORIGIN(STORAGE) + LENGTH(STORAGE);
