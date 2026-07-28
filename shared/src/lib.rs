//! Storage helpers shared between `governance_contract` and
//! `settlement_contract`, so `read_admin` and the pause-checking pattern
//! have a single implementation instead of two independently maintained
//! copies (see BettaPay-Contract issues #312 and #313).
//!
//! Each contract keeps its own `DataKey` enum and its own `#[contracterror]`
//! type; the `Admin`/`Paused` unit variants below serialize identically to
//! a contract-local enum's variants of the same name (a Soroban
//! `#[contracttype]` enum's storage key encodes only the variant name and
//! shape, never the enclosing Rust type), so this is safe to use for the
//! same `Admin`/`Paused` storage slots each contract's own `init`, `pause`,
//! `unpause`, `transfer_admin`, and `execute_recovery` already read/write
//! through their local `DataKey`.

#![no_std]

use soroban_sdk::{contracttype, Address, Env, Error};

const ADMIN_TTL_THRESHOLD: u32 = 17280 * 14;
const ADMIN_TTL_BUMP: u32 = 17280 * 30;

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Paused,
}

/// Reads the admin address from instance storage, refreshing its TTL.
///
/// Panics with `not_initialized` if no admin has been set yet.
pub fn read_admin<E>(env: &Env, not_initialized: E) -> Address
where
    E: Into<Error>,
{
    env.storage()
        .instance()
        .extend_ttl(ADMIN_TTL_THRESHOLD, ADMIN_TTL_BUMP);
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| env.panic_with_error(not_initialized))
}

/// Returns whether the contract's paused flag is currently set.
pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

/// Panics with `paused_error` if the contract is currently paused.
pub fn assert_not_paused<E>(env: &Env, paused_error: E)
where
    E: Into<Error>,
{
    if is_paused(env) {
        env.panic_with_error(paused_error);
    }
}
