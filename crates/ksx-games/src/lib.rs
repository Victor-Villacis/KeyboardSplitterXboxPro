//! ksx-games — game/profile registry and launch orchestration.
//!
//! Successor to the legacy games list + `game=` CLI autostart: profiles bind slot
//! layouts + presets + block flags to an optional executable; `ksx run --game`
//! launches it, applies the profile, and stops emulation on exit (legacy >3 s exit
//! rule preserved). Designed for LaunchBox/RetroBat wrapper integration.
//! Implemented in M5.
