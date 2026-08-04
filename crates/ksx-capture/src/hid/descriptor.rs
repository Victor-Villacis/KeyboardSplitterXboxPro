//! HID **report descriptor** parser, reduced to the one question the capture
//! loop asks: *given these report bytes, which keyboard usages are pressed?*
//!
//! # Why parse instead of hardcoding the boot report
//!
//! The I-PAC4 is NKRO-capable (`docs/research/keyboard-capture-2026.md` §5), so
//! it does **not** deliver 8-byte boot reports on the interface we claim — it
//! delivers whatever its report descriptor says, which for NKRO boards is
//! usually a bitmap, sometimes behind a report ID, sometimes alongside a boot-
//! shaped array in the same descriptor. Hardcoding either shape means the board
//! works until a firmware update (Ultimarc ships them; §5 lists 1.55/1.56)
//! silently re-lays out the report and every key lands on the wrong pad.
//!
//! Once WinUSB owns the interface there is no `hidclass.sys` above us to do
//! this, so the descriptor request (`GET_DESCRIPTOR(Report)`, wValue
//! `0x2200`) and this parser are the *only* thing standing between raw bytes
//! and a key. It runs exactly once, at claim time, on the cold path.
//!
//! # What is modelled and what is deliberately not
//!
//! Modelled: short items, Push/Pop, report IDs, and the two field shapes a
//! keyboard can use for the Keyboard/Keypad page (0x07):
//!
//! - **Variable** (`Input (Data,Var,Abs)`, report size 1) → a *bitmap*: bit *i*
//!   is usage `usage_min + i`. This is the boot report's modifier byte and the
//!   whole of an NKRO report.
//! - **Array** (`Input (Data,Ary,Abs)`) → *slots*: each element holds a usage
//!   value, `0` meaning empty. This is the boot report's six key slots.
//!
//! Not modelled, on purpose: Output/Feature reports (we never write to the
//! board — LED control is out of scope for M6), non-keyboard usage pages
//! (the I-PAC's MI_01/MI_02 collections are a separate interface we do not
//! claim), long items (no vendor in this stack emits them; they are skipped by
//! length so a descriptor containing one still parses), and multi-byte usages
//! above 0xFF on page 0x07 (the page's last defined usage is 0xE7).
//!
//! Every parse failure is *soft*: an unparseable or key-less descriptor yields
//! `None`/an empty format, and the caller refuses to claim rather than guessing
//! a layout. Guessing is how you get a stuck modifier on a captured board.

use super::usage::PAGE_KEYBOARD;

/// A parsed input field carrying Keyboard/Keypad-page usages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// One bit per usage: bit `n` of the field is `usage_min + n`.
    Bitmap {
        /// Bit offset of the field within the report *payload* (report ID byte,
        /// when present, already excluded).
        bit_offset: usize,
        /// Number of bits, i.e. number of usages covered.
        bits: usize,
        /// Usage of bit 0.
        usage_min: u8,
    },
    /// `count` slots of `bits` bits each; each slot holds a usage value.
    Array {
        bit_offset: usize,
        /// Bits per slot (8 for every keyboard in existence; parsed anyway).
        bits: usize,
        count: usize,
    },
}

/// How to read pressed usages out of one report ID's payload.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReportFormat {
    /// `0` when the descriptor declares no report IDs (payload starts at byte
    /// 0); otherwise the ID that must match the report's first byte.
    pub report_id: u8,
    /// Total payload size in bytes, rounded up from the declared bit length.
    pub payload_len: usize,
    /// Keyboard-page fields, in descriptor order.
    pub fields: Vec<Field>,
}

impl ReportFormat {
    /// Does this format carry any keyboard usages at all?
    pub fn is_keyboard(&self) -> bool {
        !self.fields.is_empty()
    }
}

/// Every keyboard-bearing input report the interface can send.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyboardFormat {
    /// One entry per report ID that carries keyboard usages.
    pub reports: Vec<ReportFormat>,
    /// `true` when the descriptor declares report IDs, i.e. every report on the
    /// wire is prefixed with its ID byte.
    pub uses_report_ids: bool,
}

impl KeyboardFormat {
    /// Longest report we can receive, including the report-ID byte. Used to
    /// size the (single, reused) transfer buffer.
    pub fn max_report_len(&self) -> usize {
        let prefix = usize::from(self.uses_report_ids);
        self.reports
            .iter()
            .map(|r| r.payload_len + prefix)
            .max()
            .unwrap_or(0)
    }

    /// The format matching a raw report, plus the payload slice.
    ///
    /// Hot path: a linear scan over at most a handful of formats, no allocation.
    pub fn match_report<'a>(&self, report: &'a [u8]) -> Option<(&ReportFormat, &'a [u8])> {
        if self.uses_report_ids {
            let (&id, payload) = report.split_first()?;
            let format = self.reports.iter().find(|r| r.report_id == id)?;
            Some((format, payload))
        } else {
            let format = self.reports.first()?;
            Some((format, report))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.reports.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Item decoding
// ---------------------------------------------------------------------------

const ITEM_TYPE_MAIN: u8 = 0;
const ITEM_TYPE_GLOBAL: u8 = 1;
const ITEM_TYPE_LOCAL: u8 = 2;

const MAIN_INPUT: u8 = 0x8;

const GLOBAL_USAGE_PAGE: u8 = 0x0;
const GLOBAL_REPORT_SIZE: u8 = 0x7;
const GLOBAL_REPORT_ID: u8 = 0x8;
const GLOBAL_REPORT_COUNT: u8 = 0x9;
const GLOBAL_PUSH: u8 = 0xA;
const GLOBAL_POP: u8 = 0xB;

const LOCAL_USAGE: u8 = 0x0;
const LOCAL_USAGE_MIN: u8 = 0x1;
const LOCAL_USAGE_MAX: u8 = 0x2;

/// `Input` item flags we care about.
const INPUT_CONSTANT: u32 = 1 << 0;
const INPUT_VARIABLE: u32 = 1 << 1;

/// The long-item prefix (HID 1.11 §6.2.2.3). Skipped by declared length.
const LONG_ITEM_PREFIX: u8 = 0xFE;

/// Largest report payload `parse` will honor, in bytes.
///
/// One USB interrupt-IN transfer carries at most 1024 bytes (the high-speed
/// endpoint ceiling; full speed is 64), and HID input reports arrive in one
/// transfer — so no keyboard can deliver a longer report no matter what its
/// descriptor claims. Real keyboard payloads are ≤ 32 bytes; the headroom is
/// for exotic NKRO layouts, not for trusting the descriptor.
const MAX_PAYLOAD_LEN: usize = 1024;

/// Global item state, saved/restored by Push/Pop.
#[derive(Clone, Copy, Debug, Default)]
struct GlobalState {
    usage_page: u16,
    report_size: u32,
    report_count: u32,
    report_id: u8,
}

/// Parse a HID report descriptor into the keyboard fields it declares.
///
/// Returns an empty [`KeyboardFormat`] for a descriptor that is malformed or
/// simply has no Keyboard/Keypad input fields — the caller treats both the same
/// way (refuse to claim).
pub fn parse(descriptor: &[u8]) -> KeyboardFormat {
    let mut global = GlobalState::default();
    let mut stack: Vec<GlobalState> = Vec::new();
    // Local items are cleared after every Main item (HID 1.11 §6.2.2.8).
    let mut usage_min: Option<u32> = None;
    let mut usages: Vec<u32> = Vec::new();

    // Input-report bit cursor, per report ID. Output/Feature have their own
    // cursors in the spec; we never read them, so they are simply not tracked
    // (and their items must NOT advance the input cursor — hence the match on
    // MAIN_INPUT only).
    let mut cursors: Vec<(u8, usize)> = Vec::new();
    let mut formats: Vec<ReportFormat> = Vec::new();
    let mut saw_report_id = false;

    let mut i = 0usize;
    while i < descriptor.len() {
        let prefix = descriptor[i];
        if prefix == LONG_ITEM_PREFIX {
            // bDataSize, bLongItemTag, then data.
            let Some(&size) = descriptor.get(i + 1) else {
                break;
            };
            i += 3 + size as usize;
            continue;
        }
        let size = match prefix & 0x03 {
            3 => 4,
            n => n as usize,
        };
        let tag = prefix >> 4;
        let ty = (prefix >> 2) & 0x03;
        let start = i + 1;
        let end = start + size;
        if end > descriptor.len() {
            break; // truncated descriptor: keep what we have
        }
        let data = unsigned(&descriptor[start..end]);
        i = end;

        match ty {
            ITEM_TYPE_GLOBAL => match tag {
                GLOBAL_USAGE_PAGE => global.usage_page = data as u16,
                GLOBAL_REPORT_SIZE => global.report_size = data,
                GLOBAL_REPORT_COUNT => global.report_count = data,
                GLOBAL_REPORT_ID => {
                    global.report_id = data as u8;
                    saw_report_id = true;
                }
                GLOBAL_PUSH => stack.push(global),
                GLOBAL_POP => {
                    if let Some(prev) = stack.pop() {
                        global = prev;
                    }
                }
                _ => {} // NOTE: no local-scope clear here. HID 1.11 §6.2.2.8 is
                        // explicit that **only Main items** flush the local table, and
                        // a keyboard descriptor relies on it: `Usage Min/Max` for the
                        // modifiers is declared *before* `Logical Min/Max`, `Report
                        // Size` and `Report Count`, all of which are Global. Clearing
                        // on Global loses the usage range by the time the `Input` item
                        // arrives, and the modifier bitmap silently disappears — which
                        // is exactly how a captured board delivers letters but no
                        // Ctrl/Alt/Shift, and therefore no escape gesture.
            },
            ITEM_TYPE_LOCAL => {
                match tag {
                    LOCAL_USAGE => usages.push(data),
                    LOCAL_USAGE_MIN => usage_min = Some(data),
                    // Usage Maximum is parsed and dropped: the field's extent
                    // comes from Report Size x Report Count, which is what the
                    // wire actually carries. A descriptor whose usage range
                    // disagrees with its bit count is the device's bug, and
                    // trusting the bit count is what keeps later fields at the
                    // right offset.
                    LOCAL_USAGE_MAX => {}
                    _ => {}
                }
                continue; // only a Main item ends the local scope
            }
            ITEM_TYPE_MAIN => {
                if tag == MAIN_INPUT {
                    let bits =
                        (global.report_size as usize).saturating_mul(global.report_count as usize);
                    let cursor = cursor_for(&mut cursors, global.report_id);
                    let bit_offset = *cursor;
                    // Saturating like the multiply above: a hostile descriptor
                    // can declare ~2^64 bits per Input item, and a wrapping
                    // cursor is a debug-build panic / release-build phantom
                    // field offset (found by ksx-fuzz).
                    *cursor = bit_offset.saturating_add(bits);

                    if global.usage_page == PAGE_KEYBOARD
                        && data & INPUT_CONSTANT == 0
                        && bits > 0
                        && global.report_size > 0
                    {
                        let field = if data & INPUT_VARIABLE != 0 {
                            // Variable: one bit (or field) per usage. Only the
                            // 1-bit form is a keyboard bitmap; a wider variable
                            // field on the keyboard page is not something a
                            // keyboard emits, and guessing at it is exactly the
                            // kind of invention that breaks a captured board.
                            (global.report_size == 1).then(|| {
                                let base =
                                    usage_min.or_else(|| usages.first().copied()).unwrap_or(0);
                                Field::Bitmap {
                                    bit_offset,
                                    bits,
                                    usage_min: clamp_usage(base),
                                }
                            })
                        } else {
                            Some(Field::Array {
                                bit_offset,
                                bits: global.report_size as usize,
                                count: global.report_count as usize,
                            })
                        };
                        if let Some(field) = field {
                            push_field(&mut formats, global.report_id, field);
                        }
                    }
                    note_len(&mut formats, global.report_id, *cursor);
                }
                // Collection/EndCollection/Output/Feature need nothing else.
                // (Output/Feature deliberately do NOT advance the input
                // cursor.) But every Main item — including those — flushes the
                // local table, per HID 1.11 §6.2.2.8.
                usage_min = None;
                usages.clear();
            }
            _ => {}
        }
    }

    // Drop reports that ended up with no keyboard field (e.g. a mouse report ID
    // in the same descriptor) but keep their length knowledge out of the way.
    // Also drop any report declared longer than one USB interrupt transfer:
    // no real keyboard can send it (an interrupt-IN transfer caps at 1024
    // bytes even at high speed), and honoring it is a device-supplied DoS —
    // `max_report_len` sizes the claim-time buffer and `feed` walks the
    // declared bits, so a 13-byte descriptor declaring a 2^32-bit bitmap costs
    // a ~512 MiB allocation plus seconds *per report* (found by ksx-fuzz).
    // Dropping the report (not clamping it) keeps the "malformed → refuse to
    // claim" contract: a device that lies about its reports is not captured.
    formats.retain(|f| f.is_keyboard() && f.payload_len <= MAX_PAYLOAD_LEN);
    // A descriptor that declares report IDs prefixes every report with the ID.
    KeyboardFormat {
        reports: formats,
        uses_report_ids: saw_report_id,
    }
}

/// Usages above 0xFF cannot occur on page 0x07 (last defined: 0xE7). Clamping
/// keeps the bitmap arithmetic in `u8` without a silent wrap.
fn clamp_usage(usage: u32) -> u8 {
    u8::try_from(usage).unwrap_or(u8::MAX)
}

fn cursor_for(cursors: &mut Vec<(u8, usize)>, report_id: u8) -> &mut usize {
    if let Some(idx) = cursors.iter().position(|(id, _)| *id == report_id) {
        return &mut cursors[idx].1;
    }
    cursors.push((report_id, 0));
    &mut cursors.last_mut().expect("just pushed").1
}

fn format_for(formats: &mut Vec<ReportFormat>, report_id: u8) -> &mut ReportFormat {
    if let Some(idx) = formats.iter().position(|f| f.report_id == report_id) {
        return &mut formats[idx];
    }
    formats.push(ReportFormat {
        report_id,
        payload_len: 0,
        fields: Vec::new(),
    });
    formats.last_mut().expect("just pushed")
}

fn push_field(formats: &mut Vec<ReportFormat>, report_id: u8, field: Field) {
    format_for(formats, report_id).fields.push(field);
}

fn note_len(formats: &mut Vec<ReportFormat>, report_id: u8, bits: usize) {
    let format = format_for(formats, report_id);
    format.payload_len = format.payload_len.max(bits.div_ceil(8));
}

/// Item data is little-endian and unsigned for every tag this parser reads
/// (`Logical Minimum` would need sign extension; we never consult it).
fn unsigned(bytes: &[u8]) -> u32 {
    let mut value = 0u32;
    for (i, &b) in bytes.iter().take(4).enumerate() {
        value |= u32::from(b) << (8 * i);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::fixtures;

    #[test]
    fn boot_keyboard_descriptor_is_modifiers_plus_six_slots() {
        let format = parse(fixtures::BOOT_KEYBOARD_DESCRIPTOR);
        assert!(!format.uses_report_ids);
        assert_eq!(format.reports.len(), 1);
        let report = &format.reports[0];
        assert_eq!(report.report_id, 0);
        assert_eq!(report.payload_len, 8, "the classic 8-byte boot report");
        assert_eq!(
            report.fields,
            vec![
                // Byte 0, bits 0..8: modifiers 0xE0..=0xE7.
                Field::Bitmap {
                    bit_offset: 0,
                    bits: 8,
                    usage_min: 0xE0
                },
                // Byte 1 is the constant reserved byte — not a field, but it
                // still moved the cursor: the array starts at bit 16.
                Field::Array {
                    bit_offset: 16,
                    bits: 8,
                    count: 6
                },
            ]
        );
        assert_eq!(format.max_report_len(), 8);
    }

    #[test]
    fn led_output_items_do_not_shift_the_input_cursor() {
        // The boot descriptor puts 8 bits of LED Output between the modifier
        // bitmap and the key array. If Output advanced the input cursor the
        // array would land at bit 24 and every key would be read from the wrong
        // byte — the exact bug this test exists to catch.
        let format = parse(fixtures::BOOT_KEYBOARD_DESCRIPTOR);
        let Field::Array { bit_offset, .. } = format.reports[0].fields[1] else {
            panic!("expected an array field");
        };
        assert_eq!(bit_offset, 16);
    }

    #[test]
    fn nkro_bitmap_descriptor_covers_the_whole_page() {
        let format = parse(fixtures::NKRO_DESCRIPTOR);
        assert!(format.uses_report_ids);
        assert_eq!(format.reports.len(), 1);
        let report = &format.reports[0];
        assert_eq!(report.report_id, fixtures::NKRO_REPORT_ID);
        assert_eq!(
            report.fields,
            vec![
                Field::Bitmap {
                    bit_offset: 0,
                    bits: 8,
                    usage_min: 0xE0
                },
                Field::Bitmap {
                    bit_offset: 8,
                    bits: 240,
                    usage_min: 0x00
                },
            ]
        );
        assert_eq!(report.payload_len, 31);
        assert_eq!(format.max_report_len(), 32, "payload + the report-ID byte");
    }

    #[test]
    fn composite_descriptor_keeps_only_the_keyboard_report() {
        // A board that also declares a consumer-control report on the same
        // interface: the non-keyboard report must not appear, and must not
        // disturb the keyboard report's bit offsets.
        let format = parse(fixtures::KEYBOARD_PLUS_CONSUMER_DESCRIPTOR);
        assert!(format.uses_report_ids);
        assert_eq!(format.reports.len(), 1);
        assert_eq!(format.reports[0].report_id, 1);
        assert_eq!(
            format.reports[0].fields[0],
            Field::Bitmap {
                bit_offset: 0,
                bits: 8,
                usage_min: 0xE0
            }
        );
    }

    #[test]
    fn report_ids_select_the_right_format_and_strip_the_prefix() {
        let format = parse(fixtures::NKRO_DESCRIPTOR);
        let mut report = vec![fixtures::NKRO_REPORT_ID];
        report.extend(std::iter::repeat_n(0u8, 31));
        report[1] = 0b0000_0001; // LeftControl bit
        let (matched, payload) = format.match_report(&report).expect("format matches");
        assert_eq!(matched.report_id, fixtures::NKRO_REPORT_ID);
        assert_eq!(payload.len(), 31);
        assert_eq!(payload[0], 0b0000_0001);

        // A report for an ID we do not know is not ours.
        report[0] = 0x77;
        assert!(format.match_report(&report).is_none());
    }

    #[test]
    fn push_and_pop_restore_the_usage_page() {
        let format = parse(fixtures::PUSH_POP_DESCRIPTOR);
        assert_eq!(format.reports.len(), 1);
        assert_eq!(
            format.reports[0].fields,
            vec![Field::Bitmap {
                bit_offset: 8,
                bits: 8,
                usage_min: 0xE0
            }],
            "the 8 bits declared while the page was pushed to Button must be \
             skipped, but must still advance the cursor"
        );
    }

    #[test]
    fn malformed_descriptors_never_panic_and_never_invent_a_layout() {
        // Truncated item data.
        assert!(parse(&[0x05]).is_empty());
        assert!(parse(&[0x05, 0x07, 0x81]).is_empty());
        // A long item that runs off the end.
        assert!(parse(&[0xFE, 0x40, 0x00]).is_empty());
        // Pure noise.
        assert!(parse(&[0xFF; 64]).is_empty());
        // Empty.
        assert!(parse(&[]).is_empty());
        // A mouse: valid, parseable, and not a keyboard.
        assert!(parse(fixtures::MOUSE_DESCRIPTOR).is_empty());
    }

    #[test]
    fn pop_without_push_is_survivable() {
        assert!(parse(&[0xB4, 0xB4, 0xB4]).is_empty());
    }

    #[test]
    fn cursor_overflow_from_giant_declared_fields_is_saturated_not_a_panic() {
        // Two Input items, each declaring Report Size × Report Count =
        // (2^32-1)^2 bits. The second cursor advance used to overflow usize —
        // a debug-build panic (silent wrap in release) on a device-supplied
        // descriptor. Found by ksx-fuzz (tests/hid_report.rs).
        let desc = [
            0x77, 0xFF, 0xFF, 0xFF, 0xFF, // Report Size (0xFFFFFFFF)
            0x97, 0xFF, 0xFF, 0xFF, 0xFF, // Report Count (0xFFFFFFFF)
            0x81, 0x01, // Input (Const) — cursor += ~2^64 bits
            0x81, 0x01, // Input (Const) — the second advance overflowed
        ];
        assert!(parse(&desc).is_empty());
    }

    #[test]
    fn wide_variable_fields_on_the_keyboard_page_are_ignored() {
        // Report size 8, variable, keyboard page: not a bitmap and not an
        // array. Inventing a meaning for it is how a stuck modifier happens.
        let desc = [
            0x05, 0x07, // Usage Page (Keyboard)
            0x19, 0x00, // Usage Min (0)
            0x29, 0xFF, // Usage Max (255)
            0x75, 0x08, // Report Size (8)
            0x95, 0x02, // Report Count (2)
            0x81, 0x02, // Input (Data, Var, Abs)
        ];
        assert!(parse(&desc).is_empty());
    }
}
