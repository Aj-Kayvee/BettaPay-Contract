//! # BettaPay Settlement Contract
//!
//! This module provides the core implementation of the settlement contract for BettaPay.
//! It is responsible for managing merchant registration, settlement rules, and the payment storage architecture.
//!
//! ## Merchant Rules
//!
//! The contract maintains a registry of authorized merchants. For each registered merchant,
//! an admin can configure specific settlement rules defined by the `SettlementRule` struct.
//! A settlement rule dictates:
//! - **Platform Fee (BPS)**: The fee collected by the platform.
//! - **Network Fee (BPS)**: The fee collected by the network.
//! - **Settlement Delay**: The delay in ledger sequences before a settlement can occur.
//! - **Auto-settle**: A flag indicating whether auto-settlement is enabled.
//!
//! If a merchant lacks a specific rule, the system falls back to an admin-configured global default rule,
//! and ultimately to a hardcoded bootstrap default rule if necessary.
//!
//! ## Payment Storage Architecture
//!
//! Payments are tracked and stored using a unique 32-byte reference (`BytesN<32>`).
//! When `store_payment_reference` is invoked, the contract calculates the exact fee split
//! (platform fee, network fee, and net merchant amount) based on the merchant's effective settlement rule.
//!
//! The resulting data is persisted in a `PaymentRecord`, which encapsulates:
//! - The calculated amounts and fee BPS.
//! - The ledger sequence of the transaction.
//! - Settlement delay and auto-settle configurations.
//!
//! The contract leverages different `DataKey` variants (`Admin`, `Merchant`, `Rule`, `Payment`, etc.)
//! to securely organize persistent and instance storage, while applying TTL extensions to ensure
//! active records remain available and do not expire prematurely.
//!
//! ## Event Conventions
//!
//! Events are emitted via [`soroban_sdk::Env::events`]. To give off-chain
//! indexers a predictable topic layout, every event in this contract follows
//! the same conventions:
//!
//! - `topic[0]` is always the event name as a [`Symbol`], constructed via
//!   [`Symbol::new`] (or [`symbol_short!`] when the name fits in nine bytes).
//!   Indexers filter on this single topic to dispatch by event type.
//! - `topic[1..n]` carry the entity identifiers that scope the event —
//!   typically an [`Address`] (merchant, asset, admin), but for some events
//!   also a [`BytesN<32>`] (new Wasm hash on `contract_upgraded`, payment
//!   reference on `payment_stored`). The exact shape of `topic[1..n]` is
//!   fixed per event.
//! - The **data payload** carries the values describing the state change.
//!   Its shape is event-specific: a single value (`true` for `pause`,
//!   `admin` for `merchant_registered`), a tuple (e.g.
//!   `(admin, prev, rule)` for `settlement_rule_updated`), a typed struct
//!   such as the `SettlementRule` emitted on `bootstrap_fallback`, or `()`.
//! - Each entry point emits exactly the events tied to the state change it
//!   performs; no two events emitted by the same call describe the same
//!   logical change.
//!
//! ## Upgrade Process
//!
//! [`SettlementContract::upgrade`] replaces the Wasm and nothing else. That is
//! what makes it safe, and also why changing a stored type is a separate
//! problem: nothing converts existing entries, and nothing checks that they
//! still match the types the new code expects. A mismatched read fails at
//! runtime, after the upgrade has already landed.
//!
//! 1. Wasm upgrades replace code only; every storage entry survives untouched.
//! 2. Storage migrations run **inside the upgraded contract**, as an
//!    admin-gated `migrate` entry point — not from a separate migration
//!    contract. A contract can only reach its own storage, so another contract
//!    has no access path to `Payment`, `Merchant` or `Rule` entries.
//! 3. Ship the old type definition in the same Wasm as the new one. A
//!    `#[contracttype]` struct is encoded by field name, so a `PaymentRecord`
//!    written before a new field existed will not deserialise into the new
//!    struct — the old type is what keeps those entries readable.
//! 4. Order is: upgrade the Wasm, then call `migrate`, then verify the
//!    post-upgrade state, then remove the migration code in a later upgrade.
//! 5. `Payment(BytesN<32>)`, `Merchant(Address)` and `Rule(Address)` are keyed
//!    by value and Soroban cannot enumerate storage keys — which is why
//!    [`SettlementContract::get_payments`] takes the references from the
//!    caller. Convert these lazily on read, or pass the keys in explicitly.
//! 6. Call `extend_ttl` on anything the migration rewrites: `set` alone does
//!    not extend an entry's life, so a migrated record would otherwise expire
//!    sooner than an untouched one.
//!
//! Full guidance, including worked examples and how to test a migration, is in
//! [`DEVELOPMENT.md`](https://github.com/Betta-Pay/BettaPay-Contract/blob/main/DEVELOPMENT.md).
//!
//! ## Pause Model
//! The pause flag blocks payment-processing and merchant-management
//! operations (`register_merchant`, `unregister_merchant`,
//! `set_settlement_rule`, `clear_settlement_rule`, `set_default_rule`,
//! `store_payment_reference`, `update_governance` all call
//! `assert_not_paused`). The following administrative operations are
//! intentionally NOT blocked during pause, so the admin can fix the root
//! cause of the emergency:
//!
//! - `upgrade` — deploy a fix
//! - `transfer_admin` — rotate compromised keys
//! - `initiate_recovery` / `cancel_recovery` / `execute_recovery` — the
//!   admin-recovery flow itself must keep working while paused
//!
//! ## Event Convention
//!
//! This contract follows a consistent event emission pattern (see Issue #49):
//!
//! **Topics** carry the fixed event-name symbol and filterable entity
//! identifiers. The first topic is always a [`Symbol`] naming the event type,
//! enabling indexers to filter by event kind. Subsequent topics hold the
//! primary identifiers relevant to the event (e.g., merchant address, payment
//! reference), so listeners can subscribe to events for a specific entity
//! without scanning all events.
//!
//! **Data** carries caller context and event-specific details — information
//! that is useful once the event has been matched by topic but is not needed
//! for filtering. Typically this includes the caller's address (admin) and
//! any before/after values or configuration data.
//!
//! ### Canonical Example
//!
//! [`SettlementContract::register_merchant`] is the canonical example of this convention:
//!
//! | Role   | Value                                                       |
//! |--------|-------------------------------------------------------------|
//! | Topics | `(Symbol("merchant_registered"), Address merchant)`         |
//! | Data   | `Address caller` (the admin who authorized the registration)|
//!
//! New events should follow the same pattern: filterable identifiers in
//! topics, caller context and details in data.

// TODO: Refactor flat file structure into modular hierarchy (Issue #84)
// Intended module structure:
// - mod types: Data structures (enums, structs)
// - mod storage: DataKey and storage access helpers
// - mod events: Event definitions and emission helpers
// - mod errors: Error enums
// - mod contract: Main contract trait implementation
// - mod test: Unit and integration tests

#![no_std]

use bettapay_common::{
    constants::{BPS_DENOMINATOR, MIN_FEE_BPS, RECOVERY_DELAY_SECONDS},
    events::PendingRecovery,
    storage::{self, CommonDataKey},
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    BytesN, Env, Symbol, Val, Vec,
};

const MIN_PAYMENT_AMOUNT: i128 = 100;
const MAX_SETTLEMENT_DELAY_LEDGER: u32 = 100_000;
/// Approximate ledgers per day on Stellar (~5s per ledger).
const LEDGERS_PER_DAY: u32 = 17280;

// TTL thresholds and bumps use LEDGERS_PER_DAY for readability.
const PAYMENT_TTL_THRESHOLD: u32 = LEDGERS_PER_DAY * 14;  // 14 days
const PAYMENT_TTL_BUMP: u32 = LEDGERS_PER_DAY * 30;       // 30 days
const RULE_TTL_THRESHOLD: u32 = LEDGERS_PER_DAY * 14;
const RULE_TTL_BUMP: u32 = LEDGERS_PER_DAY * 30;
const MERCHANT_TTL_THRESHOLD: u32 = LEDGERS_PER_DAY * 14;
const MERCHANT_TTL_BUMP: u32 = LEDGERS_PER_DAY * 30;
const RECOVERY_DELAY_SECONDS: u64 = 7 * 24 * 60 * 60;
const PAYMENT_TTL_THRESHOLD: u32 = 17280 * 14;
const PAYMENT_TTL_BUMP: u32 = 17280 * 30;
const RULE_TTL_THRESHOLD: u32 = 17280 * 14;
const RULE_TTL_BUMP: u32 = 17280 * 30;
const MERCHANT_TTL_THRESHOLD: u32 = 17280 * 14;
const MERCHANT_TTL_BUMP: u32 = 17280 * 30;
const DEFAULT_TIMELOCK_DELAY_SECONDS: u64 = 2 * 24 * 60 * 60; // 48 hours

// Settlement-specific TTL policy for short-lived reads of admin / governance /
// recovery addresses. Deliberately shorter than the protocol defaults so that
// an inactive instance-side entry can still be evicted in days rather than
// weeks — see ADR 003 for the rationale.
const READ_INSTANCE_TTL_THRESHOLD: u32 = 50_000;
const READ_INSTANCE_TTL_BUMP: u32 = 100_000;

// Used until the admin sets a global default settlement rule.
const BOOTSTRAP_DEFAULT_RULE: SettlementRule = SettlementRule {
    platform_fee_bps: 100,
    network_fee_bps: 0,
    settlement_delay_ledger: 0,
    auto_settle: false,
};

/// Configuration governing how merchant payments are settled.
///
/// This struct defines the fee allocation and settlement timing for a merchant,
/// including the platform and network fee shares as well as whether
/// settlement is processed automatically after a delay.
#[derive(Clone)]
#[contracttype]
pub struct SettlementRule {
    /// Platform fee charged on each payment, expressed in basis points.
    ///
    /// One basis point is 0.01%, and 100 basis points equals 1%.
    /// This value is used when calculating the platform's share of a payment.
    pub platform_fee_bps: u32,
    /// Network fee charged on each payment, expressed in basis points.
    ///
    /// This represents the portion reserved for network or protocol-related
    /// costs and is combined with other fees as validated elsewhere in the contract.
    pub network_fee_bps: u32,
    /// Number of ledger closes to wait before settlement becomes eligible.
    ///
    /// A value of `0` enables immediate settlement, while larger values delay
    /// settlement until the specified number of ledgers has elapsed.
    pub settlement_delay_ledger: u32,
    /// Indicates whether settlement should occur automatically.
    ///
    /// When set to `true`, settlements may be processed automatically after
    /// the configured settlement delay has elapsed; when `false`, settlement
    /// requires manual or external triggering.
    pub auto_settle: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct FeeSplit {
    /// The total gross amount of the payment.
    /// Mirrors the `amount` parameter passed to `store_payment_reference`.
    pub gross_amount: i128,
    /// Portion of the settlement fee allocated to the platform.
    /// This amount is calculated by applying the platform fee basis points to the gross amount.
    pub platform_fee_amount: i128,
    /// Portion of the settlement fee allocated to the network.
    /// This amount is calculated by applying the network fee basis points to the gross amount.
    pub network_fee_amount: i128,
    /// Net amount allocated to the merchant.
    /// This derived output is calculated as the gross amount minus the rounded platform and network fee amounts.
    pub merchant_amount: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct PaymentRecord {
    /// The total gross amount of the payment processed.
    /// Set upon payment creation and used to derive the fee split.
    pub amount: i128,
    /// The exact amount deducted for the platform fee.
    /// Calculated and stored at payment creation to lock in the fee value.
    pub platform_fee_amount: i128,
    /// The exact amount deducted for the network fee.
    /// Calculated and stored at payment creation to lock in the fee value.
    pub network_fee_amount: i128,
    /// The net payout amount owed to the merchant.
    /// Calculated at payment creation to ensure deterministic settlement value.
    pub merchant_amount: i128,
    /// The platform fee rate (in basis points) applied to this payment.
    /// Snapshot taken from the active settlement rule during creation.
    pub platform_fee_bps: u32,
    /// The network fee rate (in basis points) applied to this payment.
    /// Snapshot taken from the active settlement rule during creation.
    pub network_fee_bps: u32,
    /// Ledger sequence timestamp when the payment was recorded.
    /// Used alongside settlement_delay_ledger to verify if the payment is ripe for settlement.
    pub ledger: u32,
    /// The delay period (in ledgers) before settlement can occur.
    /// Sourced from the active settlement rule and used to prevent premature settlement.
    pub settlement_delay_ledger: u32,
    /// Indicates if the payment should participate in automated settlement batches.
    /// Set from the active rule and used by external auto-settlement processes.
    pub auto_settle: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct FeeConfig {
    pub platform_fee_bps: u32,
    pub network_fee_bps: u32,
}

// Admin, RecoveryAddress, PendingRecovery, and Paused live in
// `bettapay_common::storage::CommonDataKey` instead of here - see that
// type's doc comment for why a shared key type is safe to mix with this
// contract's own storage without a migration.
#[derive(Clone)]
#[contracttype]
pub enum Operation {
    UpdateGovernance(Address),
    CancelRecovery,
    TransferAdmin(Address),
    Upgrade(BytesN<32>),
    RegisterMerchant(Address),
    UnregisterMerchant(Address),
    SetSettlementRule(Address, SettlementRule),
    ClearSettlementRule(Address),
    SetDefaultRule(SettlementRule),
}

#[derive(Clone)]
#[contracttype]
pub enum Operation {
    UpdateGovernance(Address),
    CancelRecovery,
    TransferAdmin(Address),
    Upgrade(BytesN<32>),
    RegisterMerchant(Address),
    UnregisterMerchant(Address),
    SetSettlementRule(Address, SettlementRule),
    ClearSettlementRule(Address),
    SetDefaultRule(SettlementRule),
}

#[derive(Clone)]
#[contracttype]
enum DataKey {
    /// Instance — singleton address, rarely changes.
    Governance,
    /// Persistent — one per merchant, many entries.
    Merchant(Address),
    /// Persistent — one per merchant, may expire.
    Rule(Address),
    /// Persistent — single value but may be updated.
    DefaultRule,
    /// Persistent — one per payment, high volume.
    Payment(BytesN<32>),
    /// Instance — singleton boolean, read on every mutating call.
    Paused,
    /// Storage key for a scheduled operation.
    ScheduledOperation(BytesN<32>),
}

#[contracterror]
#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
pub enum SettlementError {
    /// `init()` has already been called. Only one initialization is permitted.
    AlreadyInitialized = 1,
    /// `init()` has not been called. All admin-guarded functions require prior initialization.
    NotInitialized = 2,
    /// The caller does not match the stored admin address.
    Unauthorized = 3,
    /// `register_merchant` was called for an address that is already registered.
    MerchantExists = 4,
    /// The target merchant address is not registered. Raised by
    /// `set_settlement_rule`, `store_payment_reference`, `calculate_fee_split`,
    /// and `unregister_merchant` when the merchant is missing.
    MerchantMissing = 5,
    /// The fee BPS values exceed 10 000 (`BPS_DENOMINATOR`) or their sum
    /// exceeds 10 000, or either value is below `MIN_FEE_BPS` (5).
    /// Raised by `set_settlement_rule` and `set_default_rule`.
    InvalidFeeBps = 6,
    // Code 7 is intentionally reserved (formerly `InvalidAmount`).
    /// `store_payment_reference` was called with a 32‑byte reference that
    /// already exists in storage.
    DuplicatePaymentReference = 8,
    /// The contract is paused. Most state‑mutating operations are blocked.
    Paused = 9,
    /// No merchant-specific rule has been set. The merchant will use the default rule or bootstrap fallback.
    MerchantRuleNotSet = 10,
    /// The supplied address is an empty string.
    /// Raised by `register_merchant` and `transfer_admin`.
    EmptyAddress = 20,
    /// The supplied address is the zero‑address.
    /// Raised by `register_merchant` and `transfer_admin`.
    ZeroAddress = 21,
    /// `store_payment_reference` was called with an all‑zero 32‑byte
    /// reference, which is reserved.
    InvalidPaymentReference = 12,
    /// `settlement_delay_ledger` exceeds `MAX_SETTLEMENT_DELAY_LEDGER`
    /// (100 000). Raised by `set_settlement_rule` and `set_default_rule`.
    InvalidSettlementDelay = 13,
    /// `transfer_admin` was called with the current admin address as the
    /// new admin. The new admin must be different.
    InvalidAdmin = 14,
    InvalidGovernance = 15,
    InvalidRecoveryAddress = 16,
    RecoveryNotPending = 17,
    RecoveryDelayActive = 18,
    /// The payment amount is large enough that multiplying it by a fee's
    /// basis points would overflow `i128`. Raised by `calculate_split`
    /// before the multiplication is attempted.
    AmountOverflow = 19,
    /// The scheduled operation is not yet ready for execution.
    ExecutionNotReady = 22,
    /// The operation has not been scheduled.
    OperationNotScheduled = 23,
    /// The operation has already been scheduled.
    OperationAlreadyScheduled = 24,
}

#[contract]
pub struct SettlementContract;

#[contractimpl]
impl SettlementContract {
    /// Initialize the contract with the given admin address.
    ///
    /// # Panics
    ///
    /// * [`AlreadyInitialized`](SettlementError::AlreadyInitialized) — if the contract has already been initialized.
    pub fn init(env: Env, admin: Address, governance: Address, recovery_address: Address) {
        if env.storage().instance().has(&CommonDataKey::Admin) {
            panic_with_error!(&env, SettlementError::AlreadyInitialized);
        }
        admin.require_auth();
        validate_governance(&env, &governance);
        validate_nonzero_address(
            &env,
            &recovery_address,
            SettlementError::InvalidRecoveryAddress,
            SettlementError::InvalidRecoveryAddress,
        );
        env.storage().instance().set(&CommonDataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage()
            .instance()
            .set(&CommonDataKey::RecoveryAddress, &recovery_address);
    }

    /// Return the current admin address.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    pub fn get_admin(env: Env) -> Address {
        read_admin(&env)
    }

    pub fn get_governance(env: Env) -> Address {
        read_governance(&env)
    }

    pub fn get_recovery_address(env: Env) -> Address {
        read_recovery_address(&env)
    }

    pub fn update_governance(env: Env, new_governance: Address) {
        let admin = read_admin(&env);
        admin.require_auth();
        assert_not_paused(&env);
        validate_governance(&env, &new_governance);
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        env.events().publish(
            (Symbol::new(&env, "governance_updated"),),
            (admin, new_governance),
        );
    }

    pub fn initiate_recovery(env: Env, new_admin: Address) {
        let recovery_address = read_recovery_address(&env);
        recovery_address.require_auth();
        validate_nonzero_address(
            &env,
            &new_admin,
            SettlementError::InvalidAdmin,
            SettlementError::InvalidAdmin,
        );

        let pending = PendingRecovery {
            new_admin: new_admin.clone(),
            execute_after: env.ledger().timestamp() + RECOVERY_DELAY_SECONDS,
        };
        env.storage()
            .instance()
            .set(&CommonDataKey::PendingRecovery, &pending);
        // Settlement re-uses the same `(recovery, new_admin, execute_after)`
        // payload shape it had before the refactor. The topic name is the same.
        env.events().publish(
            (Symbol::new(&env, "recovery_initiated"),),
            (recovery_address, new_admin, pending.execute_after),
        );
    }

    pub fn cancel_recovery(env: Env) {
        let admin = read_admin(&env);
        admin.require_auth();
        if !env
            .storage()
            .instance()
            .has(&CommonDataKey::PendingRecovery)
        {
            panic_with_error!(&env, SettlementError::RecoveryNotPending);
        }
        env.storage()
            .instance()
            .remove(&CommonDataKey::PendingRecovery);
        env.events()
            .publish((Symbol::new(&env, "recovery_cancelled"),), admin);
    }

    pub fn execute_recovery(env: Env) {
        let pending = read_pending_recovery(&env);
        if env.ledger().timestamp() < pending.execute_after {
            panic_with_error!(&env, SettlementError::RecoveryDelayActive);
        }

        env.storage()
            .instance()
            .set(&CommonDataKey::Admin, &pending.new_admin);
        env.storage()
            .instance()
            .remove(&CommonDataKey::PendingRecovery);
        // Settlement emits just the new admin here, not the structured
        // `AdminTransferred` payload that governance emits. The two contracts
        // diverged historically; unifying the payload shape is tracked by
        // issue #84.
        env.events()
            .publish((Symbol::new(&env, "recovery_executed"),), pending.new_admin);
    }

    /// Transfer the admin role to a new address.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    /// * [`EmptyAddress`](SettlementError::EmptyAddress) — if `new_admin` is an empty string.
    /// * [`ZeroAddress`](SettlementError::ZeroAddress) — if `new_admin` is the zero address.
    /// * [`InvalidAdmin`](SettlementError::InvalidAdmin) — if `new_admin` is the same as the current admin.
    ///
    /// ## Emitted Event: `admin`
    ///
    /// **Topics**: `(Symbol("admin"),)`
    /// **Data**: `Address new_admin`
    pub fn transfer_admin(env: Env, new_admin: Address) {
        let admin = read_admin(&env);
        admin.require_auth();

        validate_nonzero_address(
            &env,
            &new_admin,
            SettlementError::EmptyAddress,
            SettlementError::ZeroAddress,
        );

        if new_admin == admin {
            panic_with_error!(&env, SettlementError::InvalidAdmin);
        }
        env.storage()
            .instance()
            .set(&CommonDataKey::Admin, &new_admin);
        env.storage()
            .instance()
            .remove(&CommonDataKey::PendingRecovery);
        env.events().publish((symbol_short!("admin"),), new_admin);
    }

    /// Upgrades the underlying Wasm bytecode implementation of the contract under strict admin authority.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if the caller is not the registered admin.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        let admin = read_admin(&env);
        admin.require_auth();

        let event_wasm_hash = new_wasm_hash.clone();
        env.events().publish(
            (Symbol::new(&env, "contract_upgraded"), event_wasm_hash),
            admin,
        );

        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Pause the contract, preventing certain operations.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if the caller is not the admin.
    ///
    /// ## Emitted Event: `pause`
    ///
    /// **Topics**: `(Symbol("pause"),)`
    /// **Data**: `bool true`
    /// **Data**: `(Address caller, bool is_paused)`
    pub fn pause(env: Env) {
        let admin = read_admin(&env);
        admin.require_auth();
        // Bypasses timelock for emergencies
        env.storage().instance().set(&DataKey::Paused, &true);
        env.events()
            .publish((symbol_short!("pause"),), (admin, true));
    }

    
    /// Unpause the contract, re-enabling paused operations.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if the caller is not the admin.
    ///
    /// ## Emitted Event: `unpause`
    ///
    /// **Topics**: `(Symbol("unpause"),)`
    /// **Data**: `bool false`
    /// **Data**: `(Address caller, bool is_paused)`
    pub fn unpause(env: Env) {
        let admin = read_admin(&env);
        admin.require_auth();
        // Bypasses timelock for emergencies
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events()
            .publish((symbol_short!("unpause"),), (admin, false));
    }

    
    /// Returns `true` if the contract is currently paused, `false` otherwise.
    pub fn is_paused(env: Env) -> bool {
        storage::is_paused(&env)
    }

    /// ## Emitted Event: `merchant_registered`
    ///
    /// **Topics**: `(Symbol("merchant_registered"), Address merchant)`
    /// - First topic: fixed event-name symbol for filtering by event type
    /// - Second topic: the merchant address that was registered
    ///
    /// **Data**: `Address caller`
    /// - `caller`: the admin who authorized the registration
    pub fn register_merchant(env: Env, merchant: Address) {
        assert_not_paused(&env);

        validate_nonzero_address(
            &env,
            &merchant,
            SettlementError::EmptyAddress,
            SettlementError::ZeroAddress,
        );

        let admin = read_admin(&env);
        admin.require_auth();

        let key = DataKey::Merchant(merchant.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::MerchantExists);
        }

        env.storage().persistent().set(&key, &());
        env.storage()
            .persistent()
            .extend_ttl(&key, MERCHANT_TTL_THRESHOLD, MERCHANT_TTL_BUMP);
        env.events()
            .publish((Symbol::new(&env, "merchant_registered"), merchant), admin);
    }

    /// Remove a merchant from the registry and clear any associated settlement rule.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    /// * [`Unauthorized`](SettlementError::Unauthorized) — if the caller is not the admin.
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not registered.
    ///
    /// ## Emitted Events
    ///
    /// If the merchant has a settlement rule set, a `settlement_rule_cleared`
    /// event is emitted before the `merchant_unregistered` event.
    ///
    /// ### `settlement_rule_cleared` (conditional)
    ///
    /// **Topics**: `(Symbol("settlement_rule_cleared"), Address merchant)`
    ///
    /// **Data**: `(Address caller, SettlementRule removed)`
    /// - `caller`: the admin who authorized the unregistration
    /// - `removed`: the settlement rule that was removed
    ///
    /// ### `merchant_unregistered`
    ///
    /// **Topics**: `(Symbol("merchant_unregistered"), Address merchant)`
    /// - First topic: fixed event-name symbol for filtering by event type
    /// - Second topic: the merchant address that was unregistered
    ///
    /// **Data**: `Address caller`
    /// - `caller`: the admin who authorized the unregistration
    pub fn unregister_merchant(env: Env, merchant: Address) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        let key = DataKey::Merchant(merchant.clone());
        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }

        env.storage().persistent().remove(&key);

        let rule_key = DataKey::Rule(merchant.clone());
        let old_rule: Option<SettlementRule> = env.storage().persistent().get(&rule_key);
        if let Some(old_rule) = old_rule {
            env.storage().persistent().remove(&rule_key);
            env.events().publish(
                (
                    Symbol::new(&env, "settlement_rule_cleared"),
                    merchant.clone(),
                ),
                (admin.clone(), old_rule),
            );
        }

        env.events().publish(
            (Symbol::new(&env, "merchant_unregistered"), merchant),
            admin,
        );
    }

    /// ## Emitted Event: `settlement_rule_updated`
    ///
    /// **Topics**: `(Symbol("settlement_rule_updated"), Address rule_id)`
    /// - First topic: fixed event-name symbol for filtering by event type
    /// - Second topic: the merchant address identifying which rule was updated
    ///
    /// **Data**: `(Address caller, SettlementRule previous, SettlementRule current)`
    /// - `caller`: the admin who authorized the rule change
    /// - `previous`: the rule values before the update (or system defaults on first set)
    /// - `current`: the new rule values after the update
    pub fn set_settlement_rule(env: Env, merchant: Address, rule: SettlementRule) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        if rule.platform_fee_bps > BPS_DENOMINATOR || rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps < MIN_FEE_BPS || rule.network_fee_bps < MIN_FEE_BPS {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps + rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(&env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::Rule(merchant.clone()))
            .unwrap_or_else(|| read_rule_or_default(&env, merchant.clone()));

        let key = DataKey::Rule(merchant.clone());
        env.storage().persistent().set(&key, &rule);

        env.storage()
            .persistent()
            .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

        env.events().publish(
            (Symbol::new(&env, "settlement_rule_updated"), merchant),
            (admin, prev, rule),
        );
    }

    /// ## Emitted Event: `settlement_rule_cleared`
    ///
    /// **Topics**: `(Symbol("settlement_rule_cleared"), Address rule_id)`
    /// - First topic: fixed event-name symbol for filtering by event type
    /// - Second topic: the merchant address identifying which rule was cleared
    ///
    /// **Data**: `(Address caller, SettlementRule removed, SettlementRule fallback)`
    /// - `caller`: the admin who authorized the removal
    /// - `removed`: the rule values that were removed from storage
    /// - `fallback`: the effective rule the merchant will use after clearing (global default or bootstrap)
    pub fn clear_settlement_rule(env: Env, merchant: Address) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        let key = DataKey::Rule(merchant.clone());
        let removed = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&key)
            .unwrap_or_else(|| panic_with_error!(&env, SettlementError::MerchantRuleNotSet));

        env.storage().persistent().remove(&key);

        let fallback = read_rule_or_default(&env, merchant.clone());

        env.events().publish(
            (Symbol::new(&env, "settlement_rule_cleared"), merchant),
            (admin, removed, fallback),
        );
    }

    /// ## Emitted Event: `default_rule_updated`
    ///
    /// **Topics**: `(Symbol("default_rule_updated"),)`
    /// - First topic: fixed event-name symbol for filtering by event type
    ///
    /// **Data**: `(Address caller, SettlementRule previous, SettlementRule current)`
    /// - `caller`: the admin who authorized the change
    /// - `previous`: the previous global default rule (or bootstrap fallback if none was set)
    /// - `current`: the new global default rule
    /// ## Event: `default_rule_updated`
    ///
    /// Emitted when the global default settlement rule is updated.
    ///
    /// ## Panics
    ///
    /// - Panics with `InvalidSettlementDelay` if `new_rule.settlement_delay_ledger`
    ///   exceeds `MAX_SETTLEMENT_DELAY_LEDGER`.
    pub fn set_default_rule(env: Env, new_rule: SettlementRule) {
        assert_not_paused(&env);
        let admin = read_admin(&env);
        admin.require_auth();

        if new_rule.platform_fee_bps > BPS_DENOMINATOR || new_rule.network_fee_bps > BPS_DENOMINATOR
        {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if new_rule.platform_fee_bps < MIN_FEE_BPS || new_rule.network_fee_bps < MIN_FEE_BPS {
            panic_with_error!(&env, SettlementError::InvalidFeeBps);
        }
        if new_rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(&env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::DefaultRule)
            .unwrap_or(BOOTSTRAP_DEFAULT_RULE);

        env.storage()
            .persistent()
            .set(&DataKey::DefaultRule, &new_rule);
        env.storage().persistent().extend_ttl(
            &DataKey::DefaultRule,
            RULE_TTL_THRESHOLD,
            RULE_TTL_BUMP,
        );

        env.events().publish(
            (Symbol::new(&env, "default_rule_updated"),),
            (admin, prev, new_rule),
        );
    }

    /// Returns the global default settlement rule, if one has been set.
    /// Automatically extends the persistent storage TTL to prevent archival
    /// during public read queries (clausal to TTL eviction).
    pub fn get_default_rule(env: Env) -> Option<SettlementRule> {
        let key = DataKey::DefaultRule;
        match env.storage().persistent().get::<_, SettlementRule>(&key) {
            Some(rule) => {
                env.storage()
                    .persistent()
                    .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);
                Some(rule)
            }
            None => None,
        }
    }

    /// Store a payment reference for a merchant and calculate the fee split.
    ///
    /// # Panics
    ///
    /// * [`Paused`](SettlementError::Paused) — if the contract is paused.
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not registered.
    /// * [`InvalidPaymentReference`](SettlementError::InvalidPaymentReference) — if `reference` is all zeros.
    /// * [`AmountTooSmall`](SettlementError::AmountTooSmall) — if `amount` is below the minimum.
    /// * [`DuplicatePaymentReference`](SettlementError::DuplicatePaymentReference) — if the reference already exists.
    /// * [`AmountOverflow`](SettlementError::AmountOverflow) — if `amount * bps` would overflow `i128`.
    ///
    /// ## Emitted Event: `payment_stored`
    ///
    /// **Topics**: `(Symbol("payment_stored"), Address merchant, BytesN<32> reference)`
    /// **Data**: `()`
    ///
    /// The fee split (platform fee, network fee, merchant amount, gross amount)
    /// is available on the `PaymentRecord` in this event's data; no separate
    /// split event is emitted.
    pub fn store_payment_reference(
        env: Env,
        merchant: Address,
        reference: BytesN<32>,
        amount: i128,
    ) -> FeeSplit {
        assert_not_paused(&env);

        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        merchant.require_auth();
        if reference == BytesN::from_array(&env, &[0; 32]) {
            panic_with_error!(&env, SettlementError::InvalidPaymentReference);
        }
        if amount < MIN_PAYMENT_AMOUNT {
            panic_with_error!(&env, SettlementError::AmountTooSmall);
        }

        let payment_key = DataKey::Payment(reference.clone());
        if env.storage().persistent().has(&payment_key) {
            panic_with_error!(&env, SettlementError::DuplicatePaymentReference);
        }

        let rule = read_rule_or_default(&env, merchant.clone());
        let split = calculate_split(&env, amount, &rule);
        let record = PaymentRecord {
            amount,
            platform_fee_amount: split.platform_fee_amount,
            network_fee_amount: split.network_fee_amount,
            merchant_amount: split.merchant_amount,
            platform_fee_bps: rule.platform_fee_bps,
            network_fee_bps: rule.network_fee_bps,
            ledger: env.ledger().sequence(),
            settlement_delay_ledger: rule.settlement_delay_ledger,
            auto_settle: rule.auto_settle,
        };

        env.storage().persistent().set(&payment_key, &record);
        env.storage().persistent().extend_ttl(
            &payment_key,
            PAYMENT_TTL_THRESHOLD,
            PAYMENT_TTL_BUMP,
        );

        env.events().publish(
            (
                Symbol::new(&env, "payment_stored"),
                merchant.clone(),
                reference.clone(),
            ),
            (),
        );

        split
    }

    /// Returns `true` if the given address is a registered merchant, `false` otherwise.
    ///
    /// # Panics
    ///
    /// * [`NotInitialized`](SettlementError::NotInitialized) — if the contract has not been initialized yet.
    pub fn is_merchant_registered(env: Env, merchant: Address) -> bool {
        if !env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, SettlementError::NotInitialized);
        }
        is_merchant_registered_internal(&env, merchant)
    }

    /// Returns the merchant-specific settlement rule, if one has been set.
    /// Automatically extends the persistent storage TTL to prevent archival.
    pub fn get_settlement_rule(env: Env, merchant: Address) -> Option<SettlementRule> {
        let key = DataKey::Rule(merchant);

        if let Some(rule) = env.storage().persistent().get(&key) {
            // Extend the TTL using the same named constants as set_settlement_rule
            // so the read and write paths never drift apart if the policy changes.
            env.storage()
                .persistent()
                .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

            Some(rule)
        } else {
            None
        }
    }

    /// Calculate the fee split for a given merchant and amount without storing a payment reference.
    ///
    /// # Panics
    ///
    /// * [`MerchantMissing`](SettlementError::MerchantMissing) — if the merchant is not registered.
    /// * [`AmountZero`](SettlementError::AmountZero) — if `amount` is zero.
    /// * [`AmountNegative`](SettlementError::AmountNegative) — if `amount` is negative.
    /// * [`AmountOverflow`](SettlementError::AmountOverflow) — if `amount * bps` would overflow `i128`.
    pub fn calculate_fee_split(env: Env, merchant: Address, amount: i128) -> FeeSplit {
        if !is_merchant_registered_internal(&env, merchant.clone()) {
            panic_with_error!(&env, SettlementError::MerchantMissing);
        }
        if amount == 0 {
            panic_with_error!(&env, SettlementError::AmountZero);
        }
        if amount < 0 {
            panic_with_error!(&env, SettlementError::AmountNegative);
        }
        let rule = read_rule_or_default(&env, merchant);
        calculate_split(&env, amount, &rule)
    }

    /// Retrieve a payment record by its reference, extending the storage TTL if found.
    pub fn get_payment_reference(env: Env, reference: BytesN<32>) -> Option<PaymentRecord> {
        let key = DataKey::Payment(reference);
        let record: Option<PaymentRecord> = env.storage().persistent().get(&key);
        if record.is_some() {
            // `extend_ttl` only writes when the current TTL is below
            // `threshold`, so this has the same externally observable
            // behavior as a manual get_ttl-then-extend check, without
            // depending on `get_ttl`, which is test-only in production code.
            env.storage()
                .persistent()
                .extend_ttl(&key, PAYMENT_TTL_THRESHOLD, PAYMENT_TTL_BUMP);
        }
        record
    }

    /// Retrieve multiple payment records by a vector of references.
    ///
    /// The returned vector preserves the input order. Each entry is `Some(payment)`
    /// when a record exists for the corresponding reference and `None` otherwise.
    pub fn get_payments(env: Env, references: Vec<BytesN<32>>) -> Vec<Option<PaymentRecord>> {
        // `references.len()` is known upfront, so pre-allocating would avoid repeated
        // reallocation as this vector grows. soroban-sdk 21.7.7's Vec<T> has no
        // `with_capacity` constructor (only `new`, `from_array`, `from_slice`), so
        // this is left as a potential optimization for a future SDK version.
        let mut payments = Vec::new(&env);

        for reference in references.iter() {
            let payment = Self::get_payment_reference(env.clone(), reference.clone());
            payments.push_back(payment);
        }

        payments
    }

    /// Schedules an administrative operation to be executed after a timelock.
    pub fn schedule(env: Env, caller: Address, operation: Operation, execute_in: u64) {
        let admin = read_admin(&env);
        if caller != admin {
            panic_with_error!(&env, SettlementError::Unauthorized);
        }
        caller.require_auth();

        if execute_in < DEFAULT_TIMELOCK_DELAY_SECONDS {
            panic_with_error!(&env, SettlementError::ExecutionNotReady);
        }

        let op_hash = env.crypto().sha256(&operation.to_raw(&env));
        let key = DataKey::ScheduledOperation(op_hash.clone());

        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::OperationAlreadyScheduled);
        }

        let execute_at = env.ledger().timestamp() + execute_in;
        env.storage().persistent().set(&key, &execute_at);
        env.storage()
            .persistent()
            .extend_ttl(&key, 17280 * 14, 17280 * 30);

        env.events().publish(
            (Symbol::new(&env, "op_scheduled"), op_hash),
            (caller, execute_at),
        );
    }

    /// Executes a previously scheduled administrative operation.
    pub fn execute(env: Env, operation: Operation) {
        let op_hash = env.crypto().sha256(&operation.to_raw(&env));
        let key = DataKey::ScheduledOperation(op_hash.clone());

        let execute_at: u64 = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, SettlementError::OperationNotScheduled));

        if env.ledger().timestamp() < execute_at {
            panic_with_error!(&env, SettlementError::ExecutionNotReady);
        }

        env.storage().persistent().remove(&key);

        match operation {
            Operation::UpdateGovernance(new_gov) => Self::_update_governance(&env, new_gov),
            Operation::CancelRecovery => {
                let admin = read_admin(&env);
                admin.require_auth();
                Self::_cancel_recovery(&env)
            }
            Operation::TransferAdmin(new_admin) => Self::_transfer_admin(&env, new_admin),
            Operation::Upgrade(wasm_hash) => Self::_upgrade(&env, wasm_hash),
            Operation::RegisterMerchant(merchant) => Self::_register_merchant(&env, merchant),
            Operation::UnregisterMerchant(merchant) => Self::_unregister_merchant(&env, merchant),
            Operation::SetSettlementRule(merchant, rule) => {
                Self::_set_settlement_rule(&env, merchant, rule)
            }
            Operation::ClearSettlementRule(merchant) => {
                Self::_clear_settlement_rule(&env, merchant)
            }
            Operation::SetDefaultRule(rule) => Self::_set_default_rule(&env, rule),
        }

        env.events()
            .publish((Symbol::new(&env, "op_executed"), op_hash), ());
    }

    /// Cancels a scheduled administrative operation.
    pub fn cancel(env: Env, caller: Address, operation: Operation) {
        let admin = read_admin(&env);
        if caller != admin {
            panic_with_error!(&env, SettlementError::Unauthorized);
        }
        caller.require_auth();

        let op_hash = env.crypto().sha256(&operation.to_raw(&env));
        let key = DataKey::ScheduledOperation(op_hash.clone());

        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, SettlementError::OperationNotScheduled);
        }

        env.storage().persistent().remove(&key);

        env.events()
            .publish((Symbol::new(&env, "op_cancelled"), op_hash), caller);
    }

    // --- Internal Admin Functions ---

    fn _update_governance(env: &Env, new_governance: Address) {
        assert_not_paused(env);
        validate_governance(env, &new_governance);
        let admin = read_admin(env);
        env.storage()
            .instance()
            .set(&DataKey::Governance, &new_governance);
        env.events().publish(
            (Symbol::new(env, "governance_updated"),),
            (admin, new_governance),
        );
    }

    fn _cancel_recovery(env: &Env) {
        if !env.storage().instance().has(&DataKey::PendingRecovery) {
            panic_with_error!(env, SettlementError::RecoveryNotPending);
        }
        let admin = read_admin(env);
        env.storage().instance().remove(&DataKey::PendingRecovery);
        env.events()
            .publish((Symbol::new(env, "recovery_cancelled"),), admin);
    }

    fn _transfer_admin(env: &Env, new_admin: Address) {
        let admin = read_admin(env);
        validate_nonzero_address(env, &new_admin, SettlementError::EmptyAddress, SettlementError::ZeroAddress);
        if new_admin == admin {
            panic_with_error!(env, SettlementError::InvalidAdmin);
        }
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.events().publish((symbol_short!("admin"),), new_admin);
    }

    fn _upgrade(env: &Env, new_wasm_hash: BytesN<32>) {
        let admin = read_admin(env);
        env.events().publish(
            (
                Symbol::new(env, "contract_upgraded"),
                new_wasm_hash.clone(),
            ),
            admin,
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    fn _register_merchant(env: &Env, merchant: Address) {
        assert_not_paused(env);
        validate_nonzero_address(env, &merchant, SettlementError::EmptyAddress, SettlementError::ZeroAddress);
        let admin = read_admin(env);

        let key = DataKey::Merchant(merchant.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(env, SettlementError::MerchantExists);
        }

        env.storage().persistent().set(&key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&key, MERCHANT_TTL_THRESHOLD, MERCHANT_TTL_BUMP);
        env.events()
            .publish((Symbol::new(env, "merchant_registered"), merchant), admin);
    }

    fn _unregister_merchant(env: &Env, merchant: Address) {
        assert_not_paused(env);
        let admin = read_admin(env);

        let key = DataKey::Merchant(merchant.clone());
        if !env.storage().persistent().has(&key) {
            panic_with_error!(env, SettlementError::MerchantMissing);
        }

        env.storage().persistent().remove(&key);

        let rule_key = DataKey::Rule(merchant.clone());
        let old_rule: Option<SettlementRule> = env.storage().persistent().get(&rule_key);
        if old_rule.is_some() {
            env.storage().persistent().remove(&rule_key);
            env.events().publish(
                (Symbol::new(env, "settlement_rule_cleared"), merchant.clone()),
                (admin.clone(), old_rule.unwrap()),
            );
        }

        env.events().publish(
            (Symbol::new(env, "merchant_unregistered"), merchant),
            admin,
        );
    }

    fn _set_settlement_rule(env: &Env, merchant: Address, rule: SettlementRule) {
        assert_not_paused(env);
        let admin = read_admin(env);

        if !is_merchant_registered_internal(env, merchant.clone()) {
            panic_with_error!(env, SettlementError::MerchantMissing);
        }
        if rule.platform_fee_bps > BPS_DENOMINATOR || rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps < MIN_FEE_BPS || rule.network_fee_bps < MIN_FEE_BPS {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if rule.platform_fee_bps + rule.network_fee_bps > BPS_DENOMINATOR {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::Rule(merchant.clone()))
            .unwrap_or_else(|| read_rule_or_default(env, merchant.clone()));

        let key = DataKey::Rule(merchant.clone());
        env.storage().persistent().set(&key, &rule);
        env.storage()
            .persistent()
            .extend_ttl(&key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);

        env.events().publish(
            (Symbol::new(env, "settlement_rule_updated"), merchant),
            (admin, prev, rule),
        );
    }

    fn _clear_settlement_rule(env: &Env, merchant: Address) {
        assert_not_paused(env);
        let admin = read_admin(env);

        let key = DataKey::Rule(merchant.clone());
        let removed = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&key)
            .unwrap_or_else(|| panic_with_error!(env, SettlementError::MerchantRuleNotSet));

        env.storage().persistent().remove(&key);

        let fallback = read_rule_or_default(env, merchant.clone());

        env.events().publish(
            (Symbol::new(env, "settlement_rule_cleared"), merchant),
            (admin, removed, fallback),
        );
    }

    fn _set_default_rule(env: &Env, new_rule: SettlementRule) {
        assert_not_paused(env);
        let admin = read_admin(env);

        if new_rule.platform_fee_bps > BPS_DENOMINATOR || new_rule.network_fee_bps > BPS_DENOMINATOR
        {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if new_rule.platform_fee_bps < MIN_FEE_BPS || new_rule.network_fee_bps < MIN_FEE_BPS {
            panic_with_error!(env, SettlementError::InvalidFeeBps);
        }
        if new_rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
            panic_with_error!(env, SettlementError::InvalidSettlementDelay);
        }

        let prev = env
            .storage()
            .persistent()
            .get::<_, SettlementRule>(&DataKey::DefaultRule)
            .unwrap_or(BOOTSTRAP_DEFAULT_RULE);

        env.storage()
            .persistent()
            .set(&DataKey::DefaultRule, &new_rule);
        env.storage().persistent().extend_ttl(
            &DataKey::DefaultRule,
            RULE_TTL_THRESHOLD,
            RULE_TTL_BUMP,
        );

        env.events().publish(
            (Symbol::new(env, "default_rule_updated"),),
            (admin, prev, new_rule),
        );
    }
}

/// Reads the configured admin address and refreshes the instance TTL so it remains available.
fn read_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&CommonDataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

fn read_governance(env: &Env) -> Address {
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&DataKey::Governance)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

fn read_recovery_address(env: &Env) -> Address {
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&CommonDataKey::RecoveryAddress)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

fn read_pending_recovery(env: &Env) -> PendingRecovery {
    env.storage()
        .instance()
        .get(&CommonDataKey::PendingRecovery)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::RecoveryNotPending))
}

fn validate_governance(env: &Env, governance: &Address) {
    validate_nonzero_address(
        env,
        governance,
        SettlementError::InvalidGovernance,
        SettlementError::InvalidGovernance,
    );
    let args: Vec<Val> = Vec::new(env);
    let _: Option<FeeConfig> =
        env.invoke_contract(governance, &Symbol::new(env, "get_fee_config"), args);
}

fn validate_nonzero_address(
    env: &Env,
    address: &Address,
    empty_error: SettlementError,
    zero_error: SettlementError,
) {
    if address.to_string().is_empty() {
        panic_with_error!(env, empty_error);
    }
    if storage::is_zero_address(env, address) {
        panic_with_error!(env, zero_error);
    }
}

/// Returns whether a merchant has been registered and keeps the marker entry warm in storage.
fn is_merchant_registered_internal(env: &Env, merchant: Address) -> bool {
    let key = DataKey::Merchant(merchant);
    let exists = env.storage().persistent().has(&key);
    if exists {
        // Keep the merchant marker warm so active merchants do not expire early.
        env.storage()
            .persistent()
            .extend_ttl(&key, MERCHANT_TTL_THRESHOLD, MERCHANT_TTL_BUMP);
    }
    exists
}

/// Resolves the effective settlement rule for a merchant by preferring the merchant-specific override,
/// then falling back to the global default, and finally using the bootstrap fallback.
fn read_rule_or_default(env: &Env, merchant: Address) -> SettlementRule {
    // Merchant-specific rule wins over any shared configuration.
    let merchant_key = DataKey::Rule(merchant);
    if let Some(rule) = env
        .storage()
        .persistent()
        .get::<_, SettlementRule>(&merchant_key)
    {
        env.storage()
            .persistent()
            .extend_ttl(&merchant_key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);
        return rule;
    }
    // Fall back to the admin-controlled global default when present.
    let default_key = DataKey::DefaultRule;
    if let Some(rule) = env
        .storage()
        .persistent()
        .get::<_, SettlementRule>(&default_key)
    {
        env.storage()
            .persistent()
            .extend_ttl(&default_key, RULE_TTL_THRESHOLD, RULE_TTL_BUMP);
        return rule;
    }
    // Protocol fee source: governance's FeeConfig, when available.
    if let Some(rule) = read_governance_fee_rule(env) {
        return rule;
    }
    // Final fallback keeps the contract usable before any config is stored.
    env.events().publish(
        (Symbol::new(env, "bootstrap_fallback"),),
        BOOTSTRAP_DEFAULT_RULE,
    );
    BOOTSTRAP_DEFAULT_RULE
}

/// Attempts to read fee BPS from the configured governance contract.
///
/// Returns `None` when governance has no fee configuration yet or the call
/// fails — callers then continue down the fallback chain to bootstrap.
fn read_governance_fee_rule(env: &Env) -> Option<SettlementRule> {
    let governance: Address = env.storage().instance().get(&DataKey::Governance)?;
    let args: Vec<Val> = Vec::new(env);
    match env.try_invoke_contract::<Option<FeeConfig>, SettlementError>(
        &governance,
        &Symbol::new(env, "get_fee_config"),
        args,
    ) {
        Ok(Ok(Some(config))) => Some(SettlementRule {
            platform_fee_bps: config.platform_fee_bps,
            network_fee_bps: config.network_fee_bps,
            settlement_delay_ledger: 0,
            auto_settle: false,
        }),
        _ => None,
    }
}

/// Ensures the contract is not paused before mutating state or performing privileged actions.
fn assert_not_paused(env: &Env) {
    if storage::is_paused(env) {
        panic_with_error!(env, SettlementError::Paused);
    }
}

/// Computes the platform, network, and merchant fee amounts for an amount using ceil-based rounding.
///
/// # Known edge case: negative merchant amount
///
/// Ceiling rounding of both fees independently can make
/// `platform_fee_amount + network_fee_amount > amount` for small gross amounts
/// (e.g. `amount = 1`, `platform_fee_bps = 5000`, `network_fee_bps = 5000`),
/// which yields a **negative** `merchant_amount`. This is intentional with the
/// current rounding policy (fees are never under-collected); callers must treat
/// a negative merchant payout as a known, documented outcome rather than a bug.
fn calculate_split(env: &Env, amount: i128, rule: &SettlementRule) -> FeeSplit {
    let denom = BPS_DENOMINATOR as i128;

    // Guard against `amount * bps + (denom - 1)` overflowing i128 before it is attempted below,
    // so callers get a readable AmountOverflow error instead of a raw arithmetic-overflow panic.
    // The `denom - 1` term (the ceil-rounding adjustment) is subtracted from the budget up front
    // so the check stays exact at the boundary instead of leaving a narrow window where the
    // multiplication is "safe" but the following `+ denom - 1` still overflows.
    let max_bps = core::cmp::max(rule.platform_fee_bps, rule.network_fee_bps) as i128;
    if max_bps > 0 && amount > (i128::MAX - (denom - 1)) / max_bps {
        panic_with_error!(env, SettlementError::AmountOverflow);
    }

    // Integer arithmetic is used instead of floats to ensure deterministic, reproducible smart contract execution.
    // Standard integer division (`/`) truncates fractions toward zero, causing precision loss and under-collecting fees.
    // To prevent fee under-collection, ceiling division is simulated by adding `BPS_DENOMINATOR - 1` to the numerator.
    // Edge case: For small amounts, ceil rounding can force fees to 1 unit even when the basis points represent a tiny fraction.
    let platform_fee_amount = (amount * (rule.platform_fee_bps as i128) + denom - 1) / denom;
    let network_fee_amount = (amount * (rule.network_fee_bps as i128) + denom - 1) / denom;

    // The merchant amount is calculated as the subtraction remainder of the gross amount minus all rounded-up fees.
    // This ensures the sum of the split amounts (platform fee + network fee + merchant share) always equals the gross amount.
    // Consequence: The merchant absorbs all rounding dust. For very small gross amounts with high/extreme fee percentages,
    // the sum of rounded-up fees can exceed the gross amount, resulting in a negative merchant payout.
    let merchant_amount = amount - platform_fee_amount - network_fee_amount;
    FeeSplit {
        gross_amount: amount,
        platform_fee_amount,
        network_fee_amount,
        merchant_amount,
    }
}

#[cfg(test)]
mod integration_tests;

        let (_contract_id, topics, _data) = events.get(prev_count).unwrap();

        // Topic[0] must be the fixed event-name symbol
        assert_eq!(topics.len(), 2);
        assert_eq!(
            Symbol::from_val(&env, &topics.get(0).unwrap()),
            Symbol::new(&env, "settlement_rule_updated")
        );
        // Topic[1] must be the merchant
        assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);

        // Verify storage was updated
        let stored = client
            .get_settlement_rule(&merchant)
            .expect("expected settlement rule");
        assert_eq!(stored.platform_fee_bps, 200);
        assert_eq!(stored.network_fee_bps, 50);
        assert_eq!(stored.settlement_delay_ledger, 20);
        assert!(stored.auto_settle);
    }

    #[test]
    fn stores_payment_reference_once_and_calculates_split() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference = BytesN::from_array(&env, &[7; 32]);
        let before = env.events().all().len();
        let split = client.store_payment_reference(&merchant, &reference, &20_000);
        let stored = client
            .get_payment_reference(&reference)
            .expect("expected payment record");

        assert_eq!(split.platform_fee_amount, 500);
        assert_eq!(split.network_fee_amount, 100);
        assert_eq!(split.merchant_amount, 19_400);
        assert_eq!(stored.platform_fee_bps, 250);
        assert_eq!(stored.network_fee_bps, 50);
        assert_eq!(stored.amount, 20_000);
        assert!(env.events().all().len() >= before + 2);
    }

    #[test]
    #[should_panic]
    fn rejects_all_zero_payment_reference() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference = BytesN::from_array(&env, &[0; 32]);
        client.store_payment_reference(&merchant, &reference, &10_000);
    }

    #[test]
    fn reads_payment_reference_and_extends_ttl() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference = BytesN::from_array(&env, &[8; 32]);
        client.store_payment_reference(&merchant, &reference, &10_000);

        // Call get_payment_reference, which should extend the TTL
        let stored = client
            .get_payment_reference(&reference)
            .expect("expected payment record");

        assert_eq!(stored.amount, 10_000);

        // Verify the persistent entry exists after read
        env.as_contract(&client.address, || {
            let key = DataKey::Payment(reference.clone());
            assert!(env.storage().persistent().has(&key));
        });
    }

    #[test]
    fn get_payment_reference_returns_none_for_unknown() {
        let (env, client, _, _) = setup();
        let unknown_ref = BytesN::from_array(&env, &[0xab; 32]);
        let result = client.get_payment_reference(&unknown_ref);
        assert!(result.is_none());
    }

    #[test]
    fn gets_payments_in_batches() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let rule = SettlementRule {
            platform_fee_bps: 250,
            network_fee_bps: 50,
            settlement_delay_ledger: 0,
            auto_settle: false,
        };
        client.set_settlement_rule(&merchant, &rule);

        let reference_one = BytesN::from_array(&env, &[11; 32]);
        let reference_two = BytesN::from_array(&env, &[12; 32]);
        client.store_payment_reference(&merchant, &reference_one, &15_000);
        client.store_payment_reference(&merchant, &reference_two, &25_000);

        let references = Vec::from_array(&env, [reference_one.clone(), reference_two.clone()]);
        let payments = client.get_payments(&references);

        assert_eq!(payments.len(), 2);
        assert_eq!(payments.get(0).unwrap().unwrap().amount, 15_000);
        assert_eq!(payments.get(1).unwrap().unwrap().amount, 25_000);
    }

    // Issue #298: verify get_payments returns an empty vector when given an empty input vector.
    #[test]
    fn get_payments_with_empty_input_vector_returns_empty_vector() {
        let (env, client, _admin, _merchant) = setup();
        let references = Vec::new(&env);
        let payments = client.get_payments(&references);
        assert_eq!(payments.len(), 0);
    }

    // Issue #299: verify get_payments returns an empty vector when all requested references are missing from storage.
    #[test]
    fn get_payments_with_all_missing_references_returns_empty_vector() {
        let (env, client, _admin, _merchant) = setup();
        let missing_one = BytesN::from_array(&env, &[90; 32]);
        let missing_two = BytesN::from_array(&env, &[91; 32]);
        let references = Vec::from_array(&env, [missing_one, missing_two]);
        let payments = client.get_payments(&references);
        assert_eq!(payments.len(), 2);
        assert!(payments.get(0).unwrap().is_none());
        assert!(payments.get(1).unwrap().is_none());
    }

    // Issue #300: verify get_payments correctly filters out missing references and returns records for valid ones.
    #[test]
    fn get_payments_with_mixed_valid_and_missing_references() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        let valid_one = BytesN::from_array(&env, &[80; 32]);
        let valid_two = BytesN::from_array(&env, &[81; 32]);
        let missing_ref = BytesN::from_array(&env, &[82; 32]);

        client.store_payment_reference(&merchant, &valid_one, &10_000);
        client.store_payment_reference(&merchant, &valid_two, &20_000);

        // Query with: [valid_one, missing_ref, valid_two]
        let references = Vec::from_array(&env, [valid_one.clone(), missing_ref, valid_two.clone()]);
        let payments = client.get_payments(&references);

        assert_eq!(payments.len(), 3);
        assert_eq!(payments.get(0).unwrap().unwrap().amount, 10_000);
        assert!(payments.get(1).unwrap().is_none());
        assert_eq!(payments.get(2).unwrap().unwrap().amount, 20_000);
    }

    // Issue #340: get_payments must still return every requested payment, in the
    // requested order, for a batch large enough to trigger multiple Vec growths
    // (regardless of whether the underlying Vec is pre-allocated).
    #[test]
    fn get_payments_returns_all_records_in_order_for_large_batch() {
        let (env, client, _admin, merchant) = setup();
        client.register_merchant(&merchant);

        const BATCH_SIZE: u8 = 20;
        let mut references = Vec::new(&env);
        for i in 1..=BATCH_SIZE {
            let reference = BytesN::from_array(&env, &[i; 32]);
            let amount = MIN_PAYMENT_AMOUNT + i as i128;
            client.store_payment_reference(&merchant, &reference, &amount);
            references.push_back(reference);
        }

        let payments = client.get_payments(&references);

        assert_eq!(payments.len(), BATCH_SIZE as u32);
        for i in 1..=BATCH_SIZE {
            assert_eq!(
                payments.get((i - 1) as u32).unwrap().unwrap().amount,
                MIN_PAYMENT_AMOUNT + i as i128
            );
        }
    }
