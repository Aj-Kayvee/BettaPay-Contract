//! Integration tests exercising settlement ↔ governance cross-contract fee flow.
//!
//! Covers issue #293: deploy a governance-shaped contract alongside settlement,
//! wire it at init, set fee config, and verify settlement fee splits honor it.
//!
//! A lightweight mock is used (same `FeeConfig` layout / `get_fee_config` ABI as
//! governance) so these tests do not depend on compiling the governance crate as
//! an `rlib`, while still exercising the cross-contract invoke path.

use super::*;
use soroban_sdk::testutils::Address as _;

#[contract]
struct MockGovernanceWithFees;

#[contractimpl]
impl MockGovernanceWithFees {
    pub fn init(env: Env, config: FeeConfig) {
        env.storage().instance().set(&symbol_short!("fee"), &config);
    }

    pub fn set_fee_config(env: Env, config: FeeConfig) {
        env.storage().instance().set(&symbol_short!("fee"), &config);
    }

    pub fn get_fee_config(env: Env) -> Option<FeeConfig> {
        env.storage().instance().get(&symbol_short!("fee"))
    }
}

fn deploy_governance(env: &Env, config: Option<FeeConfig>) -> Address {
    let gov_id = env.register_contract(None, MockGovernanceWithFees);
    let client = MockGovernanceWithFeesClient::new(env, &gov_id);
    if let Some(cfg) = config {
        client.init(&cfg);
    }
    gov_id
}

fn deploy_settlement(
    env: &Env,
    admin: &Address,
    governance: &Address,
    recovery: &Address,
) -> SettlementContractClient<'static> {
    let id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(env, &id);
    client.init(admin, governance, recovery);
    client
}

/// Happy path: governance fee config drives settlement's `store_payment_reference` split.
#[test]
fn settlement_fee_split_respects_governance_fee_config() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let config = FeeConfig {
        platform_fee_bps: 250,
        network_fee_bps: 50,
    };
    let gov = deploy_governance(&env, Some(config));
    let settlement = deploy_settlement(&env, &admin, &gov, &recovery);

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
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let gov = deploy_governance(
        &env,
        Some(FeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 25,
        }),
    );
    let gov_client = MockGovernanceWithFeesClient::new(&env, &gov);
    let settlement = deploy_settlement(&env, &admin, &gov, &recovery);
    settlement.register_merchant(&merchant);

    let first = settlement.calculate_fee_split(&merchant, &10_000);
    assert_eq!(first.platform_fee_amount, 100);
    assert_eq!(first.network_fee_amount, 25);
    assert_eq!(first.merchant_amount, 9_875);

    gov_client.set_fee_config(&FeeConfig {
        platform_fee_bps: 400,
        network_fee_bps: 100,
    });
    let second = settlement.calculate_fee_split(&merchant, &10_000);
    assert_eq!(second.platform_fee_amount, 400);
    assert_eq!(second.network_fee_amount, 100);
    assert_eq!(second.merchant_amount, 9_500);
}

/// Non-contract governance address is rejected at init (validate_governance).
#[test]
#[should_panic]
fn settlement_rejects_governance_not_deployed_at_init() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let fake_governance = Address::generate(&env);

    let id = env.register_contract(None, SettlementContract);
    let settlement = SettlementContractClient::new(&env, &id);
    settlement.init(&admin, &fake_governance, &recovery);
}

/// Governance deployed but with no fee config → bootstrap fee fallback.
#[test]
fn settlement_falls_back_when_governance_has_no_fee_config() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    // Governance contract registered (passes validate_governance) but no fee set.
    let gov = deploy_governance(&env, None);
    let settlement = deploy_settlement(&env, &admin, &gov, &recovery);
    settlement.register_merchant(&merchant);

    let split = settlement.calculate_fee_split(&merchant, &10_000);
    // BOOTSTRAP_DEFAULT_RULE: 100 bps platform, 0 network
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
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let gov = deploy_governance(
        &env,
        Some(FeeConfig {
            platform_fee_bps: 250,
            network_fee_bps: 50,
        }),
    );
    let settlement = deploy_settlement(&env, &admin, &gov, &recovery);

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

/// Local default rule overrides governance fee config.
#[test]
fn default_rule_overrides_governance_fee_config() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let merchant = Address::generate(&env);

    let gov = deploy_governance(
        &env,
        Some(FeeConfig {
            platform_fee_bps: 250,
            network_fee_bps: 50,
        }),
    );
    let settlement = deploy_settlement(&env, &admin, &gov, &recovery);

    settlement.register_merchant(&merchant);
    settlement.set_default_rule(&SettlementRule {
        platform_fee_bps: 175,
        network_fee_bps: 25,
        settlement_delay_ledger: 0,
        auto_settle: false,
    });

    let split = settlement.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 175);
    assert_eq!(split.network_fee_amount, 25);
    assert_eq!(split.merchant_amount, 9_800);
}
