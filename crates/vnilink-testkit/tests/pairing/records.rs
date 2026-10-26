//! Opening an installation: a store that cannot be read or holds no valid
//! record is left as it was, a first save that fails keeps no identity,
//! an engine without a store saves nothing, and the relay a saved change
//! needs a restart for.

use vnilink::{Error, RelaySettings};
use vnilink_testkit::relay::LocalRelay;
use vnilink_testkit::{traced, TestResult};

use super::support::{credentialed_relay, start_storeless, Fault, Installed, TestStore};

/// A store's failures say what the owner must do, and a store that is
/// locked or unavailable opens no engine and is written nothing.
#[test]
fn locked_and_unavailable_stores_fail_without_writing() -> TestResult {
    let _trace = traced();
    for (error, says) in [
        (Error::StoreLocked, "the credential store is locked"),
        (
            Error::StoreUnavailable,
            "the installation store is unavailable",
        ),
        (
            Error::StoreRecoveryRequired,
            "the installation store must be reopened and VniLink restarted",
        ),
        (Error::CorruptRecord, "the installation record is not valid"),
    ] {
        assert_eq!(error.to_string(), says, "{error:?} says what happened");
    }
    for (fault, refused) in [
        (Fault::Locked, "a locked store"),
        (Fault::Unavailable, "an unavailable store"),
    ] {
        let store = TestStore::refusing(fault);
        let opened = Installed::open_in("host", store.clone(), RelaySettings::public_n0());
        match (fault, opened.map(drop)) {
            (Fault::Locked, Err(Error::StoreLocked))
            | (Fault::Unavailable, Err(Error::StoreUnavailable)) => {}
            (_, other) => panic!("{refused} opens no engine and says why: {other:?}"),
        }
        assert!(store.record().is_none(), "{refused} is written nothing");
        assert_eq!(store.save_attempts(), 0, "{refused} is asked no save");
    }
    Ok(())
}

/// A record that is not valid opens no engine and is left as it was; the
/// valid record it was made from still opens the same installation, also
/// left as it was.
#[test]
fn a_corrupt_record_is_left_unchanged() -> TestResult {
    let _trace = traced();
    let store = TestStore::default();
    let id = Installed::open_in("host", store.clone(), RelaySettings::public_n0())?
        .core
        .endpoint_id();
    let good = store.record().ok_or("the first open saves a record")?;
    let mut corrupt = good.clone();
    corrupt[10] ^= 0x5a;
    let broken = TestStore::containing(corrupt.clone());
    assert!(
        matches!(
            Installed::open_in("host", broken.clone(), RelaySettings::public_n0()).map(drop),
            Err(Error::CorruptRecord)
        ),
        "a damaged record opens no engine"
    );
    assert_eq!(
        broken.record(),
        Some(corrupt),
        "a damaged record is left as it was"
    );

    let host = Installed::open_in("host", store.clone(), RelaySettings::public_n0())?;
    assert_eq!(host.core.endpoint_id(), id, "the valid record still opens");
    assert_eq!(store.record(), Some(good), "opening it rewrites nothing");
    Ok(())
}

/// When the first save of a new installation fails, no engine opens and
/// no identity is kept; the next open makes one and keeps it.
#[test]
fn the_first_save_failure_does_not_keep_an_identity() -> TestResult {
    let _trace = traced();
    let store = TestStore::default();
    store.fail_next_save(Fault::Unavailable);
    assert!(
        matches!(
            Installed::open_in("host", store.clone(), RelaySettings::public_n0()).map(drop),
            Err(Error::StoreUnavailable)
        ),
        "a first save that fails opens no engine"
    );
    assert!(store.record().is_none(), "and keeps no identity");

    let id = Installed::open_in("host", store.clone(), RelaySettings::public_n0())?
        .core
        .endpoint_id();
    assert_eq!(
        Installed::open_in("host", store, RelaySettings::public_n0())?
            .core
            .endpoint_id(),
        id,
        "the identity the next open made is kept"
    );
    Ok(())
}

/// A first save whose outcome is uncertain opens no engine, keeps no
/// identity, and is tried once.
#[test]
fn the_first_uncertain_save_does_not_keep_an_identity() -> TestResult {
    let _trace = traced();
    let store = TestStore::default();
    store.fail_next_save(Fault::RecoveryRequired);
    assert!(
        matches!(
            Installed::open_in("host", store.clone(), RelaySettings::public_n0()).map(drop),
            Err(Error::StoreRecoveryRequired)
        ),
        "an uncertain first save opens no engine"
    );
    assert!(store.record().is_none(), "and keeps no identity");
    assert_eq!(store.save_attempts(), 1, "the save is tried once");
    Ok(())
}

/// An engine started without a store refuses what must be saved, the relay
/// and a new identity, and keeps working as it was.
#[test]
fn a_core_without_a_store_does_not_save() -> TestResult {
    let _trace = traced();
    let relay = LocalRelay::start();
    let core = start_storeless("host", &relay, None, 64)?;
    let id = core.endpoint_id();
    assert!(
        matches!(
            core.set_relay(RelaySettings::public_n0()),
            Err(Error::InvalidSetting(_))
        ),
        "the relay cannot be saved without a store"
    );
    assert!(
        matches!(core.reset_identity(), Err(Error::InvalidSetting(_))),
        "nor a new identity"
    );
    assert_eq!(core.endpoint_id(), id, "the engine keeps its identity");
    assert!(
        core.snapshot()?.devices.is_empty(),
        "and keeps working as it was"
    );
    core.shutdown();
    Ok(())
}

/// A saved relay that differs from the bound one, its credential alone
/// included, needs a restart; saving the bound one again needs none.
#[test]
fn relay_restart_required_includes_credentials_and_clears_when_reverted() -> TestResult {
    let _trace = traced();
    let original = credentialed_relay("relay-credential-alpha")?;
    let host = Installed::open_in("host", TestStore::default(), original.clone())?;
    assert!(
        !host.core.relay_restart_required()?,
        "the bound relay needs no restart"
    );
    host.core
        .set_relay(credentialed_relay("relay-credential-beta")?)?;
    assert!(
        host.core.relay_restart_required()?,
        "another credential needs a restart"
    );
    host.core.set_relay(original)?;
    assert!(
        !host.core.relay_restart_required()?,
        "the bound relay saved again needs none"
    );
    Ok(())
}
