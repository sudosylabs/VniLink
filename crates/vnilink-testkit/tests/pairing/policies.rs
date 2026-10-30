// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What an owner chose for this computer, kept in its installation: what
//! each peer may use, and the terminal, file and power policies. Each
//! survives a restart, and a change whose save fails is undone.

use std::time::Duration;

use vnilink::job::{FileResourcePolicy, FileRetentionPolicy};
use vnilink::power::PowerPolicy;
use vnilink::terminal::TerminalPolicy;
use vnilink::{Error, ServiceKind, ServicePermissions};
use vnilink_testkit::store::{Fault, Installed};
use vnilink_testkit::{traced, Engine, TestResult};

use super::support::EVERYTHING;

/// What the host's peer may use once the host shares everything but Files
/// and system actions.
const ALL_BUT_FILES: ServicePermissions = ServicePermissions {
    desktop: true,
    files: false,
    terminal: true,
    camera: true,
    system_audio: true,
    microphone: true,
    recording: true,
    system_actions: false,
};

/// Each service is shared on its own and kept; a change, or a revocation,
/// whose save fails leaves what the peer may use and the record as they
/// were; a refreshed invitation keeps the peer's permissions, and so does a
/// restart; forgetting the peer forgets them while the sharing stays.
#[test]
fn service_permissions_are_independent_persisted_and_rollback_before_revocation() -> TestResult {
    let _trace = traced();
    let mut host = Installed::faulty("host");
    let controller = Engine::start("controller");
    let lifetime = Duration::from_secs(60);
    host.core
        .accept_invitation(&controller.core.create_invitation(lifetime)?)?;
    let id = controller.endpoint();
    assert_eq!(
        host.core.service_permissions(&id)?,
        ServicePermissions::default(),
        "a new peer may use nothing"
    );
    for kind in [
        ServiceKind::Terminal,
        ServiceKind::Camera,
        ServiceKind::SystemAudio,
        ServiceKind::Microphone,
    ] {
        host.core.set_service_sharing(kind, true)?;
    }
    host.core.set_recording_sharing(true)?;
    host.core.set_desktop_sharing(EVERYTHING)?;
    assert_eq!(
        host.core.service_permissions(&id)?,
        ServicePermissions {
            files: true,
            ..ALL_BUT_FILES
        },
        "the peer may use what is shared, Files with transfer"
    );
    host.core.set_service_sharing(ServiceKind::Files, false)?;
    assert_eq!(
        host.core.service_permissions(&id)?,
        ALL_BUT_FILES,
        "Files is withdrawn on its own"
    );
    assert!(
        host.core.snapshot()?.devices[0].grants.clipboard,
        "withdrawing Files keeps the clipboard"
    );
    let saved = host.store.record().ok_or("a saved record")?;
    host.store.fail_next_save(Fault::Locked);
    assert!(
        matches!(
            host.core.set_service_sharing(ServiceKind::Terminal, false),
            Err(Error::StoreLocked)
        ),
        "withdrawing Terminal whose save fails is refused"
    );
    assert!(
        host.core.service_permissions(&id)?.terminal,
        "the peer keeps Terminal"
    );
    assert_eq!(
        host.store.record().as_ref(),
        Some(&saved),
        "the record is as it was"
    );
    host.store.fail_next_save(Fault::Locked);
    assert!(
        matches!(
            host.core
                .set_service_sharing(ServiceKind::SystemAudio, false),
            Err(Error::StoreLocked)
        ),
        "withdrawing system audio whose save fails is refused"
    );
    assert!(
        host.core.service_permissions(&id)?.system_audio,
        "the peer keeps system audio"
    );
    host.store.fail_next_save(Fault::Locked);
    assert!(
        matches!(
            host.core.set_recording_sharing(false),
            Err(Error::StoreLocked)
        ),
        "withdrawing recording whose save fails is refused"
    );
    assert!(
        host.core.service_permissions(&id)?.recording,
        "the peer keeps recording"
    );
    assert_eq!(
        host.store.record().as_ref(),
        Some(&saved),
        "the record is as it was"
    );
    host.store.fail_next_save(Fault::Unavailable);
    assert!(
        matches!(host.core.revoke(&id), Err(Error::StoreUnavailable)),
        "a revocation whose save fails is refused"
    );
    assert_eq!(
        host.core.service_permissions(&id)?,
        ALL_BUT_FILES,
        "the peer keeps what it may use"
    );
    assert_eq!(host.store.record(), Some(saved), "the record is as it was");
    // A refreshed invitation is the peer's news, not a withdrawal.
    host.core
        .accept_invitation(&controller.core.create_invitation(lifetime)?)?;
    assert!(
        host.core.service_permissions(&id)?.camera,
        "a refreshed invitation keeps the peer's permissions"
    );
    host.restart();
    assert_eq!(
        host.core.service_permissions(&id)?,
        ALL_BUT_FILES,
        "a restart keeps the peer's permissions"
    );
    host.core.unpair(&id)?;
    assert!(
        matches!(host.core.service_permissions(&id), Err(Error::NotPaired)),
        "a forgotten peer has no permissions"
    );
    host.core.set_service_sharing(ServiceKind::Camera, true)?;
    assert!(
        host.core.sharing_policy()?.camera,
        "sharing outlives the peers it was granted to"
    );
    Ok(())
}

/// The terminal policy is kept across a restart; a change whose save fails
/// and a policy that is not one are refused, leaving it and the record as
/// they were.
#[test]
fn terminal_policy_is_durable_and_save_failure_rolls_back() -> TestResult {
    let _trace = traced();
    let mut host = Installed::faulty("host");
    let policy = TerminalPolicy {
        allow_persistence: false,
        max_sessions: 3,
        max_sessions_per_peer: 1,
        history_bytes: 32 * 1024,
        replay_bytes: 1024,
        detached_timeout_secs: 17,
    };
    host.core.set_terminal_policy(policy)?;
    let saved = host.store.record().ok_or("a saved record")?;
    host.store.fail_next_save(Fault::Unavailable);
    assert!(
        matches!(
            host.core.set_terminal_policy(TerminalPolicy::default()),
            Err(Error::StoreUnavailable)
        ),
        "a policy whose save fails is refused"
    );
    assert_eq!(
        host.core.terminal_policy()?,
        policy,
        "the policy is as it was"
    );
    assert_eq!(
        host.store.record().as_ref(),
        Some(&saved),
        "and so is the record"
    );
    host.restart();
    assert_eq!(
        host.core.terminal_policy()?,
        policy,
        "a restart keeps the policy"
    );
    let mut invalid = policy;
    invalid.replay_bytes = invalid.history_bytes + 1;
    assert!(
        matches!(
            host.core.set_terminal_policy(invalid),
            Err(Error::InvalidSetting(_))
        ),
        "a replay longer than the history is refused"
    );
    assert_eq!(
        host.core.terminal_policy()?,
        policy,
        "the policy is as it was"
    );
    assert_eq!(host.store.record(), Some(saved), "and so is the record");
    Ok(())
}

/// The file retention policy is kept across a restart; a change whose save
/// fails, and one that is not a policy, are refused.
#[test]
fn file_retention_policy_is_durable_and_save_failure_rolls_back() -> TestResult {
    let _trace = traced();
    let mut host = Installed::faulty("host");
    let policy = FileRetentionPolicy {
        resume_cache_bytes: 12_345,
        resume_cache_age_secs: 456,
        recent_fetch_bytes: 789,
        recent_fetch_age_secs: 10,
    };
    host.core.set_file_retention_policy(policy)?;
    let saved = host.store.record().ok_or("a saved record")?;
    host.store.fail_next_save(Fault::Unavailable);
    assert!(
        matches!(
            host.core.set_file_retention_policy(FileRetentionPolicy {
                resume_cache_bytes: 0,
                resume_cache_age_secs: 0,
                recent_fetch_bytes: 0,
                recent_fetch_age_secs: 0
            }),
            Err(Error::StoreUnavailable)
        ),
        "a policy whose save fails is refused"
    );
    assert_eq!(
        host.core.file_retention_policy()?,
        policy,
        "the policy is as it was"
    );
    assert_eq!(host.store.record(), Some(saved), "and so is the record");
    let mut invalid = policy;
    invalid.resume_cache_bytes = u64::MAX;
    assert!(
        matches!(
            host.core.set_file_retention_policy(invalid),
            Err(Error::InvalidSetting(_))
        ),
        "a cache without bound is refused"
    );
    host.restart();
    assert_eq!(
        host.core.file_retention_policy()?,
        policy,
        "a restart keeps the policy"
    );
    Ok(())
}

/// The power policy is kept across a restart; a change whose save fails
/// leaves it and the record as they were.
#[test]
fn power_policy_is_durable_and_save_failure_rolls_back() -> TestResult {
    let _trace = traced();
    let mut host = Installed::faulty("host");
    let policy = PowerPolicy {
        incoming_display: true,
        outgoing_system: true,
        incoming_idle_secs: 73,
        outgoing_idle_secs: 94,
        ..Default::default()
    };
    host.core.set_power_policy(policy)?;
    let saved = host.store.record().ok_or("a saved record")?;
    host.store.fail_next_save(Fault::Unavailable);
    assert!(
        matches!(
            host.core.set_power_policy(PowerPolicy::default()),
            Err(Error::StoreUnavailable)
        ),
        "a policy whose save fails is refused"
    );
    assert_eq!(host.core.power_policy()?, policy, "the policy is as it was");
    assert_eq!(host.store.record(), Some(saved), "and so is the record");
    host.restart();
    assert_eq!(
        host.core.power_policy()?,
        policy,
        "a restart keeps the policy"
    );
    Ok(())
}

/// The file resource policy is kept across a restart; a change whose save
/// fails, and each policy that is not one, leave it and the record as they
/// were.
#[test]
fn file_resource_policy_roundtrips_and_save_failure_preserves_durable_state() -> TestResult {
    let _trace = traced();
    let mut host = Installed::faulty("host");
    let policy = FileResourcePolicy {
        max_concurrent_receivers: 2,
        max_reserved_bytes: 123_456,
        minimum_free_bytes: 7890,
    };
    host.core.set_file_resource_policy(policy)?;
    let saved = host.store.record().ok_or("a saved record")?;
    host.store.fail_next_save(Fault::Unavailable);
    assert!(
        matches!(
            host.core.set_file_resource_policy(Default::default()),
            Err(Error::StoreUnavailable)
        ),
        "a policy whose save fails is refused"
    );
    assert_eq!(
        host.core.file_resource_policy()?,
        policy,
        "the policy is as it was"
    );
    assert_eq!(
        host.store.record().as_ref(),
        Some(&saved),
        "and so is the record"
    );
    for invalid in [
        FileResourcePolicy {
            max_concurrent_receivers: 0,
            ..policy
        },
        FileResourcePolicy {
            max_concurrent_receivers: 17,
            ..policy
        },
        FileResourcePolicy {
            max_reserved_bytes: 0,
            ..policy
        },
        FileResourcePolicy {
            minimum_free_bytes: u64::MAX,
            ..policy
        },
    ] {
        assert!(
            host.core.set_file_resource_policy(invalid).is_err(),
            "{invalid:?} is refused"
        );
        assert_eq!(
            host.core.file_resource_policy()?,
            policy,
            "the policy is as it was"
        );
        assert_eq!(
            host.store.record().as_ref(),
            Some(&saved),
            "and so is the record"
        );
    }
    host.restart();
    assert_eq!(
        host.core.file_resource_policy()?,
        policy,
        "a restart keeps the policy"
    );
    Ok(())
}
