//! What the services scenarios share: a host sharing its whole disk, a
//! connection waited back after a redial, and the closes an engine published.

use vnilink::{
    CloseCause, CoreEvent, EventSubscription, Grants, ServiceInfo, ServiceKind, ServicePhase,
    SessionInfo, SessionPhase,
};
use vnilink_testkit::{wait_for, Engine, Traced, DEFAULT_WAIT};

/// What a host grants a viewer it may see and control, and no more.
pub(super) const VIEW_AND_CONTROL: Grants = Grants {
    view: true,
    control: true,
    clipboard: false,
    transfer: false,
};

/// `host` hosts, trusts `viewer` and lets it see and control, as two
/// computers do once paired.
pub(super) fn pair_for_control(host: &Engine, viewer: &Engine) -> Result<(), vnilink::Error> {
    host.pair_with(viewer)?;
    host.core.set_desktop_sharing(VIEW_AND_CONTROL)
}

/// Shares `engine`'s whole disk rather than its one folder, as the
/// application does, and names the engine's folder from the disk's root:
/// the root's id and the path within it.
pub(super) fn whole_disk(engine: &Engine) -> Result<(u64, String), Box<dyn std::error::Error>> {
    engine.core.enable_filesystem_access()?;
    let folder = std::fs::canonicalize(engine.root())?;
    let (id, base) = engine
        .core
        .file_roots()?
        .into_iter()
        .find(|(_, path)| folder.starts_with(path))
        .ok_or("a disk root holds the engine's folder")?;
    let path = folder
        .strip_prefix(&base)?
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    Ok((id, path))
}

/// `connection` on `controller` once it is active again under a newer
/// generation, after its link dropped and it dialled again.
pub(super) fn redialed(controller: &Engine, connection: &SessionInfo) -> SessionInfo {
    wait_for("the connection to dial again", DEFAULT_WAIT, || {
        let next = controller.core.controller_session(connection.id).ok()?;
        (next.phase == SessionPhase::Active && next.generation > connection.generation)
            .then_some(next)
    })
}

/// `session` on `engine` once it reached `phase`.
pub(super) fn reaches(engine: &Engine, session: u64, phase: SessionPhase) -> SessionInfo {
    wait_for(
        &format!("the session to reach {phase:?}"),
        DEFAULT_WAIT,
        || {
            engine
                .core
                .controller_session(session)
                .ok()
                .filter(|info| info.phase == phase)
        },
    )
}

/// The phase of `service` on the controller now.
pub(super) fn phase(controller: &Engine, service: &ServiceInfo) -> ServicePhase {
    controller
        .core
        .controller_service(service.id)
        .expect("the controller keeps the service's record")
        .phase
}

/// The hosted services of `kind` that `events` says closed for `cause`,
/// from what it holds now.
pub(super) fn hosted_closes(
    events: &EventSubscription,
    kind: ServiceKind,
    cause: CloseCause,
) -> Vec<ServiceInfo> {
    let mut closed = Vec::new();
    while let Some(event) = events.try_recv() {
        if let CoreEvent::HostedServiceChanged { service } = event {
            if service.kind == kind && service.phase == ServicePhase::Closed(cause) {
                closed.push(service);
            }
        }
    }
    closed
}

/// Waits until `events` says a hosted service of `kind` closed for `cause`.
pub(super) fn wait_hosted_close(
    events: &EventSubscription,
    kind: ServiceKind,
    cause: CloseCause,
) -> ServiceInfo {
    wait_for(
        &format!("the host to publish its {kind:?} close for {cause:?}"),
        DEFAULT_WAIT,
        || hosted_closes(events, kind, cause).into_iter().next(),
    )
}

/// The controller-side service changes `events` holds now, in order.
pub(super) fn service_changes(events: &EventSubscription) -> Vec<ServiceInfo> {
    let mut changes = Vec::new();
    while let Some(event) = events.try_recv() {
        if let CoreEvent::ServiceChanged { service } = event {
            changes.push(service);
        }
    }
    changes
}

/// Whether `phase` is an end: closed or failed.
pub(super) fn finished(phase: &ServicePhase) -> bool {
    matches!(phase, ServicePhase::Closed(_) | ServicePhase::Failed(_))
}

/// Waits until `engine` recorded `message` with every field in `fields` as
/// given: for a record one engine makes at a moment of its own.
pub(super) fn wait_record(trace: &Traced, engine: &str, message: &str, fields: &[(&str, &str)]) {
    wait_for(
        &format!("{engine} to record \"{message}\" {fields:?}"),
        DEFAULT_WAIT,
        || {
            trace
                .named(message)
                .iter()
                .any(|event| {
                    event["engine"] == engine
                        && fields
                            .iter()
                            .all(|(field, value)| event[*field].as_str() == Some(*value))
                })
                .then_some(())
        },
    );
}

/// How `engine` recorded each hosted service it served ending, by service
/// id: the phase it ended in, whichever path recorded it. A connection's
/// close and a serve task's own end race to record a lost parent, under
/// different messages and the same cause.
pub(super) fn hosted_ends(trace: &Traced, engine: &str) -> Vec<(u64, String)> {
    let mut ends: Vec<(u64, String)> = trace
        .events()
        .iter()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["engine"] == engine)
        .filter_map(|event| {
            let phase = match event["message"].as_str()? {
                "service phase" if event["target"] == "vnilink::service" => event["to"].as_str()?,
                "service closed" | "service ended without closing" => event["phase"].as_str()?,
                _ => return None,
            };
            (phase.starts_with("closed") || phase.starts_with("failed"))
                .then(|| (event["service"].as_u64(), phase.to_owned()))
                .and_then(|(service, phase)| Some((service?, phase)))
        })
        .collect();
    ends.sort();
    ends
}
