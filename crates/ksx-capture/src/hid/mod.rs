//! HID plumbing for the WinUSB backend: report-descriptor parsing, report
//! decoding, and the usage → legacy-key vocabulary bridge.
//!
//! Platform-free and OS-free on purpose — this is the part of M6 that is fully
//! CI-testable, and it is tested against **synthetic** report bytes assembled
//! from the HID 1.11 spec ([`fixtures`]), never against the cabinet's live
//! I-PAC. See [`report`] for the rollover rule and [`usage`] for why the
//! translation goes through set-1 scancodes instead of straight to
//! [`ksx_core::Key`].

pub mod descriptor;
pub mod fixtures;
pub mod report;
pub mod usage;

pub use descriptor::{Field, KeyboardFormat, ReportFormat};
pub use report::{ReportKind, ReportReader, UsageSet};
pub use usage::{usage_to_scancode, usage_to_stroke, PAGE_KEYBOARD};

/// `bDescriptorType` for a HID report descriptor (HID 1.11 §7.1.1) — the
/// `wValue` high byte of the `GET_DESCRIPTOR` request the backend issues once
/// at claim time.
pub const DESCRIPTOR_TYPE_REPORT: u8 = 0x22;

/// USB interface class for HID.
pub const INTERFACE_CLASS_HID: u8 = 0x03;

/// HID interface subclass 1 = "boot interface".
pub const INTERFACE_SUBCLASS_BOOT: u8 = 0x01;

/// HID boot-interface protocol 1 = keyboard.
pub const INTERFACE_PROTOCOL_KEYBOARD: u8 = 0x01;
