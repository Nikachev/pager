//! USB Mass Storage implementation for [usb-device]
//!
//! # Subclasses:
//! * [SCSI] - SCSI device
//!
//! # Transports:
//! * [Bulk Only]
//!
//! # Features
//! | Feature | Description                           |
//! | ------- |---------------------------------------|
//! | `bbb` | Include Bulk Only Transport           |
//! | `scsi` | Include SCSI subclass                 |
//! | `defmt` | Enable logging via [defmt](https://crates.io/crates/defmt) crate |
//!
//! [usb-device]: https://crates.io/crates/usb-device
//! [SCSI]: crate::subclass::scsi
//! [Bulk Only]: crate::transport::bbb
//! [Transport]: crate::transport::Transport

#![no_std]

#[cfg(feature = "bbb")]
pub(crate) mod buffer;
pub(crate) mod fmt;
pub mod subclass;
pub mod transport;

/// USB Mass Storage Class code
pub const CLASS_MASS_STORAGE: u8 = 0x08;
