//! Tests for merchant management entry points:
//! `register_merchant`, `unregister_merchant`.

use crate::*;
use soroban_sdk::testutils::{Address as _, Events, MockAuth, MockAuthInvoke};
use soroban_sdk::{FromVal, IntoVal};

use super::{register_governance, setup};

// ---------------------------------------------------------------------------
// register_merchant
// ---------------------------------------------------------------------------

#[test]
fn registers_merchant_and_persists_flag() {
    let (env, client, _admin, merchant) = setup();
    let before = env.events().all().len();
    client.register_merchant(&merchant);
    assert!(client.is_merchant_registered(&merchant));
    assert!(env.events().all().len() > before);
}

#[test]
fn emits_event_on_registration() {
    let (env, client, admin, merchant) = setup();

    client.register_merchant(&merchant);

    let events = env.events().all();
    let event = events.last().unwrap();
    let (_contract_id, topics, data) = event;

    // Topic 0: Event Name symbol
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "merchant_registered")
    );
    // Topic 1: Merchant Address
    assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
    // Data: Admin Address (the caller)
    assert_eq!(Address::from_val(&env, &data), admin);
}

#[test]
#[should_panic]
fn rejects_invalid_merchant_address() {
    let (env, client, _admin, _merchant) = setup();
    let zero_address = Address::from_string(&soroban_sdk::String::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));
    client.register_merchant(&zero_address);
}

#[test]
#[should_panic]
fn rejects_duplicate_merchant() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    client.register_merchant(&merchant);
}

// Issue #77: verify duplicate merchant registration fails with MerchantExists
#[test]
#[should_panic]
fn duplicate_merchant_registration_fails() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    client.register_merchant(&merchant);
}

// Issue #76: verify only admin can register merchants
#[test]
#[should_panic]
fn register_merchant_requires_admin_auth() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let non_admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    let governance = register_governance(&env);
    let recovery_address = Address::generate(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    env.mock_all_auths();
    client.init(&admin, &governance, &recovery_address);
    env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "register_merchant",
            args: soroban_sdk::vec![&env, merchant.clone().into_val(&env)],
            sub_invokes: &[],
        },
    }]);
    client.register_merchant(&merchant);
}

// ---------------------------------------------------------------------------
// unregister_merchant
// ---------------------------------------------------------------------------

#[test]
fn unregisters_merchant_and_cleans_up() {
    let (env, client, admin, merchant) = setup();
    client.register_merchant(&merchant);

    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);

    assert!(client.is_merchant_registered(&merchant));
    assert!(client.get_settlement_rule(&merchant).is_some());

    let before = env.events().all().len();
    client.unregister_merchant(&merchant);

    assert!(!client.is_merchant_registered(&merchant));
    assert!(client.get_settlement_rule(&merchant).is_none());
    // Two events: settlement_rule_cleared then merchant_unregistered
    assert_eq!(env.events().all().len(), before + 2);

    let events = env.events().all();
    let (_, topics, data) = events.get(before).unwrap();
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "settlement_rule_cleared")
    );
    assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
    let (event_admin, removed): (Address, SettlementRule) = FromVal::from_val(&env, &data);
    assert_eq!(event_admin, admin);
    assert_eq!(removed.platform_fee_bps, rule.platform_fee_bps);
    assert_eq!(removed.network_fee_bps, rule.network_fee_bps);
}

#[test]
fn emits_structured_event_when_unregistering_merchant() {
    let (env, client, admin, merchant) = setup();
    client.register_merchant(&merchant);

    client.unregister_merchant(&merchant);

    let events = env.events().all();
    let event = events.last().unwrap();
    let (_contract_id, topics, data) = event;

    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "merchant_unregistered")
    );
    assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
    assert_eq!(Address::from_val(&env, &data), admin);
}

#[test]
#[should_panic]
fn unregister_rejects_missing_merchant() {
    let (_env, client, _admin, merchant) = setup();
    client.unregister_merchant(&merchant);
}

// Issue #10: clear_settlement_rule must fail after unregister has removed the rule
#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn clear_settlement_rule_fails_after_unregister_removes_rule() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
    assert!(client.get_settlement_rule(&merchant).is_some());

    // Unregister silently removes the merchant-specific rule.
    client.unregister_merchant(&merchant);
    assert!(client.get_settlement_rule(&merchant).is_none());

    // The rule no longer exists, so clear_settlement_rule must panic with RuleNotSet.
    client.clear_settlement_rule(&merchant);
}
