#![no_std]

#[cfg(test)]
extern crate std;

pub mod ble;
pub mod flash;
pub mod protocol;

pub mod layout {
    include!(concat!(env!("OUT_DIR"), "/layout.rs"));
}
