//! Lock, restart and shut down: asking a paired computer, and the notice
//! this computer's user answers when one asks it.
use super::{core_error, parse_id, session_target, Command, Reply, State};
use napi::{Error, Result};
use serde::Serialize;
use std::sync::Arc;
use vnilink_app::{
    FfiOperationState, FfiScheduledSystemAction, FfiSystemAction, FfiSystemOperation,
    SystemServiceHandle,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TransferSystemOperation {
    id: String,
    action: &'static str,
    state: &'static str,
    detail: String,
    notice_remaining_ms: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TransferScheduledSystemAction {
    id: String,
    action: &'static str,
    endpoint_id: String,
    remaining_ms: u32,
}

pub(super) fn action_name(action: FfiSystemAction) -> &'static str {
    match action {
        FfiSystemAction::Lock => "lock",
        FfiSystemAction::Restart => "restart",
        FfiSystemAction::Shutdown => "shutdown",
    }
}

pub(super) fn state_name(state: FfiOperationState) -> &'static str {
    match state {
        FfiOperationState::Admitted => "admitted",
        FfiOperationState::Refused => "refused",
        FfiOperationState::Executed => "executed",
        FfiOperationState::Failed => "failed",
        FfiOperationState::Scheduled => "scheduled",
        FfiOperationState::Cancelled => "cancelled",
    }
}

fn transfer(operation: FfiSystemOperation) -> Reply {
    Reply::SystemOperation {
        operation: TransferSystemOperation {
            id: operation.id.to_string(),
            action: action_name(operation.action),
            state: state_name(operation.state),
            detail: operation.detail,
            notice_remaining_ms: operation.notice_remaining_ms,
        },
    }
}

fn scheduled(action: FfiScheduledSystemAction) -> TransferScheduledSystemAction {
    TransferScheduledSystemAction {
        id: action.operation_id.to_string(),
        action: action_name(action.action),
        endpoint_id: action.endpoint_id,
        remaining_ms: action.remaining_ms,
    }
}

/// The System service this computer holds on a connection, opened on first
/// use. It stays open while operations on it may still be asked about; a
/// countdown also ends if it closes, as the asker leaving.
fn service(state: &mut State, session: u64, generation: u64) -> Result<Arc<SystemServiceHandle>> {
    state.systems.retain(|_, handle| !handle.is_cancelled());
    if let Some(handle) = state.systems.get(&(session, generation)) {
        return Ok(handle.clone());
    }
    let owner = state
        .core
        .attach_controller_service(session, generation, vnilink_app::FfiServiceKind::System)
        .map_err(core_error)?;
    let handle = match state.core.open_system_service(owner.id, owner.generation) {
        Ok(handle) => handle,
        Err(error) => {
            if state
                .core
                .close_controller_service(owner.id, owner.generation)
                .is_ok()
            {
                let _ = state.core.forget_controller_service(owner.id);
            }
            return Err(core_error(error));
        }
    };
    state.systems.insert((session, generation), handle.clone());
    Ok(handle)
}

pub(super) fn dispatch(state: &mut State, command: &Command) -> Result<Reply> {
    match command {
        Command::SystemAction {
            session_id,
            generation,
            action,
        } => {
            let action = match action.as_str() {
                "lock" => FfiSystemAction::Lock,
                "restart" => FfiSystemAction::Restart,
                "shutdown" => FfiSystemAction::Shutdown,
                _ => return Err(Error::from_reason("Invalid system action")),
            };
            let (session, generation) = session_target(session_id, generation)?;
            let handle = service(state, session, generation)?;
            Ok(transfer(handle.execute(action).map_err(core_error)?))
        }
        Command::SystemOperation {
            session_id,
            generation,
            id,
        } => {
            let (session, generation) = session_target(session_id, generation)?;
            let id = parse_id(id, "Invalid system operation ID")?;
            let handle = service(state, session, generation)?;
            Ok(transfer(handle.operation(id).map_err(core_error)?))
        }
        Command::CancelSystemAction {
            session_id,
            generation,
            id,
        } => {
            let (session, generation) = session_target(session_id, generation)?;
            let id = parse_id(id, "Invalid system operation ID")?;
            let handle = service(state, session, generation)?;
            Ok(transfer(handle.cancel(id).map_err(core_error)?))
        }
        Command::ScheduledSystemActions => Ok(Reply::ScheduledSystemActions {
            actions: state
                .core
                .scheduled_system_actions()
                .map_err(core_error)?
                .into_iter()
                .map(scheduled)
                .collect(),
        }),
        Command::CancelHostedSystemAction { id } => {
            let id = parse_id(id, "Invalid system operation ID")?;
            Ok(transfer(
                state.core.cancel_system_action(id).map_err(core_error)?,
            ))
        }
        Command::RunHostedSystemActionNow { id } => {
            let id = parse_id(id, "Invalid system operation ID")?;
            state.core.run_system_action_now(id).map_err(core_error)?;
            Ok(Reply::Done)
        }
        _ => Err(Error::from_reason("Invalid system command")),
    }
}
