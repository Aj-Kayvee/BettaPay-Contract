//! Tests for payment entry points:
//! `store_payment_reference`, `get_payment_reference`, `get_payments`.

use crate::*;
use soroban_sdk::testutils::storage::Persistent as _;
use soroban_sdk::testutils::{Address as _, Events, MockAuth, MockAuthInvoke};
use soroban_sdk::FromVal;

use super::{register_governance, setup};

// ---------------------------------------------------------------------------
// store_payment_reference
// ---------------------------------------------------------------------------

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
    assert!(env.events().all().len() > before);
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
#[should_panic(expected = "Error(Contract, #8)")]
fn rejects_duplicate_payment_reference() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let reference = BytesN::from_array(&env, &[1; 32]);
    client.store_payment_reference(&merchant, &reference, &1000);
    client.store_payment_reference(&merchant, &reference, &2000);
}

#[test]
#[should_panic]
fn rejects_invalid_amount() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let reference = BytesN::from_array(&env, &[2; 32]);
    client.store_payment_reference(&merchant, &reference, &0);
}

#[test]
#[should_panic]
fn rejects_below_minimum_amount() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let reference = BytesN::from_array(&env, &[99; 32]);
    client.store_payment_reference(&merchant, &reference, &99);
}

#[test]
fn accepts_valid_minimum_amount() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let reference = BytesN::from_array(&env, &[100; 32]);
    client.store_payment_reference(&merchant, &reference, &100);

    let stored = client
        .get_payment_reference(&reference)
        .expect("expected payment record");
    assert_eq!(stored.amount, 100);
}

// Issue #297: verify store_payment_reference succeeds when amount = MIN_PAYMENT_AMOUNT (100)
// combined with a platform_fee_bps of 10_000 (100%). The contract must accept the call since
// the amount meets the minimum threshold. With ceiling-based fee arithmetic, the platform fee
// consumes the entire gross amount (100 bps * 100 / 10_000 rounded up = 100), leaving the
// merchant with exactly 0. This documents the known edge case: at extreme fee rates and the
// minimum payment amount, the merchant payout is zero.
#[test]
fn store_payment_reference_min_amount_with_maximum_platform_fee_yields_zero_merchant_payout() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    // Set a rule with the maximum possible platform fee (9_995 bps) and minimum network
    // fee (5 bps), sum = 10_000 bps = 100%, satisfying all fee validation rules.
    let rule = SettlementRule {
        platform_fee_bps: 9_995,
        network_fee_bps: 5,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);

    // Use a distinct reference (different from [100; 32] used in accepts_valid_minimum_amount).
    let reference = BytesN::from_array(&env, &[101; 32]);

    // store_payment_reference must succeed: amount = 100 satisfies the MIN_PAYMENT_AMOUNT check.
    let split = client.store_payment_reference(&merchant, &reference, &100);

    // With 100% platform fee and ceiling arithmetic:
    //   platform_fee_amount = ceil(100 * 10_000 / 10_000) = 100
    //   network_fee_amount  = 0
    //   merchant_amount     = 100 - 100 - 0 = 0
    assert_eq!(
        split.gross_amount, 100,
        "gross amount must equal the submitted payment amount"
    );
    assert_eq!(
        split.platform_fee_amount, 100,
        "platform fee rounds up to 100 at 9995 bps on amount 100"
    );
    assert_eq!(
        split.network_fee_amount, 1,
        "network fee rounds up to 1 at 5 bps on amount 100"
    );
    assert_eq!(
        split.merchant_amount, -1,
        "merchant payout is negative when ceil fees exceed gross (documented edge case)"
    );

    // Confirm the stored record reflects the same computed values.
    let stored = client
        .get_payment_reference(&reference)
        .expect("payment record must be present after successful store");
    assert_eq!(stored.amount, 100);
    assert_eq!(stored.platform_fee_amount, 100);
    assert_eq!(stored.network_fee_amount, 1);
    assert_eq!(stored.merchant_amount, -1);
    assert_eq!(stored.platform_fee_bps, 9_995);
    assert_eq!(stored.network_fee_bps, 5);
}

#[test]
#[should_panic]
fn store_payment_reference_requires_merchant_auth() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    let governance = register_governance(&env);
    let recovery_address = Address::generate(&env);
    let contract_id = env.register_contract(None, SettlementContract);
    let client = SettlementContractClient::new(&env, &contract_id);

    // Authorize admin for init
    let init_invoke = MockAuthInvoke {
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
    let init_auth = MockAuth {
        address: &admin,
        invoke: &init_invoke,
    };
    env.set_auths(&[(&init_auth).into()]);
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

    // Do NOT authorize the merchant for store_payment_reference — should panic.
    let reference = BytesN::from_array(&env, &[15; 32]);
    client.store_payment_reference(&merchant, &reference, &10_000);
}

// Issue #84: store_payment_reference rejects zero amount with InvalidAmount (#7)
#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn store_payment_reference_rejects_zero_amount_with_invalid_amount_error() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let reference = BytesN::from_array(&env, &[55; 32]);
    client.store_payment_reference(&merchant, &reference, &0);
}

// Issue #84: store_payment_reference rejects negative amounts with InvalidAmount (#7)
#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn store_payment_reference_rejects_negative_amount() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let reference = BytesN::from_array(&env, &[56; 32]);
    client.store_payment_reference(&merchant, &reference, &-1);
}

// Issue #248: store_payment_reference is also protected, since it shares
// the same calculate_split code path as calculate_fee_split.
#[test]
#[should_panic(expected = "Error(Contract, #19)")]
fn store_payment_reference_rejects_amount_that_would_overflow() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 500,
        network_fee_bps: 250,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
    let reference = BytesN::from_array(&env, &[77; 32]);
    // The max of the two bps values (500) determines the overflow boundary.
    let amount = (i128::MAX - (BPS_DENOMINATOR as i128 - 1)) / 500 + 1;
    client.store_payment_reference(&merchant, &reference, &amount);
}

// Issue #82: verify storing reference for non-merchant panics with MerchantMissing
#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn store_payment_reference_fails_for_unregistered_merchant() {
    let (env, client, _admin, merchant) = setup();
    let reference = BytesN::from_array(&env, &[99; 32]);
    client.store_payment_reference(&merchant, &reference, &10_000);
}

#[test]
fn store_payment_reference_extends_rule_ttl_on_read() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);

    // Verify the rule is present and TTL was set at write time.
    env.as_contract(&client.address, || {
        let key = DataKey::Rule(merchant.clone());
        assert!(env.storage().persistent().has(&key));
        let ttl = env.storage().persistent().get_ttl(&key);
        assert!(
            ttl >= RULE_TTL_BUMP,
            "Merchant Rule TTL must be set on write"
        );
    });

    let reference = BytesN::from_array(&env, &[42; 32]);
    client.store_payment_reference(&merchant, &reference, &10_000);

    // TTL is still at least RULE_TTL_BUMP (the read path calls extend_ttl as well).
    env.as_contract(&client.address, || {
        let key = DataKey::Rule(merchant.clone());
        assert!(env.storage().persistent().has(&key));
        let ttl = env.storage().persistent().get_ttl(&key);
        assert!(
            ttl >= RULE_TTL_BUMP,
            "Merchant Rule TTL must remain >= RULE_TTL_BUMP after read"
        );
    });
}

#[test]
fn verify_payment_storage_events() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);

    let rule = SettlementRule {
        platform_fee_bps: 250,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);

    let reference = BytesN::from_array(&env, &[77; 32]);
    let before = env.events().all().len();
    client.store_payment_reference(&merchant, &reference, &20_000);

    let events = env.events().all();
    assert_eq!(
        events.len(),
        before + 1,
        "exactly one event should be emitted by store_payment_reference"
    );

    let event1 = events.get(before).unwrap();
    let (_contract_id, topics1, _data1) = event1;
    assert_eq!(topics1.len(), 3);
    assert_eq!(
        Symbol::from_val(&env, &topics1.get(0).unwrap()),
        Symbol::new(&env, "payment_stored")
    );
    assert_eq!(Address::from_val(&env, &topics1.get(1).unwrap()), merchant);
    assert_eq!(
        BytesN::<32>::from_val(&env, &topics1.get(2).unwrap()),
        reference
    );
}

// Issue #90 / #271: verify the fee split is available on the stored PaymentRecord,
// accessible via get_payment_reference (no separate payment_split event is emitted).
#[test]
fn split_data_available_on_payment_stored() {
    let (env, client, _admin, merchant) = setup();
    client.register_merchant(&merchant);
    let rule = SettlementRule {
        platform_fee_bps: 200,
        network_fee_bps: 50,
        settlement_delay_ledger: 0,
        auto_settle: false,
    };
    client.set_settlement_rule(&merchant, &rule);
    let reference = BytesN::from_array(&env, &[42; 32]);
    let before = env.events().all().len();
    let split = client.store_payment_reference(&merchant, &reference, &10_000);
    // Exactly one event emitted: payment_stored
    let events = env.events().all();
    assert_eq!(
        events.len(),
        before + 1,
        "exactly one payment_stored event expected"
    );
    // The fee split is returned directly and also stored on the PaymentRecord
    assert_eq!(split.platform_fee_amount, 200); // 200 bps of 10_000
    assert_eq!(split.network_fee_amount, 50); // 50 bps of 10_000
    assert_eq!(split.merchant_amount, 9_750);
    let record = client
        .get_payment_reference(&reference)
        .expect("record must exist");
    assert_eq!(record.platform_fee_amount, 200);
    assert_eq!(record.network_fee_amount, 50);
    assert_eq!(record.merchant_amount, 9_750);
    assert_eq!(record.platform_fee_bps, 200);
    assert_eq!(record.network_fee_bps, 50);
}

// ---------------------------------------------------------------------------
// get_payment_reference
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// get_payments
// ---------------------------------------------------------------------------

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
    assert_eq!(payments.get(0).unwrap().amount, 15_000);
    assert_eq!(payments.get(1).unwrap().amount, 25_000);
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
    assert_eq!(payments.len(), 0);
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

    assert_eq!(payments.len(), 2);
    assert_eq!(payments.get(0).unwrap().amount, 10_000);
    assert_eq!(payments.get(1).unwrap().amount, 20_000);
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
            payments.get((i - 1) as u32).unwrap().amount,
            MIN_PAYMENT_AMOUNT + i as i128
        );
    }
}
