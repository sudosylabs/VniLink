import { renderWorkbench } from './workbench/browser/workbench.js';

async function initialize(): Promise<void> {
	const info = await window.vnilink.getAppInfo();
	await renderWorkbench(info, window.vnilink);
}

void initialize().catch((error: unknown) => {
	console.error(error);
	const status = document.querySelector('[role="status"]');
	if (status) {
		status.textContent = 'The desktop could not start. Close the window and try again.';
	}
});
