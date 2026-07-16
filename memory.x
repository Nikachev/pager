MEMORY
{
  /* FLASH offset is 0x1000. Length is 1020K (1024K total minus 4K MBR). */
  FLASH : ORIGIN = 0x00001000, LENGTH = 1020K
  
  /* RAM origin is shifted by 8 bytes to protect the Adafruit bootloader's */
  /* soft reset / boot detection magic flags at the start of RAM (0x20000000). */
  RAM   : ORIGIN = 0x20000008, LENGTH = 262136
}
