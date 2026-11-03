// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What several pairing scenarios share: everything a host can share,
//! pairing through a pairing file, a controller connecting through the
//! host's owner's answer and the approval that follows a file ([`check`]),
//! reading what one engine knows of another, and a relay of the computer's
//! own. Installations whose store the test holds are the kit's
//! ([`vnilink_testkit::store`]).

use expect_test::Expect;
use std::time::{Duration, Instant};

use vnilink::{
    ControlRequest, Core, CustomRelay, DeviceInfo, Error, Grants, PairingFile, RelaySettings,
    SessionInfo, SessionPhase,
};
use vnilink_testkit::session::{other_check, wait_control_check, wait_request};
use vnilink_testkit::{assert_story, wait_for, Engine, Traced};

/// Everything a host can share.
pub const EVERYTHING: Grants = Grants {
    view: true,
    control: true,
    clipboard: true,
    transfer: true,
};

/// How long a pairing file in these scenarios lives.
pub const FILE_LIFETIME: Duration = Duration::from_secs(60);

/// How long the host takes to ask its owner, and the controller to show
/// its digits, at most.
pub const ASKING: Duration = Duration::from_secs(5);

/// What a scenario fails with: an engine's error, or what it did not find.
pub type Failure = Box<dyn std::error::Error>;

/// The controller opens `host`'s pairing file and confirms its digits: the
/// controller remembers the host, and the host may approve the controller
/// once when `allow_control`.
pub fn exchange(host: &Core, controller: &Core, allow_control: bool) -> Result<PairingFile, Error> {
    let file = host.export_pairing_file(Duration::from_secs(60), allow_control)?;
    let preview = controller.open_pairing_file(&file.bytes)?;
    controller.confirm_pairing(&preview, &file.check)?;
    Ok(file)
}

/// What `core` remembers of the peer `endpoint_id`.
pub fn device(core: &Core, endpoint_id: &str) -> Result<DeviceInfo, Failure> {
    Ok(core
        .snapshot()?
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == endpoint_id)
        .ok_or("a paired device")?)
}

/// A relay of its own, with the credential `token`.
pub fn credentialed_relay(token: &str) -> Result<RelaySettings, Error> {
    let relay = CustomRelay::new("https://relay.example.com./")?.with_auth_token(token)?;
    RelaySettings::custom(vec![relay])
}

/// How the host's owner answers the request a connecting controller
/// raises, once the controller shows its digits.
#[derive(Clone, Copy, Debug)]
pub enum Answer {
    /// Types the digits the controller shows.
    Digits,
    /// Types digits that differ from them in their last place.
    WrongDigits,
    /// Types the digits the controller shows once `after` has passed since
    /// it was asked: the owner reads them slowly.
    DigitsAfter(Duration),
    /// Rejects the request.
    Reject,
}

/// One connect that the host's owner answered.
pub struct Asked {
    /// What the host asked its owner.
    pub request: ControlRequest,
    /// The digits the controller showed.
    pub shown: String,
    /// What the owner's answer returned: the controller it approved, if
    /// any.
    pub answered: Result<Option<DeviceInfo>, Error>,
    /// What the controller's connect returned.
    pub connected: Result<SessionInfo, Error>,
}

/// `controller` connects to `host`; once the host asks its owner and the
/// controller shows its digits, the owner answers as `answer` says. Both
/// sides' outcomes are the test's to state.
#[track_caller]
pub fn ask(host: &Engine, controller: &Engine, answer: Answer) -> Result<Asked, Error> {
    let events = controller.core.subscribe()?;
    Ok(std::thread::scope(|scope| {
        let connecting = scope.spawn(|| controller.connect_services(host));
        let request = wait_request(&host.core, ASKING);
        let shown = wait_control_check(&events, ASKING);
        let asked = Instant::now();
        let answered = match answer {
            Answer::Digits => host.core.confirm_control_request(&shown).map(Some),
            Answer::WrongDigits => host
                .core
                .confirm_control_request(&other_check(&shown))
                .map(Some),
            Answer::DigitsAfter(after) => {
                // The delay is the scenario: an invitation's lifetime is
                // wall-clock time on the host, so only time passing ends it.
                wait_for("the owner to answer late", after * 2, || {
                    (asked.elapsed() >= after).then_some(())
                });
                host.core.confirm_control_request(&shown).map(Some)
            }
            Answer::Reject => host.core.reject_control_request().map(|()| None),
        };
        let connected = connecting.join().expect("the connecting thread returns");
        Asked {
            request,
            shown,
            answered,
            connected,
        }
    }))
}

/// How the host's owner is asked about a controller that opened its
/// pairing file.
pub struct Asking<'a> {
    /// How long the host's pairing file lives.
    pub lifetime: Duration,
    /// How the owner answers each connect, in turn.
    pub answers: &'a [Answer],
}

/// What [`check`] asked: the host, the controller, and each connect the
/// host's owner answered.
pub struct Approval {
    pub host: Engine,
    pub controller: Engine,
    pub asked: Vec<Asked>,
}

/// Starts a host and a controller; the controller opens the host's pairing
/// file, made with the control switch to live `asking.lifetime`, and
/// confirms its digits; then it connects once for each of
/// `asking.answers`, the host's owner answering as that says ([`ask`]).
/// Asserts of each connect that the host asked about the controller, which
/// showed six digits; that the request is gone once answered; that an
/// approval opens the link and has the host remember the controller, while
/// any other answer turns the controller away and leaves the host
/// remembering nobody. Then that no record holds either computer's full
/// identity, and that the engines told the story `expect` of their
/// pairing. What a test checks beyond that it checks on the [`Approval`]
/// returned.
#[track_caller]
pub fn check(trace: &Traced, asking: Asking<'_>, expect: Expect) -> Result<Approval, Failure> {
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let file = host.core.export_pairing_file(asking.lifetime, true)?;
    let preview = controller.core.open_pairing_file(&file.bytes)?;
    controller.core.confirm_pairing(&preview, &file.check)?;
    host.core.start_hosting()?;
    let mut asked = Vec::new();
    let mut approved = false;
    for &answer in asking.answers {
        let answered = ask(&host, &controller, answer)?;
        assert_eq!(
            answered.request.endpoint_id,
            controller.endpoint(),
            "{answer:?}: the host asks about the controller"
        );
        assert!(
            answered.shown.len() == 6 && answered.shown.bytes().all(|b| b.is_ascii_digit()),
            "{answer:?}: the controller shows six digits: {}",
            answered.shown
        );
        assert!(
            host.core.control_request()?.is_none(),
            "{answer:?}: an answered request is gone"
        );
        if let Answer::Reject = answer {
            assert!(
                matches!(answered.answered, Ok(None)),
                "the owner's rejection is taken: {:?}",
                answered.answered
            );
        }
        if matches!(answered.answered, Ok(Some(_))) {
            approved = true;
            assert!(
                matches!(
                    &answered.connected,
                    Ok(session) if session.phase == SessionPhase::Active
                ),
                "{answer:?}: an approval opens the link: {:?}",
                answered.connected
            );
        } else {
            assert!(
                matches!(answered.connected, Err(Error::Unauthorized)),
                "{answer:?}: a controller not approved is turned away: {:?}",
                answered.connected
            );
        }
        assert_eq!(
            host.core.snapshot()?.devices.len(),
            usize::from(approved),
            "{answer:?}: the host remembers the controller only once approved"
        );
        asked.push(answered);
    }
    assert_story(expect, || trace.story(&["pairing"]));
    trace.assert_unrecorded(
        "a full peer identity",
        &[&host.endpoint(), &controller.endpoint()],
    );
    Ok(Approval {
        host,
        controller,
        asked,
    })
}
