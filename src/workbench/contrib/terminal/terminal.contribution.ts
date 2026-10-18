import '@xterm/xterm/css/xterm.css';
import './media/terminal.css';
import { registerAction2 } from '../../../platform/actions/common/actions.js';
import { OpenTerminalAction } from './terminalActions.js';

registerAction2(OpenTerminalAction);
