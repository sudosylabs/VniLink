import { button } from '../../../../base/browser/ui/button/button';
import type { AppInfo, DesktopApi, WindowAction } from '../../../../platform/ipc/common/desktopApi';
export function createTitlebar(info: AppInfo, api: DesktopApi, onError: (error: unknown) => void): HTMLElement {
	const titlebar = document.createElement('header');
	titlebar.className = 'titlebar';
	const tabs = document.createElement('nav');
	tabs.className = 'tabs';
	tabs.setAttribute('aria-label', 'Session tabs');
	const home = button('Home', 'tab');
	home.setAttribute('aria-current', 'page');
	tabs.append(home);
	titlebar.append(tabs);
	if (info.platform !== 'darwin') {
		const controls = document.createElement('div');
		controls.className = 'window-controls';
		const actions: [WindowAction, string, string][] = [['minimize', '−', 'Minimize'], ['maximize', '□', 'Maximize'], ['close', '×', 'Close']];
		for (const [action, symbol, label] of actions) {
			const control = button(symbol, '', () => { void api.windowAction(action).catch(onError); });
			control.setAttribute('aria-label', label);
			controls.append(control);
		}
		titlebar.append(controls);
	}
	return titlebar;
}
