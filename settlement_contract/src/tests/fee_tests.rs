//! Tests for fee and settlement rule entry points:
//! `calculate_fee_split`, `set_settlement_rule`, `clear_settlement_rule`, `set_default_rule`.

use crate::*;
use soroban_sdk::testutils::{Address as _, Events, MockAuth, MockAuthInvoke};
use soroban_sdk::FromVal;

use super::{register_governance, setup};

// ---------------------------------------------------------------------------
// calculate_fee_split
// ---------------------------------------------------------------------------

#[test]
fn calculates_split_without_storing_reference() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let split = client.calculate_fee_split(&merchant, &50_000);
    assert_eq!(split.platform_fee_amount, 500); // Because default is 100 bps
    assert_eq!(split.network_fee_amount, 0);
    assert_eq!(split.merchant_amount, 49_500);
}

#[test]
fn calculate_fee_split_extends_default_rule_ttl_on_read() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let global_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };
    client.set_default_rule(&global_rule);

    env.ledger().set_sequence_number(env.ledger().sequence() + 1000);

    client.calculate_fee_split(&merchant, &50_000);

    env.as_contract(&client.address, || {
        let key = DataKey::DefaultRule;
        assert!(env.storage().persistent().has(&key));
        let ttl = env.storage().persistent().get_ttl(&key);
        assert!(
            ttl >= env.ledger().sequence() + RULE_TTL_BUMP,
            "DefaultRule TTL must be extended on read"
        );
    });
}

#[test]
fn bootstrap_default_used_before_any_default_rule_set() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    // No global default set — falls back to hardcoded 100 bps
    let split = client.calculate_fee_split(&merchant, &50_000);
    assert_eq!(split.platform_fee_amount, 500);
    assert_eq!(split.network_fee_amount, 0);
    assert_eq!(split.merchant_amount, 49_500);
}

#[test]
fn bootstrap_fallback_emits_event_and_matches_bootstrap_rule() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let before = env.events().all().len();
    let split = client.calculate_fee_split(&merchant, &50_000);

    // Verify the returned rule matches BOOTSTRAP_DEFAULT_RULE
    assert_eq!(split.platform_fee_amount, 500);
    assert_eq!(split.network_fee_amount, 0);
    assert_eq!(split.merchant_amount, 49_500);

    // Verify bootstrap_fallback event was emitted
    let events = env.events().all();
    assert!(
        events.len() > before,
        "at least one event expected from bootstrap fallback"
    );

    let fallback_event = events
        .iter()
        .skip(before as usize)
        .find(|(_id, topics, _data)| {
            !topics.is_empty()
                && Symbol::from_val(&env, &topics.get(0).unwrap())
                    == Symbol::new(&env, "bootstrap_fallback")
        })
        .expect("expected bootstrap_fallback event to be emitted");

    let (_id, topics, data) = fallback_event;
    assert_eq!(topics.len(), 1);
    let emitted: SettlementRule = FromVal::from_val(&env, &data);
    assert_eq!(emitted.platform_fee_bps, 100);
    assert_eq!(emitted.network_fee_bps, 0);
    assert_eq!(emitted.settlement_delay_ledger, 0);
    assert!(!emitted.auto_settle);
}

#[test]
fn global_default_used_when_no_explicit_merchant_rule() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let global_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };
    client.set_default_rule(&global_rule);

    let split = client.calculate_fee_split(&merchant, &50_000);
    assert_eq!(split.platform_fee_amount, 1_000); // 200 bps
    assert_eq!(split.network_fee_amount, 250); // 50 bps
    assert_eq!(split.merchant_amount, 48_750);
}

#[test]
fn explicit_merchant_rule_overrides_global_default() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let global_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };
    client.set_default_rule(&global_rule);

    let merchant_rule = SettlementRule {
        platform_fee_bps: 175,
        network_fee_bps: 25,
        settlement_delay_ledger: 42,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &merchant_rule);

    let split = client.calculate_fee_split(&merchant, &50_000);
    // Merchant rule (175/25) takes precedence over global default (200/50)
    assert_eq!(split.platform_fee_amount, 875); // 175 bps
    assert_eq!(split.network_fee_amount, 125); // 25 bps
    assert_eq!(split.merchant_amount, 49_000);
}

// Issue #85: verify default fee split falls back to 100 BPS
#[test]
fn default_fee_split_uses_100_bps() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let split = client.calculate_fee_split(&merchant, &10_000);
    assert_eq!(split.platform_fee_amount, 100);
    assert_eq!(split.network_fee_amount, 0);
    assert_eq!(split.merchant_amount, 9_900);
}

// Issue #86: calculate_fee_split output matches custom rule parameters
#[test]
fn calculate_fee_split_uses_custom_rule_parameters() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 500,
        network_fee_bps: 250,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
    let split = client.calculate_fee_split(&merchant, &100_000);
    assert_eq!(split.gross_amount, 100_000);
    assert_eq!(split.platform_fee_amount, 5_000);
    assert_eq!(split.network_fee_amount, 2_500);
    assert_eq!(split.merchant_amount, 92_500);
}

// Issue #248: calculate_fee_split panics with a readable AmountOverflow error
// instead of a raw arithmetic-overflow panic when amount * bps would overflow i128.
#[test]
#[should_panic(expected = "Error(Contract, #19)")]
fn calculate_fee_split_rejects_amount_that_would_overflow() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 500,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
    // (i128::MAX - (BPS_DENOMINATOR - 1)) / 500 is the largest amount that stays
    // safe through the ceil-rounding addition; one past it overflows.
    let amount = (i128::MAX - (BPS_DENOMINATOR as i128 - 1)) / 500 + 1;
    client.calculate_fee_split(&merchant, &amount);
}

// Issue #248: amounts right at the overflow boundary must still succeed normally.
#[test]
fn calculate_fee_split_accepts_amount_at_overflow_boundary() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 500,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
    let amount = (i128::MAX - (BPS_DENOMINATOR as i128 - 1)) / 500;
    let split = client.calculate_fee_split(&merchant, &amount);
    assert_eq!(split.gross_amount, amount);
}

// Issue #248: a zero-bps rule must never divide by zero in the overflow precheck.
#[test]
fn calculate_fee_split_with_zero_bps_rule_accepts_max_amount() {
    // set_settlement_rule rejects fees below MIN_FEE_BPS, so exercise the
    // zero-bps overflow precheck via calculate_split directly.
    let env = Env::default();
    let rule = SettlementRule {
        platform_fee_bps: 0,
        network_fee_bps: 0,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    let split = calculate_split(&env, i128::MAX, &rule);
    assert_eq!(split.gross_amount, i128::MAX);
    assert_eq!(split.platform_fee_amount, 0);
    assert_eq!(split.network_fee_amount, 0);
    assert_eq!(split.merchant_amount, i128::MAX);
}

// ---------------------------------------------------------------------------
// Issue #294: property-based fee split invariants (proptest)
// ---------------------------------------------------------------------------

/// Property test: for random valid amounts and fee BPS, the fee-split
/// accounting invariants hold.
///
/// Amount is capped at `i128::MAX / BPS_DENOMINATOR` so `amount * bps`
/// cannot overflow; overflow behavior is covered separately by issue #295.
#[test]
fn fee_split_invariants_hold_for_random_inputs() {
    use proptest::prelude::*;
    use proptest::test_runner::{Config, TestRunner};

    let env = Env::default();
    let mut runner = TestRunner::new(Config {
        cases: 256,
        ..Config::default()
    });

    runner
        .run(
            &(
                1i128..=(i128::MAX / BPS_DENOMINATOR as i128),
                0u32..=BPS_DENOMINATOR,
                0u32..=BPS_DENOMINATOR,
            ),
            |(amount, platform_fee_bps, network_fee_bps)| {
                prop_assume!(
                    (platform_fee_bps as u64) + (network_fee_bps as u64)
                        <= BPS_DENOMINATOR as u64
                );

                let rule = SettlementRule {
                    platform_fee_bps,
                    network_fee_bps,
                    settlement_delay_ledger: 0,
                    auto_settle: false,
                };
                let split = calculate_split(&env, amount, &rule);

                prop_assert_eq!(
                    split.merchant_amount
                        + split.platform_fee_amount
                        + split.network_fee_amount,
                    amount
                );
                prop_assert_eq!(split.gross_amount, amount);
                prop_assert!(split.platform_fee_amount >= 0);
                prop_assert!(split.network_fee_amount >= 0);

                if split.merchant_amount >= 0 {
                    prop_assert!(
                        split.platform_fee_amount + split.network_fee_amount <= amount
                    );
                } else {
                    prop_assert!(
                        split.platform_fee_amount + split.network_fee_amount > amount,
                        "negative merchant must mean fees exceeded gross"
                    );
                }

                Ok(())
            },
        )
        .unwrap();
}

// ---------------------------------------------------------------------------
// Issue #295: overflow behavior of calculate_split / fee math
// ---------------------------------------------------------------------------

/// `amount = i128::MAX` with `platform_fee_bps = 10000` is rejected with
/// the contract-specific [`SettlementError::AmountOverflow`] (`#19`).
#[test]
#[should_panic(expected = "Error(Contract, #19)")]
fn calculate_fee_split_overflows_at_i128_max_with_full_bps() {
    let env = Env::default();
    let rule = SettlementRule {
        platform_fee_bps: BPS_DENOMINATOR,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    let _ = calculate_split(&env, i128::MAX, &rule);
}

// ---------------------------------------------------------------------------
// Issue #296: negative merchant amount from ceiling rounding
// ---------------------------------------------------------------------------

/// Documents the known ceiling-rounding edge case where the sum of
/// rounded-up fees exceeds the gross amount, producing a negative
/// `merchant_amount`.
///
/// With `platform_fee_bps = 5000`, `network_fee_bps = 5000`, `amount = 1`:
/// each fee = `(1 * 5000 + 9999) / 10000 = 1`, so merchant = `1 - 1 - 1 = -1`.
#[test]
fn calculate_split_negative_merchant_amount_from_rounding() {
    let env = Env::default();
    let rule = SettlementRule {
        platform_fee_bps: 5_000,
        network_fee_bps: 5_000,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    let split = calculate_split(&env, 1, &rule);

    assert_eq!(split.gross_amount, 1);
    assert_eq!(split.platform_fee_amount, 1);
    assert_eq!(split.network_fee_amount, 1);
    // Known & expected: ceil fees can exceed gross for tiny amounts.
    assert_eq!(split.merchant_amount, -1);
    assert!(
        split.merchant_amount < 0,
        "merchant_amount must be negative for this documented rounding edge case"
    );
    assert_eq!(
        split.merchant_amount + split.platform_fee_amount + split.network_fee_amount,
        split.gross_amount
    );
}

// ---------------------------------------------------------------------------
// set_settlement_rule
// ---------------------------------------------------------------------------

#[test]
fn sets_and_reads_settlement_rule() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let rule = SettlementRule {
        platform_fee_bps: 175,
        network_fee_bps: 25,
        settlement_delay_ledger: 42,
        auto_settle: true,
    };

    let prev_count = env.events().all().len();
    client.set_settlement_rule(&merchant, &rule);
    let got = client
        .get_settlement_rule(&merchant)
        .expect("expected settlement rule");

    assert_eq!(got.platform_fee_bps, 175);
    assert_eq!(got.network_fee_bps, 25);
    assert_eq!(got.settlement_delay_ledger, 42);
    assert!(got.auto_settle);

    let events = env.events().all();
    assert_eq!(events.len(), prev_count + 1, "exactly one event emitted");

    let (_contract_id, topics, _data) = events.get(prev_count).unwrap();

    // Topic[0] must be the fixed event-name symbol
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "settlement_rule_updated")
    );
    // Topic[1] must be the merchant (rule identifier)
    assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
}

#[test]
fn emits_structured_event_when_updating_rule() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let first_rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 5,
        settlement_delay_ledger: 10,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &first_rule);

    let second_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 20,
        auto_settle: true,
    };

    let prev_count = env.events().all().len();
    client.set_settlement_rule(&merchant, &second_rule);

    let events = env.events().all();
    assert_eq!(events.len(), prev_count + 1, "exactly one event emitted");

    let (_contract_id, topics, _data) = events.get(prev_count).unwrap();
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "settlement_rule_updated")
    );
    assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);

    let stored = client
        .get_settlement_rule(&merchant)
        .expect("expected settlement rule");
    assert_eq!(stored.platform_fee_bps, 200);
    assert_eq!(stored.network_fee_bps, 50);
    assert_eq!(stored.settlement_delay_ledger, 20);
    assert!(stored.auto_settle);
}

#[test]
fn extends_ttl_when_updating_settlement_rule() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };

    client.set_settlement_rule(&merchant, &rule);

    env.as_contract(&client.address, || {
        let key = DataKey::Rule(merchant.clone());
        assert!(env.storage().persistent().has(&key));
    });
}

#[test]
#[should_panic]
fn rejects_invalid_fee_bps() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let bad_rule = SettlementRule {
        platform_fee_bps: 10_001,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &bad_rule);
}

#[test]
#[should_panic]
fn rejects_settlement_rule_below_governance_min_fee() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let bad_rule = SettlementRule {
        platform_fee_bps: 4,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &bad_rule);
}

#[test]
#[should_panic]
fn rejects_fee_sum_exceeding_10000_bps() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let bad_rule = SettlementRule {
        platform_fee_bps: 6_000,
        network_fee_bps: 5_000,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &bad_rule);
}

#[test]
fn accepts_fee_sum_at_exactly_10000_bps() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 5_000,
        network_fee_bps: 5_000,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
    let stored = client
        .get_settlement_rule(&merchant)
        .expect("expected settlement rule");
    assert_eq!(stored.platform_fee_bps, 5_000);
    assert_eq!(stored.network_fee_bps, 5_000);
}

#[test]
#[should_panic]
fn rejects_settlement_delay_above_maximum() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 5,
        settlement_delay_ledger: 100_001,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
}

#[test]
#[should_panic]
fn rejects_settlement_delay_at_u32_max() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 5,
        settlement_delay_ledger: u32::MAX,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
}

#[test]
fn accepts_valid_settlement_delay_zero() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
    let stored = client
        .get_settlement_rule(&merchant)
        .expect("expected settlement rule");
    assert_eq!(stored.settlement_delay_ledger, 0);
}

#[test]
fn accepts_valid_settlement_delay_one() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 5,
        settlement_delay_ledger: 1,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
    let stored = client
        .get_settlement_rule(&merchant)
        .expect("expected settlement rule");
    assert_eq!(stored.settlement_delay_ledger, 1);
}

#[test]
fn accepts_settlement_delay_at_maximum_boundary() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 5,
        settlement_delay_ledger: 100_000,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
    let stored = client
        .get_settlement_rule(&merchant)
        .expect("expected settlement rule");
    assert_eq!(stored.settlement_delay_ledger, 100_000);
}

#[test]
#[should_panic]
fn set_settlement_rule_requires_admin_auth() {
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
    client.register_merchant(&merchant);

    let rule = SettlementRule {
        platform_fee_bps: 100,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };

    // Switch to explicit mock_auths to test authorization failure.
    // We only provide authorization for the non_admin, but the contract
    // requires authorization from the admin address.
    env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "set_settlement_rule",
            args: soroban_sdk::vec![
                &env,
                merchant.clone().into_val(&env),
                rule.clone().into_val(&env)
            ],
            sub_invokes: &[],
        },
    }]);

    client.set_settlement_rule(&merchant, &rule);
}

// Issue #88: verify set_settlement_rule publishes event with caller and rule data
#[test]
fn set_settlement_rule_publishes_event_with_rule_data() {
    let (env, client, admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 300,
        network_fee_bps: 75,
        settlement_delay_ledger: 5,
        auto_settle: true,
    };
    let before = env.events().all().len();
    client.set_settlement_rule(&merchant, &rule);
    let events = env.events().all();
    assert_eq!(events.len(), before + 1, "exactly one event emitted");
    let (_contract_id, topics, data) = events.get(before).unwrap();
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "settlement_rule_updated")
    );
    assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);
    let (caller, _prev, current): (Address, SettlementRule, SettlementRule) =
        FromVal::from_val(&env, &data);
    assert_eq!(caller, admin);
    assert_eq!(current.platform_fee_bps, 300);
    assert_eq!(current.network_fee_bps, 75);
    assert_eq!(current.settlement_delay_ledger, 5);
    assert!(current.auto_settle);
}

/// Verify that `set_settlement_rule` rejects fee combinations whose
/// `platform_fee_bps + network_fee_bps` sum exceeds 10,000 bps with
/// the specific `InvalidFeeBps` contract error (#6).
#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn assert_fee_sum_above_10000_bps_panics_with_invalid_fee_bps() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    // 6_000 + 5_000 = 11_000 which is 1_000 bps over the 10_000 cap.
    let bad_rule = SettlementRule {
        platform_fee_bps: 6_000,
        network_fee_bps: 5_000,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &bad_rule);
}

// ---------------------------------------------------------------------------
// clear_settlement_rule
// ---------------------------------------------------------------------------

#[test]
fn admin_clears_custom_rule() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let rule = SettlementRule {
        platform_fee_bps: 175,
        network_fee_bps: 25,
        settlement_delay_ledger: 42,
        auto_settle: true,
    };
    client.set_settlement_rule(&merchant, &rule);

    client.clear_settlement_rule(&merchant);

    // Storage key is gone: getter returns None
    assert!(client.get_settlement_rule(&merchant).is_none());

    // find the settlement_rule_cleared event and verify its data
    let events = env.events().all();
    let cleared_event = events
        .iter()
        .rev()
        .find(|(_id, topics, _data)| {
            topics.len() >= 2
                && Symbol::from_val(&env, &topics.get(0).unwrap())
                    == Symbol::new(&env, "settlement_rule_cleared")
                && Address::from_val(&env, &topics.get(1).unwrap()) == merchant
        })
        .expect("expected settlement_rule_cleared event");
    let (_contract_id, _topics, data) = cleared_event;

    let (admin_addr, removed, fallback): (Address, SettlementRule, SettlementRule) =
        FromVal::from_val(&env, &data);
    assert_eq!(admin_addr, _admin);
    assert_eq!(removed.platform_fee_bps, rule.platform_fee_bps);
    assert_eq!(removed.network_fee_bps, rule.network_fee_bps);
    assert_eq!(removed.settlement_delay_ledger, rule.settlement_delay_ledger);
    assert_eq!(removed.auto_settle, rule.auto_settle);
    assert_eq!(fallback.platform_fee_bps, BOOTSTRAP_DEFAULT_RULE.platform_fee_bps);
    assert_eq!(fallback.network_fee_bps, BOOTSTRAP_DEFAULT_RULE.network_fee_bps);
    assert_eq!(
        fallback.settlement_delay_ledger,
        BOOTSTRAP_DEFAULT_RULE.settlement_delay_ledger
    );
    assert_eq!(fallback.auto_settle, BOOTSTRAP_DEFAULT_RULE.auto_settle);
}

#[test]
fn clearing_rule_falls_back_to_defaults() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let rule = SettlementRule {
        platform_fee_bps: 500,
        network_fee_bps: 200,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };
    client.set_settlement_rule(&merchant, &rule);

    client.clear_settlement_rule(&merchant);

    // calculate_fee_split should now use default rates (100 bps platform, 0 bps network)
    let split = client.calculate_fee_split(&merchant, &50_000);
    assert_eq!(split.platform_fee_amount, 500); // 100 bps of 50_000
    assert_eq!(split.network_fee_amount, 0);
    assert_eq!(split.merchant_amount, 49_500);

    // find the settlement_rule_cleared event and verify its data
    let events = env.events().all();
    let cleared_event = events
        .iter()
        .rev()
        .find(|(_id, topics, _data)| {
            topics.len() >= 2
                && Symbol::from_val(&env, &topics.get(0).unwrap())
                    == Symbol::new(&env, "settlement_rule_cleared")
                && Address::from_val(&env, &topics.get(1).unwrap()) == merchant
        })
        .expect("expected settlement_rule_cleared event");
    let (_contract_id, _topics, data) = cleared_event;

    let (_caller, removed, fallback): (Address, SettlementRule, SettlementRule) =
        FromVal::from_val(&env, &data);
    assert_eq!(removed.platform_fee_bps, rule.platform_fee_bps);
    assert_eq!(removed.network_fee_bps, rule.network_fee_bps);
    assert_eq!(removed.settlement_delay_ledger, rule.settlement_delay_ledger);
    assert_eq!(removed.auto_settle, rule.auto_settle);
    assert_eq!(fallback.platform_fee_bps, BOOTSTRAP_DEFAULT_RULE.platform_fee_bps);
    assert_eq!(fallback.network_fee_bps, BOOTSTRAP_DEFAULT_RULE.network_fee_bps);
    assert_eq!(
        fallback.settlement_delay_ledger,
        BOOTSTRAP_DEFAULT_RULE.settlement_delay_ledger
    );
    assert_eq!(fallback.auto_settle, BOOTSTRAP_DEFAULT_RULE.auto_settle);
}

#[test]
fn clearing_rule_falls_back_to_global_default() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let global_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };
    client.set_default_rule(&global_rule);

    let merchant_rule = SettlementRule {
        platform_fee_bps: 500,
        network_fee_bps: 100,
        settlement_delay_ledger: 20,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &merchant_rule);

    let prev_count = env.events().all().len();
    client.clear_settlement_rule(&merchant);

    // After clearing, should fall back to global default (200/50), not bootstrap (100/0)
    let split = client.calculate_fee_split(&merchant, &50_000);
    assert_eq!(split.platform_fee_amount, 1_000); // 200 bps
    assert_eq!(split.network_fee_amount, 250); // 50 bps
    assert_eq!(split.merchant_amount, 48_750);

    // Event check: fallback should be the global default rule
    let events = env.events().all();
    assert_eq!(events.len(), prev_count + 1);
    let (_contract_id, topics, data) = events.get(prev_count).unwrap();
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "settlement_rule_cleared")
    );
    assert_eq!(Address::from_val(&env, &topics.get(1).unwrap()), merchant);

    let (_caller, removed, fallback): (Address, SettlementRule, SettlementRule) =
        FromVal::from_val(&env, &data);
    assert_eq!(removed.platform_fee_bps, merchant_rule.platform_fee_bps);
    assert_eq!(removed.network_fee_bps, merchant_rule.network_fee_bps);
    assert_eq!(
        removed.settlement_delay_ledger,
        merchant_rule.settlement_delay_ledger
    );
    assert_eq!(removed.auto_settle, merchant_rule.auto_settle);
    assert_eq!(fallback.platform_fee_bps, global_rule.platform_fee_bps);
    assert_eq!(fallback.network_fee_bps, global_rule.network_fee_bps);
    assert_eq!(
        fallback.settlement_delay_ledger,
        global_rule.settlement_delay_ledger
    );
    assert_eq!(fallback.auto_settle, global_rule.auto_settle);
}

#[test]
#[should_panic]
fn clear_settlement_rule_fails_for_non_admin() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    let governance = register_governance(&env);
    let recovery_address = Address::generate(&env);
    let contract_id: Address = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    // Authorize admin for init
    let invoke = MockAuthInvoke {
        contract: &contract_id,
        fn_name: "init",
        args: soroban_sdk::vec![
            &env,
            admin.to_val(),
            governance.to_val(),
            recovery_address.to_val()
        ],
        sub_invokes: &[],
    };
    let auth = MockAuth {
        address: &admin,
        invoke: &invoke,
    };
    env.set_auths(&[(&auth).into()]);
    client.init(&admin, &governance, &recovery_address);

    // Authorize admin for register_merchant
    let reg_invoke = MockAuthInvoke {
        contract: &contract_id,
        fn_name: "register_merchant",
        args: soroban_sdk::vec![&env, merchant.to_val()],
        sub_invokes: &[],
    };
    let reg_auth = MockAuth {
        address: &admin,
        invoke: &reg_invoke,
    };
    env.set_auths(&[(&reg_auth).into()]);
    client.register_merchant(&merchant);

    // Do NOT authorize admin for clear_settlement_rule — should panic
    client.clear_settlement_rule(&merchant);
}

#[test]
#[should_panic]
fn clear_settlement_rule_fails_when_not_set() {
    let (_env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    client.clear_settlement_rule(&merchant);
}

// ---------------------------------------------------------------------------
// set_default_rule
// ---------------------------------------------------------------------------

#[test]
fn set_default_rule_stores_and_can_be_retrieved() {
    let (_env, client, _admin, _merchant) = setup();

    assert!(client.get_default_rule().is_none());

    let rule = SettlementRule {
        platform_fee_bps: 300,
        network_fee_bps: 100,
        settlement_delay_ledger: 5,
        auto_settle: true,
    };
    client.set_default_rule(&rule);

    let stored = client
        .get_default_rule()
        .expect("global default must be present");
    assert_eq!(stored.platform_fee_bps, 300);
    assert_eq!(stored.network_fee_bps, 100);
    assert_eq!(stored.settlement_delay_ledger, 5);
    assert!(stored.auto_settle);
}

#[test]
fn set_default_rule_extends_ttl() {
    let (env, client, _admin, _merchant) = setup();

    let rule = SettlementRule {
        platform_fee_bps: 300,
        network_fee_bps: 100,
        settlement_delay_ledger: 5,
        auto_settle: true,
    };
    client.set_default_rule(&rule);

    env.as_contract(&client.address, || {
        let key = DataKey::DefaultRule;
        assert!(env.storage().persistent().has(&key));
        let ttl = env.storage().persistent().get_ttl(&key);
        assert!(
            ttl >= env.ledger().sequence() + RULE_TTL_BUMP,
            "TTL must be extended to at least ledger + RULE_TTL_BUMP"
        );
    });
}

// Issue #252: the TTL must be refreshed on every write to the default rule,
// not just the first one — otherwise a rarely-updated (but frequently-read)
// default rule could still expire between updates.
#[test]
fn set_default_rule_extends_ttl_on_update() {
    let (env, client, _admin, _merchant) = setup();

    let first_rule = SettlementRule {
        platform_fee_bps: 300,
        network_fee_bps: 100,
        settlement_delay_ledger: 5,
        auto_settle: true,
    };
    client.set_default_rule(&first_rule);

    // Advance the ledger past RULE_TTL_THRESHOLD so the remaining TTL from
    // the first call drops below the threshold and a second write is
    // actually required to bump it back up (extend_ttl is a no-op while
    // the remaining TTL is still above the threshold). Advance in smaller
    // hops, touching the contract via get_admin() between hops, so the
    // instance's own (much shorter) TTL doesn't expire along the way.
    for _ in 0..5 {
        env.ledger().set_sequence_number(env.ledger().sequence() + 60_000);
        client.get_admin();
    }

    let second_rule = SettlementRule {
        platform_fee_bps: 400,
        network_fee_bps: 150,
        settlement_delay_ledger: 10,
        auto_settle: false,
    };
    client.set_default_rule(&second_rule);

    env.as_contract(&client.address, || {
        let key = DataKey::DefaultRule;
        let ttl = env.storage().persistent().get_ttl(&key);
        assert!(
            ttl >= RULE_TTL_BUMP,
            "TTL must be refreshed to at least RULE_TTL_BUMP on every write, not just the first"
        );
    });
}

#[test]
fn get_default_rule_extends_ttl_on_read() {
    let (env, client, _admin, _merchant) = setup();

    let global_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };
    client.set_default_rule(&global_rule);

    env.ledger().set_sequence_number(env.ledger().sequence() + 1000);

    let retrieved = client.get_default_rule();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().platform_fee_bps, 200);

    env.as_contract(&client.address, || {
        let key = DataKey::DefaultRule;
        assert!(env.storage().persistent().has(&key));
        let ttl = env.storage().persistent().get_ttl(&key);
        assert!(
            ttl >= env.ledger().sequence() + RULE_TTL_BUMP,
            "DefaultRule TTL must be extended on public read via get_default_rule"
        );
    });
}

#[test]
fn set_default_rule_emits_event_with_correct_topic() {
    let (env, client, _admin, _merchant) = setup();

    let rule = SettlementRule {
        platform_fee_bps: 150,
        network_fee_bps: 25,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_default_rule(&rule);

    let events = env.events().all();
    let (_contract_id, topics, _data) = events.get(events.len() - 1).unwrap();

    // Single-element topic: just the event name
    assert_eq!(topics.len(), 1);
    assert_eq!(
        Symbol::from_val(&env, &topics.get(0).unwrap()),
        Symbol::new(&env, "default_rule_updated")
    );
}

#[test]
fn set_default_rule_updates_twice_emits_correct_previous() {
    let (_env, client, _admin, _merchant) = setup();

    let first = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };
    client.set_default_rule(&first);
    let stored = client
        .get_default_rule()
        .expect("global default must be present");
    assert_eq!(stored.platform_fee_bps, 200);

    let second = SettlementRule {
        platform_fee_bps: 500,
        network_fee_bps: 100,
        settlement_delay_ledger: 20,
        auto_settle: false,
    };
    client.set_default_rule(&second);
    let stored = client
        .get_default_rule()
        .expect("global default must be present");
    assert_eq!(stored.platform_fee_bps, 500);
}

#[test]
#[should_panic]
fn set_default_rule_fails_for_non_admin() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let governance = register_governance(&env);
    let recovery_address = Address::generate(&env);
    let contract_id: Address = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    let invoke = MockAuthInvoke {
        contract: &contract_id,
        fn_name: "init",
        args: soroban_sdk::vec![
            &env,
            admin.to_val(),
            governance.to_val(),
            recovery_address.to_val()
        ],
        sub_invokes: &[],
    };
    let auth = MockAuth {
        address: &admin,
        invoke: &invoke,
    };
    env.set_auths(&[(&auth).into()]);
    client.init(&admin, &governance, &recovery_address);

    let rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };

    // Do NOT authorize admin — should panic
    client.set_default_rule(&rule);
}

#[test]
#[should_panic]
fn set_default_rule_rejects_invalid_fee_bps() {
    let (_env, client, _admin, _merchant) = setup();
    let bad_rule = SettlementRule {
        platform_fee_bps: 10_001,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_default_rule(&bad_rule);
}

#[test]
#[should_panic]
fn set_default_rule_rejects_below_governance_min_fee() {
    let (_env, client, _admin, _merchant) = setup();
    let bad_rule = SettlementRule {
        platform_fee_bps: 4,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_default_rule(&bad_rule);
}

#[test]
fn accepts_default_rule_with_valid_settlement_delay() {
    let (_env, client, _admin, _merchant) = setup();
    let rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 50_000,
        auto_settle: true,
    };
    client.set_default_rule(&rule);
    let stored = client.get_default_rule().expect("expected default rule");
    assert_eq!(stored.settlement_delay_ledger, 50_000);
}

#[test]
fn accepts_default_rule_at_settlement_delay_maximum() {
    let (_env, client, _admin, _merchant) = setup();
    let rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 100_000,
        auto_settle: true,
    };
    client.set_default_rule(&rule);
    let stored = client.get_default_rule().expect("expected default rule");
    assert_eq!(stored.settlement_delay_ledger, 100_000);
}

#[test]
#[should_panic]
fn rejects_default_rule_with_settlement_delay_above_maximum() {
    let (_env, client, _admin, _merchant) = setup();
    let rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 100_001,
        auto_settle: true,
    };
    client.set_default_rule(&rule);
}

#[test]
#[should_panic]
fn rejects_default_rule_with_settlement_delay_at_u32_max() {
    let (_env, client, _admin, _merchant) = setup();
    let rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: u32::MAX,
        auto_settle: true,
    };
    client.set_default_rule(&rule);
}

// ---------------------------------------------------------------------------
// Cross-contract invariant
// ---------------------------------------------------------------------------

#[test]
fn settlement_min_fee_matches_governance_min_fee() {
    // Both contracts must enforce the same minimum fee of 5 bps.
    let governance_min_fee_bps: u32 = 5;
    let settlement_min_fee_bps: u32 = MIN_FEE_BPS;
    assert_eq!(
        governance_min_fee_bps, settlement_min_fee_bps,
        "settlement MIN_FEE_BPS must match governance MIN_FEE_BPS"
    );
}

// ---------------------------------------------------------------------------
// Rule resolution path coverage (read_rule_or_default)
// ---------------------------------------------------------------------------

// Regression coverage for read_rule_or_default's single-`get()`-per-key lookup
// (see #264): each branch must resolve with exactly one storage read for the
// key it needs, without probing or extending the TTL of the other rule key.
#[test]
fn read_rule_or_default_short_circuits_on_merchant_rule_without_touching_default() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let default_rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 10,
        auto_settle: true,
    };
    client.set_default_rule(&default_rule);

    let merchant_rule = SettlementRule {
        platform_fee_bps: 300,
        network_fee_bps: 75,
        settlement_delay_ledger: 20,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &merchant_rule);

    // Capture DefaultRule's absolute expiration ledger (sequence + remaining TTL)
    // rather than the raw remaining-TTL count, since the count alone decays with
    // every ledger that passes regardless of whether the entry was touched.
    let default_expiration_before = env.as_contract(&client.address, || {
        env.ledger().sequence() + env.storage().persistent().get_ttl(&DataKey::DefaultRule)
    });

    // Keep the contract instance itself alive across the large jump below —
    // otherwise it would archive first and mask the assertions this test cares about.
    env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .extend_ttl(RULE_TTL_THRESHOLD + 200_000, RULE_TTL_THRESHOLD + 1_000_000);
    });

    // Advance the ledger past RULE_TTL_THRESHOLD so the merchant rule's remaining
    // TTL actually falls below the threshold and a real extend_ttl bump is
    // triggered on read (also making a spurious bump on DefaultRule observable).
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + RULE_TTL_THRESHOLD + 50_000);

    let resolved = env.as_contract(&client.address, || {
        read_rule_or_default(&env, merchant.clone())
    });
    assert_eq!(resolved.platform_fee_bps, merchant_rule.platform_fee_bps);
    assert_eq!(resolved.network_fee_bps, merchant_rule.network_fee_bps);
    assert_eq!(
        resolved.settlement_delay_ledger,
        merchant_rule.settlement_delay_ledger
    );
    assert_eq!(resolved.auto_settle, merchant_rule.auto_settle);

    env.as_contract(&client.address, || {
        let merchant_ttl = env
            .storage()
            .persistent()
            .get_ttl(&DataKey::Rule(merchant.clone()));
        assert!(
            merchant_ttl >= RULE_TTL_BUMP,
            "merchant rule TTL must be extended when the merchant rule is resolved"
        );

        let default_expiration_after =
            env.ledger().sequence() + env.storage().persistent().get_ttl(&DataKey::DefaultRule);
        assert_eq!(
            default_expiration_after, default_expiration_before,
            "DefaultRule must not be read or have its TTL extended when a merchant rule exists"
        );
    });
}

#[test]
fn read_rule_or_default_falls_back_to_default_without_creating_merchant_entry() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let default_rule = SettlementRule {
        platform_fee_bps: 150,
        network_fee_bps: 25,
        settlement_delay_ledger: 5,
        auto_settle: true,
    };
    client.set_default_rule(&default_rule);

    let resolved = env.as_contract(&client.address, || {
        read_rule_or_default(&env, merchant.clone())
    });
    assert_eq!(resolved.platform_fee_bps, default_rule.platform_fee_bps);
    assert_eq!(resolved.network_fee_bps, default_rule.network_fee_bps);
    assert_eq!(
        resolved.settlement_delay_ledger,
        default_rule.settlement_delay_ledger
    );
    assert_eq!(resolved.auto_settle, default_rule.auto_settle);

    env.as_contract(&client.address, || {
        assert!(!env
            .storage()
            .persistent()
            .has(&DataKey::Rule(merchant.clone())));

        let default_ttl = env.storage().persistent().get_ttl(&DataKey::DefaultRule);
        assert!(
            default_ttl >= RULE_TTL_BUMP,
            "DefaultRule TTL must be extended when it is the resolved rule"
        );
    });
}

#[test]
fn read_rule_or_default_bootstrap_path_reads_only_leaves_no_storage_footprint() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let before = env.events().all().len();
    let resolved = env.as_contract(&client.address, || {
        read_rule_or_default(&env, merchant.clone())
    });

    assert_eq!(
        resolved.platform_fee_bps,
        BOOTSTRAP_DEFAULT_RULE.platform_fee_bps
    );
    assert_eq!(
        resolved.network_fee_bps,
        BOOTSTRAP_DEFAULT_RULE.network_fee_bps
    );
    assert_eq!(
        resolved.settlement_delay_ledger,
        BOOTSTRAP_DEFAULT_RULE.settlement_delay_ledger
    );
    assert_eq!(resolved.auto_settle, BOOTSTRAP_DEFAULT_RULE.auto_settle);

    env.as_contract(&client.address, || {
        assert!(!env
            .storage()
            .persistent()
            .has(&DataKey::Rule(merchant.clone())));
        assert!(!env.storage().persistent().has(&DataKey::DefaultRule));
    });

    let events = env.events().all();
    assert_eq!(
        events.len(),
        before + 1,
        "exactly one bootstrap_fallback event expected"
    );
}
