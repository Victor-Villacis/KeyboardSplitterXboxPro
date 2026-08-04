//! ksx-legacy-import — reads the legacy app's UTF-16 XML and emits ksx TOML.
//!
//! Import fidelity is bit-exact against the legacy ID tables
//! (`legacy/VirtualXbox/Enums/*.cs`): XboxButton 0x0010..0x8000, triggers
//! 0x10000/0x20000, axes 1/2/4/8 with signed 16-bit values, dpad flags, and the
//! flat XboxCustomFunction space. Also honors the v1-schema upgrader semantics
//! (`LeftTrigger`→`Left`, `<pov>`→dpad) from `legacy/KeyboardSplitter/Presets/`.
//!
//! Golden-file tests run against the real cabinet corpus in `tests/fixtures/`.
//! XML is import-only; no XML writer will ever exist here. Implemented in M1.
