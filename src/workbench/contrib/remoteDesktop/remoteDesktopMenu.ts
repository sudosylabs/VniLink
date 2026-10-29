import { MenuId } from '../../../platform/actions/common/actions.js';

/**
 * Commands on the computer whose screen a remote desktop shows, in its
 * session bar's More menu after the tab's own. Each runs with that
 * computer's device id.
 */
export const RemoteDesktopMenu = new MenuId('RemoteDesktop');
