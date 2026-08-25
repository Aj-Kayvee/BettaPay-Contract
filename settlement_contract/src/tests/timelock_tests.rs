//! Regression coverage for the settlement administrative timelock.

use crate::{Operation, DEFAULT_TIMELOCK_DELAY_SECONDS};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::Address;

use super::setup;

#[test]
fn scheduled_operation_executes_only_after_delay() {
    let (env, client, admins, _) = setup();
    let admin = admins.get(0).unwrap();
    let new_admin = Address::generate(&env);
    let operation = Operation::TransferAdmin(new_admin.clone());

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client.try_execute(&operation).is_err());
    assert_eq!(client.get_admin(), admins);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    client.execute(&operation);

    assert_eq!(client.get_admin(), soroban_sdk::vec![&env, new_admin]);
    assert_eq!(client.get_threshold(), 1);
    assert!(client.try_execute(&operation).is_err());
}

#[test]
fn schedule_rejects_non_admin_and_insufficient_delay() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);
    let non_admin = Address::generate(&env);

    assert!(client
        .try_schedule(&non_admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS)
        .is_err());
    assert!(client
        .try_schedule(
            &admins.get(0).unwrap(),
            &operation,
            &(DEFAULT_TIMELOCK_DELAY_SECONDS - 1),
        )
        .is_err());
}

#[test]
fn duplicate_schedule_is_rejected() {
    let (_env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);
    let admin = admins.get(0).unwrap();

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client
        .try_schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS)
        .is_err());
}

#[test]
fn admin_can_cancel_but_non_admin_cannot() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);
    let admin = admins.get(0).unwrap();

    client.schedule(&admin, &operation, &DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client
        .try_cancel(&Address::generate(&env), &operation)
        .is_err());
    client.cancel(&admin, &operation);

    env.ledger()
        .with_mut(|ledger| ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS);
    assert!(client.try_execute(&operation).is_err());
    assert!(client.try_cancel(&admin, &operation).is_err());
}

#[test]
#[should_panic(expected = "Error(Storage, InternalError)")]
fn expired_schedule_cannot_execute() {
    let (env, client, admins, merchant) = setup();
    let operation = Operation::RegisterMerchant(merchant);

    client.schedule(
        &admins.get(0).unwrap(),
        &operation,
        &DEFAULT_TIMELOCK_DELAY_SECONDS,
    );

    // `schedule` bumps the persistent entry to 30 days (518,400 ledgers).
    // Keep the contract instance alive while advancing past only the
    // scheduled operation's TTL.
    for _ in 0..5 {
        env.ledger()
            .with_mut(|ledger| ledger.sequence_number += 100_000);
        client.get_admin();
    }
    env.ledger().with_mut(|ledger| {
        ledger.timestamp += DEFAULT_TIMELOCK_DELAY_SECONDS + 1;
        ledger.sequence_number += 18_401;
    });

    // The host rejects access to an archived key before the contract can map
    // it to `OperationNotScheduled`, so expiry is observed as a host panic in
    // the in-memory test environment.
    client.execute(&operation);
}
