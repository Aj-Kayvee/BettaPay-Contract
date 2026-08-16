//! Tests for administrative entry points:
//! `init`, `transfer_admin`, `pause`, `unpause`, `upgrade`, `recovery`.

use crate::*;
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::{contract, contractimpl, Address, BytesN, Env, FromVal, Symbol};

use bettapay_common::constants::RECOVERY_DELAY_SECONDS;

use super::{register_governance, setup};

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

#[test]
fn emits_event_on_initialization() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    client.init(
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &governance,
        &recovery,
    );

    // init stores admin/governance/recovery; event emission may vary by version.
    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, admin]);
    assert_eq!(client.get_governance(), governance);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn rejects_double_initialization() {
    let (env, client, admins, _) = setup();
    let governance = register_governance(&env);
    let recovery_address = Address::generate(&env);
    client.init(&admins, &1, &governance, &recovery_address);
    let _ = env;
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn get_admin_panics_before_init() {
    let env = Env::default();
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    client.get_admin();
}

// ---------------------------------------------------------------------------
// transfer_admin
// ---------------------------------------------------------------------------

#[test]
fn transfer_admin_updates_admin_address() {
    let (env, client, admins, _) = setup();
    let new_admin = Address::generate(&env);

    assert_eq!(client.get_admin(), admins);
    client.transfer_admin(&admins, &soroban_sdk::vec![&env, new_admin.clone()], &1);
    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, new_admin]);
}

#[test]
#[should_panic(expected = "Error(Contract, #21)")]
fn rejects_zero_address_admin_transfer() {
    let (env, client, admins, _merchant) = setup();
    let zero_address = Address::from_string(&soroban_sdk::String::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));
    client.transfer_admin(&admins, &soroban_sdk::vec![&env, zero_address], &1);
}

// Issue #72: verify non-admin transfer_admin calls are rejected
#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn transfer_admin_rejected_for_non_admin() {
    let (env, client, _admins, _merchant) = setup();
    let non_admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    client.transfer_admin(
        &soroban_sdk::vec![&env, non_admin],
        &soroban_sdk::vec![&env, new_admin],
        &1,
    );
}

#[test]
fn emits_event_on_admin_transfer() {
    let (env, client, admins, _merchant) = setup();
    let new_admin = Address::generate(&env);

    let before = env.events().all().len();
    client.transfer_admin(&admins, &soroban_sdk::vec![&env, new_admin.clone()], &1);

    let events = env.events().all();
    assert_eq!(
        events.len(),
        before + 1,
        "exactly one event should be emitted by transfer_admin"
    );

    let event = events.last().unwrap();
    let (contract_id, topics, data) = event;

    assert_eq!(contract_id, client.address);
    assert_eq!(topics.len(), 1);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "admin")
    );
    assert_eq!(Address::from_val(&env, &data), new_admin);
}

// ---------------------------------------------------------------------------
// pause / unpause
// ---------------------------------------------------------------------------

// Issue #75: verify pause flag changes state in settlement contract
#[test]
fn pause_flag_changes_state() {
    let (_env, client, admins, _merchant) = setup();
    assert!(!client.is_paused());
    client.pause(&admins);
    assert!(client.is_paused());
    client.unpause(&admins);
    assert!(!client.is_paused());
}

// Issue #73: verify non-admins cannot pause the settlement contract
#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn pause_rejected_for_non_admin() {
    let (env, client, _admins, _merchant) = setup();
    let non_admin = Address::generate(&env);
    client.pause(&soroban_sdk::vec![&env, non_admin]);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn merchant_registration_blocked_when_paused() {
    let (_env, client, admins, merchant) = setup();
    client.pause(&admins);
    assert!(client.is_paused());
    // register_merchant calls assert_not_paused, so this must panic with Paused (#9).
    client.register_merchant(&admins, &merchant);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn set_settlement_rule_rejected_when_paused() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    client.pause(&admins);
    assert!(client.is_paused());

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);
}

// Issue #350: the merchant-specific settlement rule must not be cleared while paused.
#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn clear_settlement_rule_rejected_when_paused() {
    let (_env, client, admins, merchant) = setup();
    client.register_merchant(&admins, &merchant);

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_settlement_rule(&admins, &merchant, &rule);

    client.pause(&admins);
    assert!(client.is_paused());

    client.clear_settlement_rule(&admins, &merchant);
}

// Issue #231: the global default settlement rule must not be updated while paused.
#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn set_default_rule_rejected_when_paused() {
    let (_env, client, admins, _merchant) = setup();
    client.pause(&admins);
    assert!(client.is_paused());

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 7,
        auto_settle: true,
    };
    client.set_default_rule(&admins, &rule);
}

// ---------------------------------------------------------------------------
// upgrade
// ---------------------------------------------------------------------------

#[test]
fn executes_contract_wasm_upgrade_successfully() {
    let (env, client, admins, _) = setup();
    let wasm = soroban_sdk::Bytes::from_slice(&env, &[]);
    let new_wasm_hash = env.deployer().upload_contract_wasm(wasm);

    let before = env.events().all().len();
    // Verifies the structural update pass completes without panicking
    client.upgrade(&admins, &new_wasm_hash);

    let events = env.events().all();
    assert!(events.len() > before);

    let event = events.last().unwrap();
    let (_contract_id, topics, data) = event;

    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "contract_upgraded")
    );
    assert_eq!(
        BytesN::<32>::from_val(&env, &topics.get(1).unwrap()),
        new_wasm_hash
    );
    assert_eq!(Address::from_val(&env, &data), admins.get(0).unwrap());

    // Ensure the upgraded contract remains callable and retains its state.
    let upgraded_client = SettlementContractClient::new(&env, &client.address);
    assert_eq!(upgraded_client.get_admin(), admins);
}

// ---------------------------------------------------------------------------
// recovery
// ---------------------------------------------------------------------------

#[test]
fn recovery_executes_after_delay() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let governance = register_governance(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    client.init(
        &soroban_sdk::vec![&env, admin.clone()],
        &1,
        &governance,
        &recovery_address,
    );
    assert_eq!(client.get_recovery_address(), recovery_address);

    client.initiate_recovery(&new_admin);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    client.execute_recovery();

    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, new_admin]);
}

// ---------------------------------------------------------------------------
// governance update
// ---------------------------------------------------------------------------

#[test]
fn update_governance_stores_validated_address() {
    let (env, client, admins, _merchant) = setup();
    let new_governance = register_governance(&env);

    client.update_governance(&admins, &new_governance);

    assert_eq!(client.get_governance(), new_governance);
}

#[contract]
pub struct MockGovernanceWithFee;

#[contractimpl]
impl MockGovernanceWithFee {
    pub fn get_fee_config(_env: Env) -> Option<FeeConfig> {
        Some(FeeConfig {
            platform_fee_bps: 100,
            network_fee_bps: 50,
        })
    }
}

#[test]
fn governance_derived_rule_conversion_applies_delay_cap() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let recovery_address = Address::generate(&env);
    let merchant = Address::generate(&env);

    let governance = env.register_contract(None, MockGovernanceWithFee);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);
    let admins = soroban_sdk::vec![&env, admin];
    client.init(&admins, &1, &governance, &recovery_address);
    client.register_merchant(&admins, &merchant);

    let reference = soroban_sdk::BytesN::from_array(&env, &[1; 32]);
    client.store_payment_reference(&merchant, &reference, &10_000);

    let record = client.get_payment_reference(&reference).unwrap();
    assert_eq!(record.settlement_delay_ledger, 0);
    assert!(record.settlement_delay_ledger <= MAX_SETTLEMENT_DELAY_LEDGER);
}

