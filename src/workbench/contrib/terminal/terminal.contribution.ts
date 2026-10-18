import '@xterm/xterm/css/xterm.css';
import './media/terminal.css';
import { registerAction2 } from '../../../platform/actions/common/actions.js';
import { OpenTerminalAction, RestartTerminalAction } from './terminalActions.js';

registerAction2(OpenTerminalAction);

registerAction2(RestartTerminalAction);
