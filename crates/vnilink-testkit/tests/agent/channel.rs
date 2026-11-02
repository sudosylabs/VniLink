// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The agent's channel between two engines: who it admits, the frames it
//! carries and how long the agent offers it.

use expect_test::expect;
use std::time::Duration;

use vnilink::probes::MAX_AGENT_FRAME;
use vnilink::Error;
use vnilink_testkit::{assert_story, traced, Engine, TestResult, Traced, DEFAULT_WAIT};

/// What the channel records: the agent listening and admitting or denying
/// a computer, and the dialing side's view of each dial.
const RECORDS: &[&str] = &[
    "agent listening",
    "agent stopped listening",
    "agent channel opened",
    "agent channel denied",
    "agent channel refused",
];

/// A paired computer's frames reach the agent and its answers come back,
/// in order, at sizes up to the bound; past it is refused before anything
/// is sent.
#[test]
fn a_paired_computer_exchanges_frames_with_the_agent() -> TestResult {
    let trace = traced();
    let agent = Engine::start("agent");
    let suite = Engine::start("suite");
    agent.pair_for_control(&suite)?;
    let listener = agent.core.serve_agent()?;
    let (suite_side, agent_side) = std::thread::scope(|scope| -> Result<_, Error> {
        let dialing = scope.spawn(|| suite.core.dial_agent(&agent.endpoint()));
        let accepted = listener.accept(DEFAULT_WAIT)?;
        Ok((dialing.join().expect("the dialing thread")?, accepted))
    })?;
    assert_eq!(
        agent_side.peer_id(),
        suite.endpoint(),
        "the agent knows who dialed"
    );
    assert_eq!(
        suite_side.peer_id(),
        agent.endpoint(),
        "the suite knows which agent it reached"
    );
    suite_side.send(b"hello".to_vec())?;
    suite_side.send(vec![7; MAX_AGENT_FRAME])?;
    assert_eq!(
        agent_side.recv(DEFAULT_WAIT)?,
        b"hello",
        "frames arrive whole and in order"
    );
    assert_eq!(
        agent_side.recv(DEFAULT_WAIT)?.len(),
        MAX_AGENT_FRAME,
        "a frame at the bound arrives whole"
    );
    agent_side.send(b"ready".to_vec())?;
    assert_eq!(
        suite_side.recv(DEFAULT_WAIT)?,
        b"ready",
        "the agent's answers come back"
    );
    assert!(
        matches!(
            suite_side.send(vec![0; MAX_AGENT_FRAME + 1]),
            Err(Error::Malformed)
        ),
        "a frame past the bound is refused"
    );
    assert!(
        matches!(suite_side.send(Vec::new()), Err(Error::Malformed)),
        "an empty frame only opens a channel"
    );
    drop(agent_side);
    assert!(
        matches!(suite_side.recv(DEFAULT_WAIT), Err(Error::NoSession)),
        "a channel the agent closed says so"
    );
    assert_story(
        expect![[r#"
        agent:
          hosting: agent listening
          hosting: agent channel opened
        suite:
          session: agent channel opened
    "#]],
        || story(&trace),
    );
    Ok(())
}

/// A computer the agent does not trust is closed before a frame is read,
/// and its dial says it was refused; one that never paired with the agent
/// cannot even address it.
#[test]
fn a_computer_the_agent_does_not_trust_is_refused() -> TestResult {
    let trace = traced();
    let agent = Engine::start("agent");
    let stranger = Engine::start("stranger");
    // The stranger knows the agent; the agent does not know it.
    stranger
        .core
        .accept_invitation(&agent.core.create_invitation(Duration::from_secs(60))?)?;
    let listener = agent.core.serve_agent()?;
    assert!(
        matches!(
            stranger.core.dial_agent(&agent.endpoint()),
            Err(Error::Unauthorized)
        ),
        "an untrusted computer is refused"
    );
    assert!(
        matches!(
            listener.accept(Duration::from_millis(200)),
            Err(Error::Timeout)
        ),
        "no channel reaches the agent"
    );
    let unknown = Engine::start("unknown");
    assert!(
        matches!(
            unknown.core.dial_agent(&agent.endpoint()),
            Err(Error::NotPaired)
        ),
        "a computer dials only an agent it paired with"
    );
    assert_story(
        expect![[r#"
        agent:
          hosting: agent listening
          hosting: agent channel denied paired=false
        stranger:
          session: agent channel refused failure=unauthorized
    "#]],
        || story(&trace),
    );
    Ok(())
}

/// One listener at a time; once it drops, the protocol is no longer
/// offered, and a paired computer is refused again.
#[test]
fn the_protocol_is_offered_only_while_a_listener_lives() -> TestResult {
    let trace = traced();
    let agent = Engine::start("agent");
    let suite = Engine::start("suite");
    agent.pair_for_control(&suite)?;
    let listener = agent.core.serve_agent()?;
    assert!(
        matches!(agent.core.serve_agent(), Err(Error::Busy)),
        "a second listener is refused"
    );
    drop(listener);
    assert!(
        suite.core.dial_agent(&agent.endpoint()).is_err(),
        "an engine no longer serving the agent refuses its protocol"
    );
    let _again = agent.core.serve_agent()?;
    assert_story(
        expect![[r#"
        agent:
          hosting: agent listening
          hosting: agent stopped listening
          hosting: agent listening
        suite:
          session: agent channel refused failure=network
    "#]],
        || story(&trace),
    );
    Ok(())
}

/// The channel's records among what the engines recorded of hosting and
/// sessions.
fn story(trace: &Traced) -> String {
    trace
        .tell(&["hosting", "session"])
        .only(RECORDS)
        .to_string()
}
