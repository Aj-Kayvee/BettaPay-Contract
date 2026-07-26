//! Integration tests exercising settlement ↔ governance cross-contract fee flow.
//!
//! Covers issue #293: deploy both contracts, wire governance into settlement,
//! set fee config on governance, and verify settlement fee splits honor it.

use super::*;
use governance_contract::{FeeConfig, GovernanceContract, GovernanceContractClient};
use soroban_sdk::testutils::Address as _;

fn deploy_governance(env: &Env, admin: &Address) -> GovernanceContractClient<'static> {
    let gov_id = env.register_contract(None, GovernanceContract);
    let client = GovernanceContractClient::new(env, &gov_id);
    client.init(admin);
    client
}

fn deploy_settlement(env: &Env, admin: &Address) -> SettlementContractClient<'static> {
    let id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(env, &id);
    client.init(admin);
    client
}

/// Happy path: governance fee config drives settlement's `store_payment_reference` split.
#[test]
fn settlement_fee_split_respects_governance_fee_config() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);

    let gov = deploy_governance(&env, &admin);
    let settlement = deploy_settlement(&env, &admin);
    settlement.set_governance(&gov.address);

    // Governance MIN_FEE_BPS is 5; use values inside the allowed range.
    let config = FeeConfig {
        platform_fee_bps: 250,
        network_fee_bps: 50,
    };
    gov.set_fee_config(&admin, &config);

    settlement.register_merchant(&merchant);
    let reference = BytesN::from_array(&env, &[7; 32]);
    let amount: i128 = 20_000;
    let split = settlement.store_payment_reference(&merchant, &reference, &amount);

    // ceil(20000 * 250 / 10000) = 500, ceil(20000 * 50 / 10000) = 100
    assert_eq!(split.gross_amount, amount);
    assert_eq!(split.platform_fee_amount, 500);
    assert_eq!(split.network_fee_amount, 100);
    assert_eq!(split.merchant_amount, 19_400);

    let record = settlement
        .get_payment_reference(&reference)
        .expect("payment should be stored");
    assert_eq!(record.platform_fee_bps, 250);
    assert_eq!(record.network_fee_bps, 50);
}

/// Changing governance fees must change subsequent settlement splits.
#[test]
fn settlement_reflects_updated_governance_fees() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);

    let gov = deploy_governance(&env, &admin);
    let settlement = deploy_settlement(&env, &admin);
    settlement.set_governance(&gov.address);
    settlement.register_merchant(&merchant);

    gov.set_fee_config(
        &admin,
        &FeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 25,
        },
    );
    let first = settlement.calculate_fee_split(&merchant, &10_000);
    assert_eq!(first.platform_fee_amount, 100);
    assert_eq!(first.network_fee_amount, 25);
    assert_eq!(first.merchant_amount, 9_875);

    gov.set_fee_config(
        &admin,
        &FeeConfig {
            platform_fee_bps: 400,
            network_fee_bps: 100,
        },
    );
    let second = settlement.calculate_fee_split(&merchant, &10_000);
    assert_eq!(second.platform_fee_amount, 400);
    assert_eq!(second.network_fee_amount, 100);
    assert_eq!(second.merchant_amount, 9_500);
}

/// Governance address points at a non-contract → fall back to bootstrap fees.
#[test]
fn settlement_falls_back_when_governance_not_deployed() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    let fake_governance = Address::generate(&env);

    let settlement = deploy_settlement(&env, &admin);
    settlement.set_governance(&fake_governance);
    settlement.register_merchant(&merchant);

    let split = settlement.calculate_fee_split(&merchant, &10_000);
    // BOOTSTRAP_DEFAULT_RULE: 100 bps platform, 0 network
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 0);
    assert_eq!(split.merchant_amount, 9_900);
}

/// Governance deployed but never initialized / no fee config → bootstrap fallback.
#[test]
fn settlement_falls_back_when_governance_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Register governance Wasm but do not call init / set_fee_config.
    let gov_id = env.register_contract(None, GovernanceContract);
    let settlement = deploy_settlement(&env, &admin);
    settlement.set_governance(&gov_id);
    settlement.register_merchant(&merchant);

    let split = settlement.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 0);
    assert_eq!(split.merchant_amount, 9_900);
}

/// Merchant-specific rule still wins over governance fee config.
#[test]
fn merchant_rule_overrides_governance_fee_config() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);

    let gov = deploy_governance(&env, &admin);
    let settlement = deploy_settlement(&env, &admin);
    settlement.set_governance(&gov.address);
    gov.set_fee_config(
        &admin,
        &FeeConfig {
            platform_fee_bps: 250,
            network_fee_bps: 50,
        },
    );

    settlement.register_merchant(&merchant);
    settlement.set_settlement_rule(
        &merchant,
        &SettlementRule {
            platform_fee_bps: 500,
            network_fee_bps: 100,
            settlement_delay_ledger: 0,
            auto_settle: false,
        },
    );

    let split = settlement.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 500);
    assert_eq!(split.network_fee_amount, 100);
    assert_eq!(split.merchant_amount, 9_400);
}

/// Local default rule takes precedence over governance fee config.
#[test]
fn default_rule_overrides_governance_fee_config() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);

    let gov = deploy_governance(&env, &admin);
    let settlement = deploy_settlement(&env, &admin);
    settlement.set_governance(&gov.address);
    gov.set_fee_config(
        &admin,
        &FeeConfig {
            platform_fee_bps: 250,
            network_fee_bps: 50,
        },
    );
    settlement.set_default_rule(&SettlementRule {
        platform_fee_bps: 150,
        network_fee_bps: 25,
        settlement_delay_ledger: 3,
        auto_settle: true,
    });

    settlement.register_merchant(&merchant);
    let split = settlement.calculate_fee_split(&merchant, &10_000);
    // Default rule (150/25), not governance (250/50).
    assert_eq!(split.platform_fee_amount, 150);
    assert_eq!(split.network_fee_amount, 25);
    assert_eq!(split.merchant_amount, 9_825);
}
