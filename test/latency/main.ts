/**
 * One side of the glass-to-glass latency measurement (`npm run measure:latency`).
 *
 * `VNILINK_LATENCY_ROLE=host` hosts this computer's screen from a temporary
 * engine and shows the stimulus strip across the top of the main display.
 * `VNILINK_LATENCY_ROLE=viewer` views that screen in a real workbench window
 * below the strip. The two pair through files in `VNILINK_LATENCY_DIR`, which
 * the runner in `build/lib/latency.ts` owns, and quit when it writes `done`.
 */
import { execFile } from 'node:child_process';
import { app, BrowserWindow, screen } from 'electron';
import { appendFileSync, existsSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { isLinux, isMacintosh } from '../../src/base/common/platform.js';
import { isEngineReply, record } from '../../src/platform/engine/common/engine.js';
import { TestApplication } from '../automation/application.js';
import type { WindowDriver } from '../automation/driver.js';
import { CELLS, stimulusPage } from './stimulus.js';

const role = process.env.VNILINK_LATENCY_ROLE;

const directory = process.env.VNILINK_LATENCY_DIR;

const output = process.env.VNILINK_LATENCY_OUTPUT;

/** Hold mode: how long the stimulus holds still, in ms; 0 when it never does. */
const holdMs = Number(process.env.VNILINK_LATENCY_HOLD_MS ?? '0');
/** Where the host keeps the desktop portal's grant between runs. */
const screenGrant = process.env.VNILINK_LATENCY_SCREEN_GRANT;

/** Hold mode: how long the stimulus moves between holds. */
const MOVE_MS = 2000;

/**
 * Whether the stimulus and the viewer stay above other windows: in hold
 * mode, and on Linux, where GNOME places a window shown inactive below the
 * focused one.
 */
const ON_TOP = holdMs > 0 || isLinux;

// GNOME's dock animates an application whose window opens without focus,
// as every window here does, and the animation is on the captured screen:
// no picture would hold still. On Linux the windows stay out of the dock.
if (isLinux) {
	app.on('browser-window-created', (_event, window) => {
		window.setSkipTaskbar(true);
	});
}

/** How long someone at this computer may take to answer the desktop portal's dialog; only Linux asks. */
const CONSENT_MS = isLinux ? 5 * 60000 : 0;

if ((role !== 'host' && role !== 'viewer') || !directory || !output) {
	throw new Error('VNILINK_LATENCY_ROLE (host or viewer), VNILINK_LATENCY_DIR and VNILINK_LATENCY_OUTPUT are required');
}

const rendezvous = directory;

const evidence = output;

/** Publishes `value` under `name` for the other side, whole or not at all. */
function publish(name: string, value: unknown): void {
	const path = join(rendezvous, `${name}.json`);
	writeFileSync(`${path}.partial`, JSON.stringify(value));
	renameSync(`${path}.partial`, path);
}

/** What the other side published under `name`, waiting up to `timeoutMs`. */
async function expect(name: string, timeoutMs: number): Promise<Record<string, unknown>> {
	const path = join(rendezvous, `${name}.json`);
	const deadline = Date.now() + timeoutMs;
	while (!existsSync(path)) {
		if (Date.now() > deadline) {
			throw new Error(`${name} did not arrive within ${String(timeoutMs / 1000)} s`);
		}
		await new Promise(resolve => setTimeout(resolve, 100));
	}
	const value: unknown = JSON.parse(readFileSync(path, 'utf8'));
	if (!record(value)) {
		throw new Error(`${name} is not an object`);
	}
	return value;
}

/** Seconds of CPU this application's processes have used, by kind: the engine and other utility processes by name. */
function cpuSeconds(): Record<string, number> {
	const seconds: Record<string, number> = {};
	for (const metric of app.getAppMetrics()) {
		const kind = metric.type === 'Utility' && metric.name ? metric.name : metric.type;
		seconds[kind] = (seconds[kind] ?? 0) + (metric.cpu.cumulativeCPUUsage ?? 0);
	}
	return seconds;
}

/** CPU used between two `cpuSeconds` samples, in cores, by kind. */
function cores(start: Record<string, number>, end: Record<string, number>, wallSeconds: number): Record<string, number> {
	return Object.fromEntries(Object.entries(end).map(([kind, used]) => [kind, (used - (start[kind] ?? 0)) / wallSeconds]));
}

async function engine(driver: WindowDriver, command: Record<string, unknown>): Promise<Record<string, unknown>> {
	const reply: unknown = await driver.window.webContents.executeJavaScript(`window.driver.engine.execute(${JSON.stringify(command)})`);
	if (!isEngineReply(reply) || !record(reply)) {
		throw new Error(`The engine did not answer ${String(command.type)}`);
	}
	return reply;
}

async function invitation(driver: WindowDriver): Promise<string> {
	const reply = await engine(driver, { type: 'createInvitation', ttlSeconds: 300 });
	if (reply.type !== 'invitation' || typeof reply.token !== 'string') {
		throw new Error('No invitation');
	}
	return reply.token;
}

/**
 * The strip's cell width in CSS pixels, the stimulus window's height and
 * where the viewer's window starts. The stimulus is the strip, the viewer
 * below it. In hold mode the stimulus fills the display, its strip above a
 * panel of coloured text in the top half, and the viewer covers the bottom
 * half: hidden while the stimulus holds, it leaves a screen that does not
 * change. In hold mode both cover the whole display, menu bar and Dock
 * included, above floating windows: anything else that moves there, an
 * animated desktop picture behind the Dock or a call's video, is a change.
 */
function stripGeometry(): { cell: number; height: number; viewerTop: number; area: Electron.Rectangle } {
	const area = holdMs > 0 ? screen.getPrimaryDisplay().bounds : screen.getPrimaryDisplay().workArea;
	const cell = Math.floor(area.width / (CELLS + 4));
	return holdMs > 0
		? { cell, height: area.height, viewerTop: Math.floor(area.height / 2), area }
		: { cell, height: Math.ceil(cell * 2), viewerTop: Math.ceil(cell * 2), area };
}

/** Whether this application may record the screen; asks macOS once when not. Windows asks no one. */
async function screenRecording(driver: WindowDriver): Promise<boolean> {
	const granted: unknown = await driver.window.webContents.executeJavaScript(`(async () => {
		const snapshot = await window.driver.permissions.execute({ type: 'status' });
		if (snapshot.permissions.some(permission => permission.kind === 'screen' && (permission.status === 'granted' || permission.status === 'not-required'))) {
			return true;
		}
		// macOS lists the application once it has asked; a refusal is the answer.
		await window.driver.permissions.execute({ type: 'request', kind: 'screen' }).catch(() => undefined);
		return false;
	})()`);
	return granted === true;
}

/**
 * Waits for the desktop portal's grant on Linux, which someone at this
 * computer gives in its dialog; the engine asked when it started hosting.
 */
async function portalGrant(driver: WindowDriver): Promise<void> {
	const deadline = Date.now() + CONSENT_MS;
	for (; ;) {
		const reply = await engine(driver, { type: 'nativeSharingStatus' });
		const displays = Array.isArray(reply.displays) ? reply.displays.filter(record) : [];
		if (displays.some(display => display.phase === 'Sharing')) {
			return;
		}
		const failed = displays.find(display => display.phase === 'Failed');
		if (failed) {
			throw new Error(`The screen was not shared: ${String(failed.error)}`);
		}
		if (Date.now() > deadline) {
			throw new Error(`No one allowed screen sharing in the desktop portal dialog within ${String(CONSENT_MS / 60000)} minutes.`);
		}
		await new Promise(resolve => setTimeout(resolve, 250));
	}
}

/** The pipelines the host's diagnostics hold for the screens it serves. */
async function hostedPipelines(driver: WindowDriver): Promise<Record<string, unknown>[]> {
	const reply = await engine(driver, { type: 'diagnosticSnapshot' });
	const snapshot: unknown = typeof reply.snapshot === 'string' ? JSON.parse(reply.snapshot) : undefined;
	const pipelines: unknown[] = record(snapshot) && Array.isArray(snapshot.pipelines) ? snapshot.pipelines : [];
	return pipelines.filter((pipeline): pipeline is Record<string, unknown> => record(pipeline) && pipeline.hosted === true);
}

async function host(driver: WindowDriver): Promise<void> {
	if (!isLinux && !await screenRecording(driver)) {
		throw new Error('Screen Recording is not granted to the development VniLink (.build/electron/VniLink.app). Allow it in System Settings › Privacy & Security › Screen & System Audio Recording, then run again.');
	}
	const snapshot = await engine(driver, { type: 'snapshot' });
	const endpoint = record(snapshot.snapshot) ? snapshot.snapshot.endpoint_id : undefined;
	if (typeof endpoint !== 'string') {
		throw new Error('No endpoint');
	}
	publish('host', { token: await invitation(driver), endpoint });
	const viewer = await expect('viewer', 60000);
	await engine(driver, { type: 'acceptInvitation', token: viewer.token });
	// Sharing is a setting; main writes it to the engine, as the Settings tab does.
	await driver.window.webContents.executeJavaScript(`window.driver.mainProcess.getChannel('configuration').call('updateValue', { key: 'sharing.screen', value: true })`);
	// On Linux turning it on asks the desktop portal.
	if (isLinux) {
		await portalGrant(driver);
	}
	const { cell, height, area } = stripGeometry();
	const stimulus = new BrowserWindow({
		x: area.x, y: area.y, width: area.width, height,
		frame: false, resizable: false, hasShadow: false, show: false,
		webPreferences: { backgroundThrottling: false },
	});
	if (ON_TOP) {
		stimulus.setAlwaysOnTop(true, 'screen-saver');
	}
	try {
		const hold = holdMs > 0 ? { origin: Date.now(), moveMs: MOVE_MS, holdMs } : undefined;
		if (hold) {
			writeFileSync(join(evidence, 'schedule.json'), JSON.stringify(hold));
		}
		await stimulus.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(stimulusPage(cell, hold))}`);
		stimulus.showInactive();
		publish('host-ready', {});
		// The native host's state while the viewer starts, for a start that fails.
		const trace = join(evidence, 'host-start.log');
		await driver.window.webContents.executeJavaScript(`(() => {
			window.latencyEvents = [];
			window.driver.engine.onDidReceiveEvents(batch => {
				for (const event of batch.events) {
					if (event.type !== 'presenceChanged') {
						window.latencyEvents.push(event);
					}
				}
			});
		})()`);
		for (let sample = 0; sample < 40 && !existsSync(join(rendezvous, 'viewer-streaming.json')) && !existsSync(join(rendezvous, 'read.json')); sample++) {
			const events: unknown = await driver.window.webContents.executeJavaScript('window.latencyEvents.splice(0)');
			appendFileSync(trace, `${String(Date.now())} ${JSON.stringify({ status: await engine(driver, { type: 'nativeSharingStatus' }), events, pipelines: await hostedPipelines(driver) })}\n`);
			await new Promise(resolve => setTimeout(resolve, 500));
		}
		// The reader runs as this process's child, so macOS counts it under
		// this application's Screen Recording permission.
		const read = await expect('read', 5 * 60000);
		const [cpuStart, startedAt] = [cpuSeconds(), Date.now()];
		const outcome = await new Promise<{ code: number; output: string }>(resolve => {
			execFile(String(read.reader), [String(read.seconds), String(read.samples), String(read.grant)], (error, stdout, stderr) => {
				resolve({ code: error ? 1 : 0, output: `${stdout}${stderr}` });
			});
		});
		publish('host-cpu', cores(cpuStart, cpuSeconds(), (Date.now() - startedAt) / 1000));
		// The viewer's pipeline as the host's diagnostics hold it: its encoder,
		// keyframes, reopens and drops over the run.
		publish('host-pipeline', (await hostedPipelines(driver)).at(0) ?? {});
		publish('read-done', outcome);
		await expect('done', 5 * 60000);
	} finally {
		stimulus.destroy();
	}
}

/** The still pictures the viewer's diagnostics say it showed. */
async function stillsShown(driver: WindowDriver): Promise<number | undefined> {
	const reply = await engine(driver, { type: 'diagnosticSnapshot' });
	const snapshot: unknown = typeof reply.snapshot === 'string' ? JSON.parse(reply.snapshot) : undefined;
	const pipelines: unknown[] = record(snapshot) && Array.isArray(snapshot.pipelines) ? snapshot.pipelines : [];
	const viewing = pipelines.find((pipeline): pipeline is Record<string, unknown> => record(pipeline) && pipeline.hosted === false);
	return typeof viewing?.stills_shown === 'number' ? viewing.stills_shown : undefined;
}

async function viewer(driver: WindowDriver): Promise<void> {
	if (holdMs > 0) {
		// The Smooth preset: stills of a 4K screen fit only a fast stream's budget.
		await driver.window.webContents.executeJavaScript(`window.driver.mainProcess.getChannel('configuration').call('updateValue', { key: 'desktop.quality', value: { max_bitrate_kbps: 12000, max_frame_rate: 60, max_width: 1920, max_height: 1080 } })`);
	}
	publish('viewer', { token: await invitation(driver) });
	const host = await expect('host', 60000);
	await engine(driver, { type: 'acceptInvitation', token: host.token });
	await expect('host-ready', 60000 + CONSENT_MS);
	// As the Devices page does before it shows a computer's screen.
	await engine(driver, { type: 'beginConnectServices', endpointId: host.endpoint });
	// That only starts the connection; the view shows "not connected" if
	// it opens before the connection is up, and "can't share its screen"
	// if it opens before the host's report says it can: the link may have
	// come up before a Linux host's portal grant, and the host reports
	// again only on its next check.
	const connected = Date.now() + 30000;
	for (; ;) {
		const snapshot = await engine(driver, { type: 'snapshot' });
		const devices = record(snapshot.snapshot) && Array.isArray(snapshot.snapshot.devices) ? snapshot.snapshot.devices : [];
		const availability = await engine(driver, { type: 'peerAvailability', endpointId: host.endpoint }).catch(() => undefined);
		const report: unknown[] = Array.isArray(availability?.report) ? availability.report : [];
		if (devices.some(device => record(device) && device.endpoint_id === host.endpoint && device.presence === 'connected')
			&& report.some(entry => record(entry) && entry.capability === 'desktopView' && entry.status === 'available')) {
			break;
		}
		if (Date.now() > connected) {
			throw new Error('The viewer did not connect to the host within 30 s');
		}
		await new Promise(resolve => setTimeout(resolve, 200));
	}
	const { viewerTop, area } = stripGeometry();
	driver.window.setBounds({ x: area.x, y: area.y + viewerTop, width: area.width, height: area.height - viewerTop });
	if (ON_TOP) {
		driver.window.setAlwaysOnTop(true, 'screen-saver', 1);
	}
	driver.window.showInactive();
	// The device list learns of the pairing from an engine event; the view
	// opens once it has.
	const deadline = Date.now() + 30000;
	const trace = join(evidence, 'viewer-start.log');
	// The window's own messages, for a view that does not start.
	driver.window.webContents.on('console-message', ({ level, message }) => {
		appendFileSync(trace, `${String(Date.now())} console ${level} ${message}\n`);
	});
	// What the engine reports while the view starts, for a start that fails.
	await driver.window.webContents.executeJavaScript(`(() => {
		window.latencyEvents = [];
		window.driver.engine.onDidReceiveEvents(batch => {
			for (const event of batch.events) {
				if (event.type !== 'presenceChanged') {
					window.latencyEvents.push(event);
				}
			}
		});
	})()`);
	for (let attempt = 0; ; attempt++) {
		const state: unknown = await driver.window.webContents.executeJavaScript(`(async () => {
			// Opened once: selecting the tab again is not needed and is not watched.
			if (!document.querySelector('canvas.desktop-screen')) {
				await window.driver.commands.executeCommand('vnilink.desktop.view', ${JSON.stringify(host.endpoint)});
			}
			await new Promise(resolve => setTimeout(resolve, 250));
			return {
				role: document.querySelector('.desktop-status-role')?.textContent ?? '',
				// What the view says when it cannot show the screen.
				problem: [...document.querySelectorAll('.desktop-overlay:not([hidden]) :is(.desktop-overlay-title, .desktop-overlay-body)')].map(element => element.textContent).join(' '),
				// The stats line fills once pictures are drawn and stays empty otherwise.
				stats: document.querySelector('.desktop-status-stats')?.textContent ?? '',
				events: window.latencyEvents.splice(0),
			};
		})()`);
		appendFileSync(trace, `${String(Date.now())} ${JSON.stringify(state)}\n`);
		if (attempt % 10 === 0) {
			await driver.screenshot(`viewer-start-${String(attempt)}`);
		}
		if (record(state) && typeof state.stats === 'string' && state.stats.length > 0) {
			break;
		}
		if (Date.now() > deadline) {
			await driver.screenshot('viewer-no-picture');
			throw new Error('The viewer did not show a picture within 30 s');
		}
	}
	if (holdMs > 0) {
		// The status bar changes every second, and it is on the captured
		// screen: hidden, the screen can hold still.
		await driver.window.webContents.executeJavaScript(`(() => {
			const style = document.createElement('style');
			style.textContent = '.desktop-status { visibility: hidden !important; }';
			document.head.append(style);
		})()`);
	}
	await driver.screenshot('viewer-streaming');
	publish('viewer-streaming', {});
	// The view's own stage times, once a second, as it shows them: the
	// runner sets them against the total read from the screen.
	const stages = join(evidence, 'stages.jsonl');
	const stills = join(evidence, 'stills.jsonl');
	// CPU over the reader's run, as the host measures its own.
	let cpuStart: [Record<string, number>, number] | undefined;
	let lastStages = 0;
	const schedule = holdMs > 0 ? JSON.parse(readFileSync(join(evidence, 'schedule.json'), 'utf8')) as { origin: number; moveMs: number; holdMs: number } : undefined;
	while (!existsSync(join(rendezvous, 'done.json'))) {
		// Hold mode: when each still showed, to a tenth of a second; and
		// while the stimulus holds, this window, which redraws every
		// picture it receives, is hidden: on the captured screen each of
		// its pictures is a change, and the screen would never hold still.
		// ScreenCaptureKit captures a content-protected window regardless.
		// Hidden, it still receives and decodes its pictures.
		if (schedule) {
			const holding = (Date.now() - schedule.origin) % (schedule.moveMs + schedule.holdMs) >= schedule.moveMs;
			if (holding && driver.window.isVisible()) {
				driver.window.hide();
			} else if (!holding && !driver.window.isVisible()) {
				driver.window.showInactive();
			}
			const shown = await stillsShown(driver);
			if (shown !== undefined) {
				appendFileSync(stills, `${JSON.stringify({ at: Date.now(), shown })}\n`);
			}
			if (Date.now() - lastStages < 1000) {
				await new Promise(resolve => setTimeout(resolve, 100));
				continue;
			}
		}
		lastStages = Date.now();
		if (!cpuStart && existsSync(join(rendezvous, 'read.json'))) {
			cpuStart = [cpuSeconds(), Date.now()];
		}
		if (cpuStart && existsSync(join(rendezvous, 'read-done.json')) && !existsSync(join(rendezvous, 'viewer-cpu.json'))) {
			publish('viewer-cpu', cores(cpuStart[0], cpuSeconds(), (Date.now() - cpuStart[1]) / 1000));
		}
		const text: unknown = await driver.window.webContents.executeJavaScript(`document.querySelector('.desktop-status-stats')?.title ?? ''`);
		if (typeof text === 'string' && text.length > 0) {
			appendFileSync(stages, `${JSON.stringify({ at: Date.now(), text })}\n`);
		}
		await new Promise(resolve => setTimeout(resolve, holdMs > 0 ? 100 : 1000));
	}
}

// The host prepares native video to capture and encode, and only it turns
// screen sharing on. The viewer prepares it on Windows and Linux, where
// nothing else gives its engine a decoder and it would refuse the screen. On macOS
// the window's own decoders admit it, and a viewer engine with native video
// on the same Mac as the host made the host's capture reads about 19 ms
// slower (glass to glass 133 against 116 ms p50): a cost of measuring on
// one computer, not of either side.
const application = new TestApplication({
	showWindow: false,
	nativeVideo: role === 'host' || !isMacintosh,
	screenGrant: role === 'host' ? screenGrant : undefined,
	timeoutMs: 30 * 60000,
});

void application.run((_window, driver) => role === 'host' ? host(driver) : viewer(driver));
