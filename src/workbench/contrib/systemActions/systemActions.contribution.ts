import { registerAction2 } from '../../../platform/actions/common/actions.js';
import { CancelSystemActionAction, LockComputerAction, RestartOrShutDownAction } from './systemActionsActions.js';

registerAction2(LockComputerAction);

registerAction2(RestartOrShutDownAction);

registerAction2(CancelSystemActionAction);
