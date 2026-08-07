//! Friendly names for USB vendors and boards — **display only**, by
//! construction.
//!
//! `docs/DEVICE-IDENTITY.md` §6 draws the line this module exists to hold:
//!
//! - A vendor id **may** choose a name to show a human. `Ultimarc I-PAC 4X`
//!   reads better than `USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000`, and
//!   picking that string is a lookup, not a decision.
//! - A vendor id **may not** gate capture, claiming, refusal or backend
//!   selection. ksx works with any HID keyboard interface; code that asks "is
//!   this an Ultimarc?" to decide *what to do* is the hardcoding the rewrite
//!   exists to remove.
//!
//! Everything here returns a `&str` or an `Option<&str>`. Nothing returns a
//! `bool`, and that is deliberate: `is_ipac()` is the shape that invites a
//! branch, and three separate copies of it are what this module replaces.
//!
//! # The bug that motivated it
//!
//! `ksx devices` on the reference cabinet printed:
//!
//! ```text
//!   USB\VID_D209&PID_15A2\6  "SpinTrak"  [I-PAC]
//! ```
//!
//! A SpinTrak is a trackball. It is not an I-PAC. Three copies of Ultimarc's
//! vendor id lived in three crates, each answering `is_ipac`/`is_ultimarc` from
//! the VID alone, so every Ultimarc product on the machine claimed to be the
//! one board the author happened to own. The device's own `iProduct` string
//! already said "SpinTrak" — the label was overriding better information with
//! worse.
//!
//! So the lookup is keyed on **vendor and product**, with the vendor as the
//! fallback, and the device's own product string always wins when it has one.

/// Ultimarc — arcade encoders and controls. The reference cabinet's vendor.
pub const ULTIMARC_VID: u16 = 0xD209;

/// One row of the table: a specific board, or a whole vendor when `product` is
/// `None`.
struct Vendor {
    vendor_id: u16,
    /// `None` means "any product from this vendor".
    product_id: Option<u16>,
    /// What a human calls it.
    name: &'static str,
}

/// The table. Adding hardware is a one-line data edit, in one place.
///
/// Deliberately short. This is not a USB ID database and must never grow into
/// one — it exists so the boards ksx is routinely pointed at read nicely, and
/// every device not listed here falls back to its own `iProduct` string, which
/// is usually better than anything we could hardcode.
const VENDORS: &[Vendor] = &[
    Vendor {
        vendor_id: ULTIMARC_VID,
        product_id: Some(0x0430),
        name: "Ultimarc I-PAC 4X",
    },
    Vendor {
        vendor_id: ULTIMARC_VID,
        product_id: Some(0x15A2),
        name: "Ultimarc SpinTrak",
    },
    Vendor {
        vendor_id: ULTIMARC_VID,
        product_id: None,
        name: "Ultimarc",
    },
];

/// The name of a known board, or of its vendor, if either is known.
///
/// `None` is the normal answer and not a failure — most devices are not in the
/// table and do not need to be.
pub fn name_for(vendor_id: u16, product_id: u16) -> Option<&'static str> {
    // Exact board first, vendor second: the fallback must not shadow a specific
    // match, which is precisely how a SpinTrak came to be called an I-PAC.
    VENDORS
        .iter()
        .find(|v| v.vendor_id == vendor_id && v.product_id == Some(product_id))
        .or_else(|| {
            VENDORS
                .iter()
                .find(|v| v.vendor_id == vendor_id && v.product_id.is_none())
        })
        .map(|v| v.name)
}

/// The vendor's name alone, ignoring the product.
///
/// For a row that already prints the product string and wants only to say who
/// made it.
pub fn vendor_name(vendor_id: u16) -> Option<&'static str> {
    VENDORS
        .iter()
        .find(|v| v.vendor_id == vendor_id)
        .map(|v| match v.product_id {
            // Prefer the vendor-wide row's name when there is one.
            None => v.name,
            Some(_) => VENDORS
                .iter()
                .find(|other| other.vendor_id == vendor_id && other.product_id.is_none())
                .map_or(v.name, |other| other.name),
        })
}

/// What to show a human for one device, best information first.
///
/// The device's own `iProduct` descriptor wins whenever it has one: it is what
/// the manufacturer chose to call this exact board, and it is right about
/// hardware nobody has added to the table. The table is the fallback for a
/// board that reports nothing useful, and the raw ids are the last resort.
pub fn display_name(vendor_id: u16, product_id: u16, reported: Option<&str>) -> String {
    match reported.map(str::trim).filter(|s| !s.is_empty()) {
        Some(product) => match vendor_name(vendor_id) {
            // Don't say "Ultimarc Ultimarc SpinTrak".
            Some(vendor)
                if !product
                    .to_ascii_lowercase()
                    .contains(&vendor.to_ascii_lowercase()) =>
            {
                format!("{vendor} {product}")
            }
            _ => product.to_owned(),
        },
        None => name_for(vendor_id, product_id)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("USB {vendor_id:04X}:{product_id:04X}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression this module was written for.
    #[test]
    fn a_spintrak_is_not_an_ipac() {
        assert_eq!(name_for(ULTIMARC_VID, 0x15A2), Some("Ultimarc SpinTrak"));
        assert_eq!(name_for(ULTIMARC_VID, 0x0430), Some("Ultimarc I-PAC 4X"));
        assert_ne!(
            name_for(ULTIMARC_VID, 0x15A2),
            name_for(ULTIMARC_VID, 0x0430),
            "two different Ultimarc products must not share one name"
        );
    }

    #[test]
    fn an_unlisted_product_falls_back_to_its_vendor_not_to_a_sibling_board() {
        // A future Ultimarc board ksx has never heard of.
        assert_eq!(name_for(ULTIMARC_VID, 0xBEEF), Some("Ultimarc"));
    }

    #[test]
    fn an_unknown_vendor_is_simply_unknown() {
        assert_eq!(name_for(0x1234, 0x5678), None);
    }

    /// The device knows its own name better than any table we maintain.
    #[test]
    fn the_devices_own_product_string_wins() {
        assert_eq!(
            display_name(ULTIMARC_VID, 0x15A2, Some("SpinTrak")),
            "Ultimarc SpinTrak"
        );
        assert_eq!(
            display_name(0x1234, 0x5678, Some("Generic Keyboard")),
            "Generic Keyboard"
        );
    }

    #[test]
    fn the_vendor_is_not_repeated_when_the_product_already_says_it() {
        assert_eq!(
            display_name(ULTIMARC_VID, 0x0430, Some("Ultimarc I-PAC 4X")),
            "Ultimarc I-PAC 4X",
            "never 'Ultimarc Ultimarc I-PAC 4X'"
        );
    }

    #[test]
    fn a_silent_device_falls_back_to_the_table_then_to_its_ids() {
        assert_eq!(
            display_name(ULTIMARC_VID, 0x0430, None),
            "Ultimarc I-PAC 4X"
        );
        assert_eq!(display_name(0x1234, 0x5678, None), "USB 1234:5678");
        assert_eq!(
            display_name(0x1234, 0x5678, Some("   ")),
            "USB 1234:5678",
            "a blank product string is no product string"
        );
    }
}
