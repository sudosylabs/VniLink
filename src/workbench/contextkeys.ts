import * as nls from '../nls.js';
import { RawContextKey } from '../platform/contextkey/common/contextkey.js';

/** The id of the tab whose view is showing. */
export const ActiveTabContext = new RawContextKey<string>('activeTab', '', nls.localize('activeTab', 'The identifier of the active tab'));

/** Whether saved relay settings differ from the running engine's, so a restart applies them. */
export const RestartRequiredContext = new RawContextKey<boolean>('vnilink.restartRequired', false, nls.localize('restartRequired', 'Whether VniLink must restart to apply saved settings'));

/** Whether the connection engine accepts commands. */
export const EngineAvailableContext = new RawContextKey<boolean>('vnilink.engineAvailable', false, nls.localize('engineAvailable', 'Whether the connection engine is running'));
