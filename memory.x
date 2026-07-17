MEMORY
{
  /* SoftDevice S140 v7.3.0 size: 156KB (0x27000) */
  FLASH : ORIGIN = 0x00027000, LENGTH = 868K
  
  /* RAM starts at 0x20010000 to give the SoftDevice 64KB of RAM (adjustable). Length is 192K. */
  RAM   : ORIGIN = 0x20010000, LENGTH = 192K
}
