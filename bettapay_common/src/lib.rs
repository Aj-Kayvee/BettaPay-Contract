//! # BettaPay Common
//!
//! Shared types, constants, and helpers for the BettaPay protocol's Soroban
//! contracts. Both the governance contract and the settlement contract depend
//! on this crate to avoid duplicating protocol-wide constants and the small set
//! of storage/event idioms every contract needs.
//!
//! ## Modules
//!
//! - [`constants`] — protocol-wide numeric constants (basis-point denominators,
//!   fee bounds, TTL helpers, recovery delay).
//! - [`storage`] — common storage helpers (`read_admin`, `is_paused`,
//!   `is_zero_address`) and the [`CommonDataKey`] enum that owns the keys
//!   shared by every contract's instance storage.
//! - [`events`] — shared event shapes (such as [`events::AdminTransferred`] and
//!   [`events::PendingRecovery`]) and the canonical event-name strings.
//!
//! ## Why this crate exists
//!
//! Without it, the two contracts each defined their own copy of `MIN_FEE_BPS`,
//! `BPS_DENOMINATOR`, `RECOVERY_DELAY_SECONDS`, the `Admin`/`Paused`/`RecoveryAddress`
//! storage accessors, and the `AdminTransferred` event payload — easy to let
//! drift out of sync on a quiet PR. See issue #315 for the original
//! motivation; further module extraction (event helpers, error mapping) is
//! tracked by issue #84.

#![no_std]

pub mod constants;
pub mod events;
pub mod storage;
