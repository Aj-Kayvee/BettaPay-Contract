//! Indexer-conformity tests.
//!
//! An off-chain indexer that consumes events from both BettaPay contracts must
//! be able to dispatch on a single canonical topic name and decode the payload
//! with one decoder, no matter which contract published it. These tests deploy
//! the settlement and governance contracts into the same environment, trigger
//! the shared events on each, and assert that both contracts publish the same
//! topic symbol and the same canonical payload shape (see issue #518).

use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::{Address, Env, FromVal, Symbol, TryFromVal, Val, Vec};

use bettapay_common::constants::RECOVERY_DELAY_SECONDS;
use bettapay_common::events::{
    AdminTransferred, ADMIN_TRANSFERRED_EVENT, PAUSED_EVENT, RECOVERY_EXECUTED_EVENT,
    UNPAUSED_EVENT,
};

use governance_contract::{GovernanceContract, GovernanceContractClient};

use crate::{SettlementContract, SettlementContractClient};

/// Deploys a fresh governance contract and a settlement contract configured to
/// use it, returning both clients plus each contract's admin set.
fn deploy_both_contracts() -> (
    Env,
    SettlementContractClient<'static>,
    GovernanceContractClient<'static>,
    Vec<Address>,
    Vec<Address>,
) {
    let env = Env::default();
    env.mock_all_auths();

    let recovery = Address::generate(&env);

    let gov_admin = Address::generate(&env);
    let gov_admins = soroban_sdk::vec![&env, gov_admin.clone()];
    let gov_id = env.register_contract(None, GovernanceContract);
    let gov = GovernanceContractClient::new(&env, &gov_id);
    gov.init(&gov_admins, &1, &recovery);

    let settlement_admin = Address::generate(&env);
    let settlement_admins = soroban_sdk::vec![&env, settlement_admin.clone()];
    let settlement_id = env.register_contract(None, SettlementContract);
    let settlement = SettlementContractClient::new(&env, &settlement_id);
    settlement.init(&settlement_admins, &1, &gov_id, &recovery);

    (env, settlement, gov, settlement_admins, gov_admins)
}

/// Returns the data payload of the event published by `contract` whose single
/// topic equals `Symbol(topic)`.
fn event_data(
    env: &Env,
    events: &Vec<(Address, Vec<Val>, Val)>,
    contract: &Address,
    topic: &str,
) -> Val {
    let expected: Symbol = Symbol::new(env, topic);
    events
        .iter()
        .find(|(id, topics, _)| {
            id == contract
                && topics
                    .get(0)
                    .map(|t| Symbol::from_val(env, &t) == expected)
                    .unwrap_or(false)
        })
        .map(|(_, _, data)| data)
        .expect("expected event was not emitted")
}

/// Returns the `topic[0]` symbol of the event published by `contract` with
/// that topic name.
fn event_topic(
    env: &Env,
    events: &Vec<(Address, Vec<Val>, Val)>,
    contract: &Address,
    topic: &str,
) -> Symbol {
    let expected: Symbol = Symbol::new(env, topic);
    events
        .iter()
        .find(|(id, topics, _)| {
            id == contract
                && topics
                    .get(0)
                    .map(|t| Symbol::from_val(env, &t) == expected)
                    .unwrap_or(false)
        })
        .map(|(_, topics, _)| Symbol::from_val(env, &topics.get(0).unwrap()))
        .expect("expected event was not emitted")
}

#[test]
fn admin_transferred_event_is_canonical_across_contracts() {
    let (env, settlement, gov, s_admins, g_admins) = deploy_both_contracts();

    let s_new = Address::generate(&env);
    let g_new = Address::generate(&env);
    settlement.transfer_admin(&s_admins, &soroban_sdk::vec![&env, s_new.clone()], &1);
    gov.transfer_admin(&g_admins, &soroban_sdk::vec![&env, g_new.clone()], &1);

    let events = env.events().all();

    // Both contracts must publish the identical canonical topic symbol.
    let s_topic = event_topic(&env, &events, &settlement.address, ADMIN_TRANSFERRED_EVENT);
    let g_topic = event_topic(&env, &events, &gov.address, ADMIN_TRANSFERRED_EVENT);
    assert_eq!(s_topic, g_topic);
    assert_eq!(s_topic, Symbol::new(&env, ADMIN_TRANSFERRED_EVENT));

    // And the payload must decode as the canonical `AdminTransferred` shape on
    // both contracts — not a bare address.
    let s_payload: AdminTransferred = AdminTransferred::try_from_val(
        &env,
        &event_data(&env, &events, &settlement.address, ADMIN_TRANSFERRED_EVENT),
    )
    .unwrap();
    let g_payload: AdminTransferred = AdminTransferred::try_from_val(
        &env,
        &event_data(&env, &events, &gov.address, ADMIN_TRANSFERRED_EVENT),
    )
    .unwrap();
    assert_eq!(s_payload.old_admin, s_admins.get(0).unwrap());
    assert_eq!(s_payload.new_admin, s_new);
    assert_eq!(g_payload.old_admin, g_admins.get(0).unwrap());
    assert_eq!(g_payload.new_admin, g_new);
}

#[test]
fn pause_events_are_canonical_across_contracts() {
    let (env, settlement, gov, s_admins, g_admins) = deploy_both_contracts();

    settlement.pause(&s_admins);
    gov.pause(&g_admins);
    let events = env.events().all();

    let s_topic = event_topic(&env, &events, &settlement.address, PAUSED_EVENT);
    let g_topic = event_topic(&env, &events, &gov.address, PAUSED_EVENT);
    assert_eq!(s_topic, g_topic);
    assert_eq!(s_topic, Symbol::new(&env, PAUSED_EVENT));

    let (s_admin, s_flag): (Address, bool) = FromVal::from_val(
        &env,
        &event_data(&env, &events, &settlement.address, PAUSED_EVENT),
    );
    let (g_admin, g_flag): (Address, bool) =
        FromVal::from_val(&env, &event_data(&env, &events, &gov.address, PAUSED_EVENT));
    assert_eq!(s_admin, s_admins.get(0).unwrap());
    assert!(s_flag);
    assert_eq!(g_admin, g_admins.get(0).unwrap());
    assert!(g_flag);

    settlement.unpause(&s_admins);
    gov.unpause(&g_admins);
    let events = env.events().all();

    let s_topic = event_topic(&env, &events, &settlement.address, UNPAUSED_EVENT);
    let g_topic = event_topic(&env, &events, &gov.address, UNPAUSED_EVENT);
    assert_eq!(s_topic, g_topic);
    assert_eq!(s_topic, Symbol::new(&env, UNPAUSED_EVENT));

    let (s_admin, s_flag): (Address, bool) = FromVal::from_val(
        &env,
        &event_data(&env, &events, &settlement.address, UNPAUSED_EVENT),
    );
    let (g_admin, g_flag): (Address, bool) = FromVal::from_val(
        &env,
        &event_data(&env, &events, &gov.address, UNPAUSED_EVENT),
    );
    assert_eq!(s_admin, s_admins.get(0).unwrap());
    assert!(!s_flag);
    assert_eq!(g_admin, g_admins.get(0).unwrap());
    assert!(!g_flag);
}

#[test]
fn recovery_executed_event_is_canonical_across_contracts() {
    let (env, settlement, gov, s_admins, g_admins) = deploy_both_contracts();

    let s_new_admin = Address::generate(&env);
    let g_new_admin = Address::generate(&env);
    settlement.initiate_recovery(&s_new_admin);
    gov.initiate_recovery(&g_new_admin);
    env.ledger()
        .with_mut(|ledger| ledger.timestamp += RECOVERY_DELAY_SECONDS);
    settlement.execute_recovery();
    gov.execute_recovery();

    let events = env.events().all();

    let s_topic = event_topic(&env, &events, &settlement.address, RECOVERY_EXECUTED_EVENT);
    let g_topic = event_topic(&env, &events, &gov.address, RECOVERY_EXECUTED_EVENT);
    assert_eq!(s_topic, g_topic);
    assert_eq!(s_topic, Symbol::new(&env, RECOVERY_EXECUTED_EVENT));

    // `recovery_executed` must carry the canonical `AdminTransferred` payload
    // on both contracts — not a bare address.
    let s_payload: AdminTransferred = AdminTransferred::try_from_val(
        &env,
        &event_data(&env, &events, &settlement.address, RECOVERY_EXECUTED_EVENT),
    )
    .unwrap();
    let g_payload: AdminTransferred = AdminTransferred::try_from_val(
        &env,
        &event_data(&env, &events, &gov.address, RECOVERY_EXECUTED_EVENT),
    )
    .unwrap();
    assert_eq!(s_payload.old_admin, s_admins.get(0).unwrap());
    assert_eq!(s_payload.new_admin, s_new_admin);
    assert_eq!(g_payload.old_admin, g_admins.get(0).unwrap());
    assert_eq!(g_payload.new_admin, g_new_admin);
}
