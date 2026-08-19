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
/// Bumps the instance TTL on every call, the same way [`read_admin`] does.
/// Soroban's instance storage is a single ledger entry shared by every
/// instance key (`Admin`, `Paused`, `RecoveryAddress`, ...), so in practice
/// any instance read on a live contract keeps the whole entry — including
/// the pause flag — warm. But a contract path that checks `is_paused`
/// without also touching another instance key (or one that's simply quiet
/// for a long stretch while paused) had no guaranteed keep-alive of its
/// own, and a missing entry silently reads back as `unpaused` via
/// `unwrap_or(false)` below rather than failing loudly. Bumping here removes
/// that dependency on call order.
pub fn is_paused(env: &Env) -> bool {
    bump_instance_ttl(env);
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
    use soroban_sdk::contract;
    use soroban_sdk::testutils::{storage::Instance as _, Ledger};

    #[contract]
    struct TestContract;

    /// Regression test for issue #520: `is_paused` used to be a bare read
    /// with no TTL side effect, unlike every other instance-storage reader
    /// in this module. A contract whose only activity was pause checks (no
    /// `read_admin` or other instance write/read to keep the entry warm)
    /// could let the instance entry's TTL run out while paused, and a
    /// missing entry silently reads back as "not paused" via
    /// `unwrap_or(false)` rather than failing loudly.
    #[test]
    fn is_paused_bumps_instance_ttl() {
        let env = Env::default();
        let contract_id = env.register_contract(None, TestContract);
        env.as_contract(&contract_id, || {
            set_paused(&env, true);
            let initial_ttl = env.storage().instance().get_ttl();

            // Advance far enough that the remaining TTL drops below the bump
            // threshold, but not so far that the entry is archived outright.
            let advance = initial_ttl.saturating_sub(TTL_THRESHOLD_LEDGERS) + 1_000;
            env.ledger().with_mut(|li| {
                li.sequence_number += advance;
            });
            let ttl_before = env.storage().instance().get_ttl();
            assert!(
                ttl_before < TTL_THRESHOLD_LEDGERS,
                "test setup should have decayed the TTL below the threshold, got {ttl_before}",
            );

            assert!(is_paused(&env));

            let ttl_after = env.storage().instance().get_ttl();
            assert!(
                ttl_after > ttl_before,
                "is_paused should have refreshed the instance TTL: before={ttl_before}, after={ttl_after}",
            );
        });
    }
}
