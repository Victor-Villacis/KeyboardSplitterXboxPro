//! The feedback decode table — rumble/FFB coming back from the OS.
//!
//! Transcribed from `padforge-code-audit.md` §3.4, whose Sony half was itself
//! cross-checked against Linux's `hid-playstation.c`. Every rule here is
//! **written to spec and unverified on hardware**; the tests below pin the table
//! so a live capture can contradict it precisely rather than vaguely.
//!
//! One decision governs the whole module: **a packet that fails its gate
//! preserves the previous motor state; it never zeroes it.** A lightbar-only
//! Sony report means "I have nothing to say about rumble", not "stop rumbling",
//! and a decoder that reads it as the latter turns every LED change into a
//! stutter. [`Decoded::Preserve`] is that answer, and it is deliberately not
//! representable as "motors = 0".

/// Which lane a packet arrived on. HIDMaestro reports this alongside the bytes,
/// and it disambiguates layouts that would otherwise collide on length.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum OutputSource {
    /// An `XUSB`-style vibration IOCTL, forwarded by the GIP companion.
    XInput,
    /// A HID output report written to the device.
    HidOutput,
    /// A HID `SetFeature`.
    HidFeature,
}

/// Motor magnitudes, in the 16-bit domain XInput and SDL both use.
///
/// u16 rather than u8 because two of the four sources carry more than 8 bits of
/// range (XInput's IOCTL sends the high byte of a u16; the Xbox Series BT form
/// sends 0..100 scaled by 655). Narrowing happens at the ksx boundary, once.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub struct Motors {
    /// Low-frequency (left) motor.
    pub large: u16,
    /// High-frequency (right) motor.
    pub small: u16,
    /// Left impulse trigger (Xbox One+ only; 0 elsewhere).
    pub left_trigger: u16,
    /// Right impulse trigger.
    pub right_trigger: u16,
}

impl Motors {
    /// Narrow to the 8-bit shape `ksx_output::Feedback` carries.
    pub const fn as_u8(self) -> (u8, u8) {
        ((self.large >> 8) as u8, (self.small >> 8) as u8)
    }
}

/// Result of decoding one packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decoded {
    /// New motor state, to be applied.
    Motors(Motors),
    /// The packet was a valid feedback packet whose gate failed (wrong size,
    /// bad CRC, or `validFlag0` not asserting the motor bits). **Keep the
    /// previous motors.**
    Preserve,
    /// Not a feedback packet at all (wrong header, too short to be anything).
    Ignored,
}

/// Scales the Xbox Series BT motor range (0..=100) into the 16-bit domain.
/// The audit's constant is ×655; 100 × 655 = 65500, i.e. full scale within
/// rounding, and the multiply cannot overflow a u16 for in-range input.
const XBOX_BT_MOTOR_SCALE: u16 = 655;

/// Decodes an [`OutputSource::XInput`] vibration IOCTL.
///
/// Layout: `[00, 08, leftHi, rightHi, reserved]`, i.e. the *high bytes* of the
/// two u16 magnitudes. 7+ bytes is `XINPUT_VIBRATION_EX`, which appends impulse
/// trigger magnitudes at offsets 4 and 5. A 5-byte packet explicitly **zeroes**
/// the trigger motors (it is the short form saying "no impulse data"), which is
/// different from preserving them.
///
/// Zero magnitudes are a legitimate value and are never filtered: Chromium
/// drives dual-rumble as a `hi=127 / hi=0` square wave, so dropping the zero
/// half would leave the pad buzzing continuously.
pub fn decode_xinput(bytes: &[u8]) -> Decoded {
    if bytes.len() < 5 || bytes[0] != 0x00 || bytes[1] != 0x08 {
        return Decoded::Ignored;
    }
    let mut motors = Motors {
        large: u16::from(bytes[2]) << 8,
        small: u16::from(bytes[3]) << 8,
        ..Motors::default()
    };
    // 6-byte packets carry no impulse fields either; only the documented 7+
    // form does. Treat anything shorter as "triggers zeroed", as the 5-byte
    // form does.
    if bytes.len() >= 7 {
        motors.left_trigger = u16::from(bytes[4]) << 8;
        motors.right_trigger = u16::from(bytes[5]) << 8;
    }
    Decoded::Motors(motors)
}

/// Decodes an [`OutputSource::HidOutput`] packet from an Xbox-family device.
///
/// Two forms, disambiguated by length:
/// - **len 4..=7** — the Xbox Series *Bluetooth* short form,
///   `[trigL, trigR, motorL, motorR, dur, delay, loop]`, motors 0..=100. It is
///   SDL's XboxOne rumble payload minus SDL's 2-byte header.
/// - **len >= 8** — the legacy Xbox HID form, motors at bytes 5 and 6.
pub fn decode_xbox_hid(bytes: &[u8]) -> Decoded {
    match bytes.len() {
        0..=3 => Decoded::Ignored,
        4..=7 => {
            // Out-of-range magnitudes are clamped rather than rejected: a
            // consumer that sends 255 means "as hard as you can", and dropping
            // the packet would mean "not at all".
            let scale = |v: u8| u16::from(v.min(100)) * XBOX_BT_MOTOR_SCALE;
            Decoded::Motors(Motors {
                left_trigger: scale(bytes[0]),
                right_trigger: scale(bytes[1]),
                large: scale(bytes[2]),
                small: scale(bytes[3]),
            })
        }
        _ => Decoded::Motors(Motors {
            large: u16::from(bytes[5]) << 8,
            small: u16::from(bytes[6]) << 8,
            ..Motors::default()
        }),
    }
}

/// Which Sony pad a report came from — it selects the offsets and the
/// `validFlag0` mask.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum SonyFamily {
    /// DualShock 4. USB output report 0x05: `validFlag0` at 1, motors at 4
    /// (right/small) and 5 (left/large).
    Ds4,
    /// DualSense. USB output report 0x02: `validFlag0` at 1, motors at 2
    /// (right/small) and 3 (left/large).
    Ds5,
}

impl SonyFamily {
    /// `validFlag0` bits that must be asserted for the motor fields to mean
    /// anything. DS4: 0x01. DS5: 0x03 (compatible-vibration | haptics-select).
    pub const fn motor_mask(self) -> u8 {
        match self {
            SonyFamily::Ds4 => 0x01,
            SonyFamily::Ds5 => 0x03,
        }
    }

    const fn valid_flag0_offset(self) -> usize {
        1
    }

    /// (small/right, large/left) motor byte offsets in the USB report form.
    const fn motor_offsets(self) -> (usize, usize) {
        match self {
            SonyFamily::Ds4 => (4, 5),
            SonyFamily::Ds5 => (2, 3),
        }
    }

    /// Minimum bytes a report must actually contain for the motor fields to be
    /// present.
    pub const fn required_len(self) -> usize {
        let (small, large) = self.motor_offsets();
        if small > large {
            small + 1
        } else {
            large + 1
        }
    }
}

/// A Sony output report as HIDMaestro hands it over.
///
/// `bytes` is the **USB form**: the driver pre-strips Bluetooth framing and CRC
/// before raising `OutputDecoded`, which is why the offsets above are
/// transport-independent. `declared_len` and `crc_ok` are the transport facts
/// the gate still needs.
#[derive(Clone, Copy, Debug)]
pub struct SonyReport<'a> {
    pub family: SonyFamily,
    pub bytes: &'a [u8],
    /// The largest output report size the descriptor declares. On Bluetooth
    /// this is **547** for a DualSense — see [`sony_motors_present`].
    pub declared_len: usize,
    /// Whether the transport CRC verified. USB reports carry none; pass `true`.
    pub crc_ok: bool,
}

/// The length half of the Sony gate — **`>=`, never `==`**.
///
/// **The Bluetooth length trap.** Windows sizes a Bluetooth HID host write to
/// the *largest declared output report*, which for a DualSense is 547 bytes.
/// That write is then clamped to the driver's 256-byte report slot, so what
/// actually arrives is ~257 bytes (report id + slot), and an
/// `actual == declared` check is false for **every single Bluetooth report**.
/// PadForge shipped that equality and Sony rumble silently never worked over
/// BT. The correct question is "are the motor bytes present?", which is
/// `actual >= required`, where `required` comes from the report layout
/// ([`SonyFamily::required_len`]) and not from the descriptor's declaration.
pub fn sony_motors_present(actual_len: usize, required_len: usize) -> bool {
    actual_len >= required_len
}

/// Decodes a Sony output report through the full trust gate.
///
/// Gate order (cheapest first, and each failure is [`Decoded::Preserve`], not a
/// zeroing): length → CRC → `validFlag0`.
pub fn decode_sony(report: &SonyReport<'_>) -> Decoded {
    let family = report.family;
    let required = family.required_len();
    if report.bytes.len() <= family.valid_flag0_offset() {
        // Too short to even hold the flag byte: this is not a report we can
        // reason about.
        return Decoded::Ignored;
    }
    if !sony_motors_present(report.bytes.len(), required) {
        return Decoded::Preserve;
    }
    if !report.crc_ok {
        return Decoded::Preserve;
    }
    let flags = report.bytes[family.valid_flag0_offset()];
    if flags & family.motor_mask() == 0 {
        // A lightbar-only or haptics-only report. "Ignore", not "stop".
        return Decoded::Preserve;
    }
    let (small_at, large_at) = family.motor_offsets();
    Decoded::Motors(Motors {
        large: u16::from(report.bytes[large_at]) << 8,
        small: u16::from(report.bytes[small_at]) << 8,
        ..Motors::default()
    })
}

/// A HID-PID effect, reduced to what a rumble consumer needs: a magnitude and
/// **when it expires**.
///
/// The reason this carries a deadline at all is PadForge's stuck-rumble bug
/// (Jedi Outcast): PID effect durations expire on the *device* clock, so a game
/// that sets a 200 ms constant-force effect and then goes quiet leaves the last
/// vibration latched forever unless something sweeps expiries per tick. See
/// [`PidEffects::apply_if_due`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PidEffect {
    pub magnitude: u16,
    /// Effect duration in milliseconds; `None` = infinite (until Block Free or
    /// Device Control stop).
    pub duration_ms: Option<u32>,
    /// Milliseconds since the effect started, accumulated by the per-tick pass.
    pub elapsed_ms: u32,
    pub running: bool,
}

impl PidEffect {
    pub fn expired(&self) -> bool {
        match self.duration_ms {
            Some(d) => self.elapsed_ms >= d,
            None => false,
        }
    }
}

/// The PID effect pool for one device.
///
/// Fixed capacity: the pool must be published to the driver **before the device
/// enumerates** (DirectInput issues `GetFeature(PidPool)` during `CreateEffect`,
/// so lazy init on first output packet is too late — see
/// [`crate::context::HmContext::create_controller`]), which means its size is a
/// property of the profile, not of runtime demand.
#[derive(Clone, Copy, Debug)]
pub struct PidEffects<const N: usize> {
    effects: [PidEffect; N],
}

impl<const N: usize> Default for PidEffects<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> PidEffects<N> {
    pub const fn new() -> Self {
        Self {
            effects: [PidEffect {
                magnitude: 0,
                duration_ms: None,
                elapsed_ms: 0,
                running: false,
            }; N],
        }
    }

    pub fn set(&mut self, index: usize, effect: PidEffect) -> bool {
        match self.effects.get_mut(index) {
            Some(slot) => {
                *slot = effect;
                true
            }
            None => false,
        }
    }

    pub fn get(&self, index: usize) -> Option<&PidEffect> {
        self.effects.get(index)
    }

    /// The per-tick pass. Ages every running effect by `elapsed_ms`, stops the
    /// ones that expired, and returns the resulting magnitude (max over live
    /// effects).
    ///
    /// Must be called every tick even when no packet arrived — that is the whole
    /// point. Without it the last effect latches forever.
    pub fn apply_if_due(&mut self, elapsed_ms: u32) -> u16 {
        let mut magnitude = 0u16;
        for effect in &mut self.effects {
            if !effect.running {
                continue;
            }
            effect.elapsed_ms = effect.elapsed_ms.saturating_add(elapsed_ms);
            if effect.expired() {
                effect.running = false;
                effect.magnitude = 0;
                continue;
            }
            magnitude = magnitude.max(effect.magnitude);
        }
        magnitude
    }

    /// HID-PID "Device Control: stop all" / "Block Free".
    pub fn free_all(&mut self) {
        self.effects = Self::new().effects;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xinput_ioctl_five_byte_form_zeroes_the_trigger_motors() {
        let d = decode_xinput(&[0x00, 0x08, 0x7F, 0xFF, 0x00]);
        assert_eq!(
            d,
            Decoded::Motors(Motors {
                large: 0x7F00,
                small: 0xFF00,
                left_trigger: 0,
                right_trigger: 0,
            })
        );
    }

    #[test]
    fn xinput_ioctl_seven_byte_form_carries_impulse_triggers() {
        let d = decode_xinput(&[0x00, 0x08, 0x10, 0x20, 0x30, 0x40, 0x00]);
        assert_eq!(
            d,
            Decoded::Motors(Motors {
                large: 0x1000,
                small: 0x2000,
                left_trigger: 0x3000,
                right_trigger: 0x4000,
            })
        );
    }

    #[test]
    fn xinput_zero_magnitudes_are_data_not_noise() {
        // Chromium's dual-rumble square wave is hi=127 then hi=0. Filtering the
        // zero half leaves the pad buzzing forever.
        let d = decode_xinput(&[0x00, 0x08, 0x00, 0x00, 0x00]);
        assert_eq!(d, Decoded::Motors(Motors::default()));
        assert_ne!(d, Decoded::Ignored);
        assert_ne!(d, Decoded::Preserve);
    }

    #[test]
    fn a_non_vibration_ioctl_is_ignored() {
        assert_eq!(decode_xinput(&[0x00, 0x09, 1, 2, 3]), Decoded::Ignored);
        assert_eq!(decode_xinput(&[0x00, 0x08, 1, 2]), Decoded::Ignored);
        assert_eq!(decode_xinput(&[]), Decoded::Ignored);
    }

    #[test]
    fn xbox_series_bt_short_form_scales_0_to_100_by_655() {
        // [trigL, trigR, motorL, motorR, dur, delay, loop]
        let d = decode_xbox_hid(&[0, 0, 100, 50, 0xFF, 0, 0]);
        assert_eq!(
            d,
            Decoded::Motors(Motors {
                large: 100 * 655,
                small: 50 * 655,
                left_trigger: 0,
                right_trigger: 0,
            })
        );
        // Full scale is ~65500, i.e. u16 range within rounding.
        match d {
            Decoded::Motors(m) => assert!(m.large > 65_000),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn xbox_legacy_hid_reads_motors_at_bytes_five_and_six() {
        let d = decode_xbox_hid(&[0, 0, 0, 0, 0, 0xAA, 0xBB, 0]);
        assert_eq!(
            d,
            Decoded::Motors(Motors {
                large: 0xAA00,
                small: 0xBB00,
                ..Motors::default()
            })
        );
    }

    #[test]
    fn xbox_hid_length_selects_the_form() {
        // The boundary the two layouts meet at: 7 is BT short, 8 is legacy.
        let bt = decode_xbox_hid(&[10, 20, 30, 40, 0, 0, 0]);
        let legacy = decode_xbox_hid(&[10, 20, 30, 40, 0, 0, 0, 0]);
        assert_ne!(bt, legacy);
        match bt {
            Decoded::Motors(m) => assert_eq!(m.large, 30 * 655),
            other => panic!("{other:?}"),
        }
        match legacy {
            Decoded::Motors(m) => assert_eq!(m.large, 0),
            other => panic!("{other:?}"),
        }
        assert_eq!(decode_xbox_hid(&[1, 2, 3]), Decoded::Ignored);
    }

    fn ds5(bytes: &[u8], declared: usize) -> SonyReport<'_> {
        SonyReport {
            family: SonyFamily::Ds5,
            bytes,
            declared_len: declared,
            crc_ok: true,
        }
    }

    /// **The BT length trap**, as a test: a DualSense declares 547 bytes, the
    /// clamped host write delivers 257, and the motors are right there. An
    /// `actual == declared` gate rejects every Bluetooth report forever.
    #[test]
    fn the_bluetooth_547_to_257_length_trap() {
        const DECLARED_BT: usize = 547;
        const DELIVERED: usize = 257; // driver's 256-byte slot + report id
        let mut bytes = vec![0u8; DELIVERED];
        bytes[0] = 0x02;
        bytes[1] = SonyFamily::Ds5.motor_mask();
        bytes[2] = 0x40; // small/right
        bytes[3] = 0x80; // large/left

        // The correct gate accepts it.
        assert!(sony_motors_present(
            DELIVERED,
            SonyFamily::Ds5.required_len()
        ));
        assert_eq!(
            decode_sony(&ds5(&bytes, DECLARED_BT)),
            Decoded::Motors(Motors {
                large: 0x8000,
                small: 0x4000,
                ..Motors::default()
            })
        );

        // The gate PadForge shipped first would have rejected it — stated
        // explicitly so nobody "simplifies" the >= back into an ==.
        assert_ne!(DELIVERED, DECLARED_BT);
        assert!(
            DELIVERED != DECLARED_BT
                && sony_motors_present(DELIVERED, SonyFamily::Ds5.required_len()),
            "equality would fail where >= succeeds — that is the whole trap"
        );
        // And 256 (no report id) must pass too.
        assert!(sony_motors_present(256, SonyFamily::Ds5.required_len()));
    }

    #[test]
    fn a_truly_short_report_preserves_rather_than_zeroes() {
        // Shorter than the motor offsets: we do not know the motors, so we keep
        // whatever was already running.
        let bytes = [0x02u8, SonyFamily::Ds5.motor_mask(), 0x11];
        assert_eq!(decode_sony(&ds5(&bytes, 78)), Decoded::Preserve);
        assert_eq!(SonyFamily::Ds5.required_len(), 4);
        assert_eq!(SonyFamily::Ds4.required_len(), 6);
    }

    #[test]
    fn validflag0_gates_the_motors_and_a_lightbar_report_preserves() {
        let mut bytes = vec![0u8; 16];
        bytes[0] = 0x02;
        bytes[2] = 0x40;
        bytes[3] = 0x80;
        // validFlag0 = 0x04 (some non-motor bit, e.g. a lighting flag).
        bytes[1] = 0x04;
        assert_eq!(
            decode_sony(&ds5(&bytes, 78)),
            Decoded::Preserve,
            "a lightbar-only report means 'ignore', not 'stop'"
        );
        // Assert the motor bit and it decodes.
        bytes[1] = 0x04 | 0x01;
        assert!(matches!(decode_sony(&ds5(&bytes, 78)), Decoded::Motors(_)));
    }

    #[test]
    fn a_bad_crc_preserves() {
        let mut bytes = vec![0u8; 16];
        bytes[1] = SonyFamily::Ds5.motor_mask();
        bytes[3] = 0xFF;
        let mut report = ds5(&bytes, 547);
        report.crc_ok = false;
        assert_eq!(decode_sony(&report), Decoded::Preserve);
    }

    #[test]
    fn ds4_and_ds5_read_different_offsets_and_different_masks() {
        assert_eq!(SonyFamily::Ds4.motor_mask(), 0x01);
        assert_eq!(SonyFamily::Ds5.motor_mask(), 0x03);
        let mut bytes = vec![0u8; 16];
        bytes[1] = 0x01;
        bytes[2] = 0x11;
        bytes[3] = 0x22;
        bytes[4] = 0x33;
        bytes[5] = 0x44;
        let as_ds5 = decode_sony(&SonyReport {
            family: SonyFamily::Ds5,
            bytes: &bytes,
            declared_len: 78,
            crc_ok: true,
        });
        let as_ds4 = decode_sony(&SonyReport {
            family: SonyFamily::Ds4,
            bytes: &bytes,
            declared_len: 78,
            crc_ok: true,
        });
        assert_eq!(
            as_ds5,
            Decoded::Motors(Motors {
                small: 0x1100,
                large: 0x2200,
                ..Motors::default()
            })
        );
        assert_eq!(
            as_ds4,
            Decoded::Motors(Motors {
                small: 0x3300,
                large: 0x4400,
                ..Motors::default()
            })
        );
    }

    #[test]
    fn motors_narrow_to_the_ksx_feedback_shape() {
        let m = Motors {
            large: 0xABCD,
            small: 0x1234,
            ..Motors::default()
        };
        assert_eq!(m.as_u8(), (0xAB, 0x12));
    }

    /// The Jedi Outcast bug: without a per-tick expiry sweep the last effect
    /// latches forever once the game stops sending packets.
    #[test]
    fn pid_effects_expire_on_the_tick_pass_not_on_the_next_packet() {
        let mut pool = PidEffects::<8>::new();
        assert!(pool.set(
            0,
            PidEffect {
                magnitude: 40_000,
                duration_ms: Some(200),
                elapsed_ms: 0,
                running: true,
            }
        ));
        // 199 ms of silence: still rumbling.
        assert_eq!(pool.apply_if_due(199), 40_000);
        // One more tick and it must stop by itself, with no packet involved.
        assert_eq!(pool.apply_if_due(1), 0);
        assert!(!pool.get(0).unwrap().running);
        // Repeated ticks stay at zero.
        assert_eq!(pool.apply_if_due(1000), 0);
    }

    #[test]
    fn an_infinite_pid_effect_survives_the_sweep_until_freed() {
        let mut pool = PidEffects::<4>::new();
        pool.set(
            1,
            PidEffect {
                magnitude: 1000,
                duration_ms: None,
                elapsed_ms: 0,
                running: true,
            },
        );
        assert_eq!(pool.apply_if_due(10_000), 1000);
        pool.free_all();
        assert_eq!(pool.apply_if_due(1), 0);
    }

    #[test]
    fn the_pool_reports_the_strongest_live_effect() {
        let mut pool = PidEffects::<4>::new();
        for (i, mag) in [100u16, 5000, 300].into_iter().enumerate() {
            pool.set(
                i,
                PidEffect {
                    magnitude: mag,
                    duration_ms: Some(50),
                    elapsed_ms: 0,
                    running: true,
                },
            );
        }
        assert_eq!(pool.apply_if_due(10), 5000);
        assert!(!pool.set(99, PidEffect::default()), "pool is fixed size");
    }
}
