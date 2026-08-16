use soroban_sdk::contracterror;

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
    /// Either fee BPS value is below `MIN_FEE_BPS` (5) or above `MAX_FEE_BPS`
    /// (5 000), or their sum exceeds `BPS_DENOMINATOR` (10 000).
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
    /// The deployed WASM does not implement the required interface.
    InvalidWasmInterface = 25,
    /// The provided multisig threshold is invalid.
    InvalidThreshold = 26,
    GovernanceCallFailed = 27,
    FeeExceedsGovernanceConfig = 28,
    AmountTooSmall = 29,
    AmountZero = 30,
    AmountNegative = 31,
}
