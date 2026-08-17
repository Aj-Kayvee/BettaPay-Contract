//! Shared storage helpers.
//!
//! Each contract keeps its own private `DataKey` enum for keys that are
//! contract-specific (e.g. settlement's `Merchant(Address)` or governance's
//! `Anchor(Address)`). The keys that are semantically shared — the admin
//! address, the pause flag, the recovery address, and the pending recovery
//! operation — live in [`CommonDataKey`] so every contract reads and writes
//! them in exactly the same shape.
//!
//! The on-chain SCVal encoding of a Soroban `#[contracttype]` enum is based on
//! the variant name only; the parent enum's Rust name is not part of the
//! encoding. So a value written under `governance_contract::DataKey::Admin`
//! reads back identically through `bettapay_common::CommonDataKey::Admin`,
//! which is what allows both contracts to share this enum without disturbing
//! any existing storage entry.

use soroban_sdk::{contracttype, Address, Env, String};

use crate::constants::{TTL_BUMP_LEDGERS, TTL_THRESHOLD_LEDGERS};

/// Instance-storage keys shared by every BettaPay contract.
///
/// Adding a variant here is a coordinated change — every contract that uses
/// the variant must agree on what it stores and on whether the TTL handling
/// lives in this crate or in the contract's own helpers.
#[derive(Clone)]
#[contracttype]
pub enum CommonDataKey {
    /// Contract admin `Address` (instance storage).
    Admin,
    /// Recovery `Address` authorised to initiate the recovery flow
    /// (instance storage).
    RecoveryAddress,
    /// Pending recovery operation, present only between `initiate_recovery`
    /// and `execute_recovery` (instance storage).
    PendingRecovery,
    /// Pause-flag `bool` controlling whether mutating operations are blocked
    /// (instance storage).
    Paused,
}

/// Returns the stored admin `Address` and refreshes the instance TTL while
/// reading.
///
/// Returns `None` if the contract has not been initialised yet; the caller is
/// expected to map a missing admin to its own `NotInitialized` error variant
/// so the panic message keeps the contract's specific error code.
pub fn read_admin(env: &Env) -> Option<Address> {
    bump_instance_ttl(env);
    env.storage().instance().get(&CommonDataKey::Admin)
}

/// Returns `true` if the contract is currently paused.
///
/// This is a cheap read: it just inspects the instance flag without bumping
/// TTL or cloning data. Mutating operations that gate on pause state should
/// still keep the flag warm via [`bump_instance_ttl`] if they care about the
/// entry's lifetime.
pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&CommonDataKey::Paused)
        .unwrap_or(false)
}

/// Writes the pause flag to instance storage.
pub fn set_paused(env: &Env, paused: bool) {
    env.storage()
        .instance()
        .set(&CommonDataKey::Paused, &paused);
}

/// Returns `true` if `address` is the network's zero address.
///
/// The Soroban zero-address is the well-known string
/// `"GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"` (a `G...`
/// Stellar-style address whose 32-byte key is all zeros). Both contracts need
/// to reject this on admin transfer, merchant registration, etc., so the
/// comparison lives here and callers translate a `true` result into their own
/// `Invalid*` error variant.
///
/// This is called on every admin/merchant/governance write, so it avoids
/// encoding `address` to a strkey `String` (`Address::to_string`) just to
/// compare it: that direction of the conversion scales with every call and
/// is the more expensive one, since it makes the host re-derive and allocate
/// a fresh base-32 `String` object for the *caller-supplied* address each
/// time. Instead it builds the zero `Address` once and compares the two
/// `Address` values directly, which is a cheap host object comparison
/// (`Address`'s `PartialEq` delegates to the host's `obj_cmp`) with no
/// per-call `String` allocation on the hot path.
pub fn is_zero_address(env: &Env, address: &Address) -> bool {
    let zero_address = Address::from_string(&String::from_str(
        env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));
    address == &zero_address
}

/// Bump the instance-storage TTL using the policy defined in
/// [`crate::constants`].
///
/// Useful for non-admin read paths that want to keep the instance entry warm
/// using the standard 14 / 30 day policy. Contracts that intentionally use a
/// different TTL for specific keys (per ADR 003) should call
/// `env.storage().instance().extend_ttl(...)` directly in those spots.
pub fn bump_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD_LEDGERS, TTL_BUMP_LEDGERS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn zero_address_is_recognised() {
        let env = Env::default();
        let zero = Address::from_string(&String::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ));
        assert!(is_zero_address(&env, &zero));
    }

    #[test]
    fn non_zero_address_is_not_flagged() {
        let env = Env::default();
        let address = Address::generate(&env);
        assert!(!is_zero_address(&env, &address));
    }
}
