import { screen } from 'electron';
import { Disposable } from '../../../base/common/lifecycle.js';
import { isMacintosh, isWindows } from '../../../base/common/platform.js';
import { isNativeScreenSource, type NativeScreenSource } from '../../engine/common/engine.js';
import { IEngineMainService } from '../../engine/electron-main/engineMainService.js';
import type { EngineMainService } from '../../engine/electron-main/engineMainService.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';
import { IPermissionsMainService } from '../../permissions/electron-main/permissionsMainService.js';
import type { PermissionsMainService } from '../../permissions/electron-main/permissionsMainService.js';

export const IScreenHostingMainService = createDecorator<ScreenHostingMainService>('screenHostingMainService');

/** The display a native host should serve, or none. */
export type ScreenHostingTarget = NativeScreenSource | undefined;

/** The OS facts a screen host depends on, replaceable in tests. */
export interface ScreenHostingEnvironment {
	readonly platform: 'darwin' | 'win32' | 'other';
	primaryDisplay(): ScreenHostingTarget;
	onDidChangeDisplays(listener: () => void): () => void;
}

const electronEnvironment: ScreenHostingEnvironment = {
	platform: isMacintosh ? 'darwin' : isWindows ? 'win32' : 'other',
	primaryDisplay: () => {
		const display = screen.getPrimaryDisplay();
		const source = {
			displayId: display.id,
			width: display.bounds.width,
			height: display.bounds.height,
			pixelWidth: Math.round(display.bounds.width * display.scaleFactor),
			pixelHeight: Math.round(display.bounds.height * display.scaleFactor),
		};
		return isNativeScreenSource(source) ? source : undefined;
	},
	onDidChangeDisplays: listener => {
		screen.on('display-added', listener);
		screen.on('display-removed', listener);
		screen.on('display-metrics-changed', listener);
		return () => {
			screen.off('display-added', listener);
			screen.off('display-removed', listener);
			screen.off('display-metrics-changed', listener);
		};
	},
};

/**
 * Keeps the native screen host running for the primary display, so paired
 * computers can view and control this one. The host is idle until a viewer
 * connects; capture runs only while someone watches. macOS needs Screen
 * Recording first; Windows asks for no consent. Sharing policy and peer
 * grants in the engine still decide who may view.
 */
export class ScreenHostingMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private started = false;
	private hosted: ScreenHostingTarget;
	private applying: Promise<void> | undefined;
	private applyAgain = false;

	constructor(
		@IEngineMainService private readonly engine: EngineMainService,
		@IPermissionsMainService private readonly permissions: PermissionsMainService,
		@ILogService private readonly logService: ILogService,
		private readonly environment: ScreenHostingEnvironment = electronEnvironment,
	) {
		super();
	}

	/** Hosts the primary display now and follows displays, consent and engine restarts. */
	public start(): Promise<void> {
		if (!this.started) {
			this.started = true;
			const stopListening = this.environment.onDidChangeDisplays(() => {
				void this.apply();
			});
			this._register({ dispose: stopListening });
			this._register(this.permissions.onDidChangeConsent(() => {
				void this.apply();
			}));
			this._register(this.engine.onDidChangeAvailability(available => {
				// A restarted engine hosts nothing yet.
				this.hosted = undefined;
				if (available) {
					void this.apply();
				}
			}));
		}
		return this.apply();
	}

	private wanted(): ScreenHostingTarget {
		const { platform } = this.environment;
		if (platform === 'other' || (platform === 'darwin' && !this.permissions.isGranted('screen'))) {
			return undefined;
		}
		return this.environment.primaryDisplay();
	}

	/** One change at a time; changes during an apply schedule one more. */
	private apply(): Promise<void> {
		if (this.applying) {
			this.applyAgain = true;
			return this.applying;
		}
		this.applying = (async () => {
			do {
				this.applyAgain = false;
				await this.applyOnce();
			} while (this.shouldApplyAgain());
		})().finally(() => {
			this.applying = undefined;
		});
		return this.applying;
	}

	private shouldApplyAgain(): boolean {
		return this.applyAgain && !this._store.isDisposed;
	}

	private async applyOnce(): Promise<void> {
		if (!this.engine.available) {
			return;
		}
		const wanted = this.wanted();
		if (JSON.stringify(wanted) === JSON.stringify(this.hosted)) {
			return;
		}
		try {
			if (this.hosted) {
				this.hosted = undefined;
				await this.engine.execute({ type: 'stopNativeScreenSharing' });
			}
			if (wanted) {
				await this.engine.execute({ type: 'startNativeScreenSharing', ...wanted });
				this.hosted = wanted;
			}
		} catch (error) {
			// Peers see why through the engine's availability report.
			this.logService.warn('This screen could not be shared with paired computers.', error);
		}
	}
}
