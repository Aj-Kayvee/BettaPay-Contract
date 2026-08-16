use soroban_sdk::{panic_with_error, Address, Env, Symbol, TryFromVal, TryIntoVal, Val, Vec};

use bettapay_common::{
    events::PendingRecovery,
    storage::{self, CommonDataKey},
};

use crate::errors::SettlementError;
use crate::types::{DataKey, FeeConfig, SettlementRule};
use crate::{
    BOOTSTRAP_DEFAULT_RULE, MAX_SETTLEMENT_DELAY_LEDGER, MERCHANT_TTL_BUMP, MERCHANT_TTL_THRESHOLD,
    READ_INSTANCE_TTL_BUMP, READ_INSTANCE_TTL_THRESHOLD, RULE_TTL_BUMP, RULE_TTL_THRESHOLD,
};

pub(crate) fn read_admins(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

pub(crate) fn read_admin(env: &Env) -> Address {
    read_admins(env).get(0).unwrap()
}

pub(crate) fn read_threshold(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::Threshold)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

pub(crate) fn validate_admins_and_threshold(env: &Env, admins: &Vec<Address>, threshold: u32) {
    if threshold == 0 || threshold > admins.len() {
        panic_with_error!(env, SettlementError::InvalidThreshold);
    }
    if admins.is_empty() {
        panic_with_error!(env, SettlementError::InvalidAdmin);
    }
    for i in 0..admins.len() {
        let admin = admins.get(i).unwrap();
        validate_nonzero_address(
            env,
            &admin,
            SettlementError::EmptyAddress,
            SettlementError::ZeroAddress,
        );
        for j in (i + 1)..admins.len() {
            if admin == admins.get(j).unwrap() {
                panic_with_error!(env, SettlementError::InvalidAdmin);
            }
        }
    }
}

pub(crate) fn verify_admin_auth(env: &Env, signers: &Vec<Address>, required_count: u32) {
    let admins = read_admins(env);
    if signers.len() < required_count {
        panic_with_error!(env, SettlementError::Unauthorized);
    }
    for i in 0..signers.len() {
        let signer = signers.get(i).unwrap();
        let mut is_admin = false;
        for j in 0..admins.len() {
            if signer == admins.get(j).unwrap() {
                is_admin = true;
                break;
            }
        }
        if !is_admin {
            panic_with_error!(env, SettlementError::Unauthorized);
        }
        for j in (i + 1)..signers.len() {
            if signer == signers.get(j).unwrap() {
                panic_with_error!(env, SettlementError::Unauthorized);
            }
        }
        signer.require_auth();
    }
}

pub(crate) fn read_governance(env: &Env) -> Address {
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&DataKey::Governance)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

pub(crate) fn read_recovery_address(env: &Env) -> Address {
    env.storage()
        .instance()
        .extend_ttl(READ_INSTANCE_TTL_THRESHOLD, READ_INSTANCE_TTL_BUMP);
    env.storage()
        .instance()
        .get(&CommonDataKey::RecoveryAddress)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::NotInitialized))
}

pub(crate) fn read_pending_recovery(env: &Env) -> PendingRecovery {
    env.storage()
        .instance()
        .get::<_, PendingRecovery>(&CommonDataKey::PendingRecovery)
        .unwrap_or_else(|| panic_with_error!(env, SettlementError::RecoveryNotPending))
}

pub(crate) fn validate_governance(env: &Env, governance: &Address) {
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

pub(crate) fn validate_nonzero_address(
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
pub(crate) fn is_merchant_registered_internal(env: &Env, merchant: Address) -> bool {
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
pub(crate) fn read_rule_or_default(env: &Env, merchant: Address) -> SettlementRule {
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
pub(crate) fn read_governance_fee_rule(env: &Env) -> Option<SettlementRule> {
    let governance: Address = env.storage().instance().get(&DataKey::Governance)?;
    let args: Vec<Val> = Vec::new(env);
    match env.try_invoke_contract::<Option<FeeConfig>, SettlementError>(
        &governance,
        &Symbol::new(env, "get_fee_config"),
        args,
    ) {
        Ok(Ok(Some(config))) => {
            let rule = SettlementRule {
                platform_fee_bps: config.platform_fee_bps,
                network_fee_bps: config.network_fee_bps,
                settlement_delay_ledger: 0,
                auto_settle: false,
            };
            if rule.settlement_delay_ledger > MAX_SETTLEMENT_DELAY_LEDGER {
                panic_with_error!(env, SettlementError::InvalidSettlementDelay);
            }
            Some(rule)
        }
        _ => None,
    }
}

/// Ensures the contract is not paused before mutating state or performing privileged actions.
pub(crate) fn assert_not_paused(env: &Env) {
    if storage::is_paused(env) {
        panic_with_error!(env, SettlementError::Paused);
    }
}

/// Reads the governance FeeConfig via cross-contract call and validates that
/// the settlement rule fees do not exceed governance's configured ceilings.
///
/// When governance has no fee config set, local hardcoded constants still
/// apply as baseline — this function only enforces ceilings that governance
/// has explicitly configured.
pub(crate) fn validate_fee_against_governance(env: &Env, rule: &SettlementRule) {
    let governance: Address = read_governance(env);
    let fee_config: Val = env.invoke_contract(
        &governance,
        &Symbol::new(env, "get_fee_config"),
        Vec::new(env),
    );

    // If governance returned a valid config, check fee ceilings.
    // If it returned `()` (no config set), there is no ceiling to enforce.
    if fee_config.is_void() {
        return;
    }

    let governance_fee: Vec<Val> = fee_config
        .try_into_val(env)
        .unwrap_or_else(|_| panic_with_error!(env, SettlementError::GovernanceCallFailed));

    // Governance FeeConfig tuple is (platform_fee_bps: u32, network_fee_bps: u32)
    if let Some(gov_platform_val) = governance_fee.get(0) {
        let gov_platform_bps: u32 = u32::try_from_val(env, &gov_platform_val)
            .unwrap_or_else(|_| panic_with_error!(env, SettlementError::GovernanceCallFailed));
        if rule.platform_fee_bps > gov_platform_bps {
            panic_with_error!(env, SettlementError::FeeExceedsGovernanceConfig);
        }
    }

    if let Some(gov_network_val) = governance_fee.get(1) {
        let gov_network_bps: u32 = u32::try_from_val(env, &gov_network_val)
            .unwrap_or_else(|_| panic_with_error!(env, SettlementError::GovernanceCallFailed));
        if rule.network_fee_bps > gov_network_bps {
            panic_with_error!(env, SettlementError::FeeExceedsGovernanceConfig);
        }
    }
}
