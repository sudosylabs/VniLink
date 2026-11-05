// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Owns readiness and renderer automation for one real Electron window. */
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { desktopCapturer, screen, type BrowserWindow } from 'electron';
import { Event } from '../../../src/base/common/event.js';
import { Disposable } from '../../../src/base/common/lifecycle.js';
import type { IWindowDriver } from '../../../src/workbench/services/driver/windowDriver.js';
import type { BrowserScenarios } from '../../smoke/fixtures/index.js';

/** A point in the window, in its CSS pixels from the top-left of the page. */
export interface WindowPoint {
	readonly x: number;
	readonly y: number;
}

/** The colour of each named point in a native capture, as `#rrggbb`. */
export type NativeColors = Record<string, string>;

/** Whether every pixel of `image` is opaque. */
function isOpaque(image: Electron.NativeImage): boolean {
	const bitmap = image.toBitmap();
	for (let index = 3; index < bitmap.length; index += 4) {
		if (bitmap[index] !== 255) {
			return false;
		}
	}
	return true;
}

/** `F`, answering through a promise, as a call across a process does. */
type RemoteMethod<F> = F extends (...args: infer A) => infer R ? (...args: A) => Promise<Awaited<R>> : never;

/** The methods of `T`, each answering through a promise. */
export type Remote<T> = {
	readonly [K in keyof T as T[K] extends (...args: never[]) => unknown ? K : never]: RemoteMethod<T[K]>;
};

/**
 * Calls `method` of the object at `path` in `window`'s page with `args`,
 * which travel as JSON: the one script the automation sends, and it holds
 * no logic of its own.
 */
function remote<T>(window: BrowserWindow, path: string): Remote<T> {
	const target: object = {};
	return new Proxy(target, {
		get(_target, method) {
			if (typeof method !== 'string') {
				throw new Error('A remote method is named by a string');
			}
			// Not a promise itself: `await` asks for `then`, which no remote object answers.
			if (method === 'then') {
				return undefined;
			}
			return (...args: unknown[]) => window.webContents.executeJavaScript(`${path}[${JSON.stringify(method)}](...${JSON.stringify(args)})`);
		},
	}) as Remote<T>;
}

export class WindowDriver extends Disposable {
	private fixturesLoaded = false;

	/** The workbench's driver in the page (`src/workbench/services/driver`). */
	public readonly renderer: Remote<IWindowDriver>;

	constructor(public readonly window: BrowserWindow, private readonly output: string) {
		super();
		this.renderer = remote<IWindowDriver>(window, 'window.driver');
		this._register(Event.fromNodeEventEmitter<unknown>(window.webContents, 'did-start-navigation')(() => {
			this.fixturesLoaded = false;
		}));
	}

	/**
	 * Waits until the workbench is restored. The page registers its driver as
	 * the workbench starts, so a call before then is asked again, for ten
	 * seconds at most.
	 */
	public async ready(): Promise<void> {
		const deadline = Date.now() + 10000;
		for (; ;) {
			try {
				await this.renderer.whenWorkbenchRestored();
				return;
			} catch (error) {
				if (Date.now() > deadline) {
					throw error;
				}
				await new Promise(resolve => setTimeout(resolve, 100));
			}
		}
	}

	/** The smoke fixtures in the page (`test/smoke/fixtures`), loaded on first use. */
	public async fixtures(): Promise<Remote<BrowserScenarios>> {
		await this.ready();
		if (!this.fixturesLoaded) {
			await this.window.webContents.executeJavaScript(readFileSync(join(__dirname, 'browser-tests.js'), 'utf8'));
			this.fixturesLoaded = true;
		}
		return remote<BrowserScenarios>(this.window, 'browserTests.browserScenarios');
	}

	public async screenshot(name: string): Promise<void> {
		const image = await this.window.webContents.capturePage();
		writeFileSync(join(this.output, `${name}.png`), image.toPNG());
	}

	/**
	 * Captures the window as the system composes it, native material and all,
	 * which a page capture cannot show: saves `<name>-native.png` and adds the
	 * colour at each of `points` to the run's `native-colors.json`, so runs are
	 * compared by number. `undefined` when the system gives no picture of the
	 * window, as macOS does without Screen Recording permission.
	 */
	public async captureNative(name: string, points: Readonly<Record<string, WindowPoint>>): Promise<NativeColors | undefined> {
		const content = this.window.getContentBounds();
		const display = screen.getDisplayMatching(this.window.getBounds());
		const pixels = (rectangle: Electron.Rectangle): Electron.Size => ({
			width: Math.round(rectangle.width * display.scaleFactor),
			height: Math.round(rectangle.height * display.scaleFactor),
		});
		let image = await this.composed('window', this.window.getMediaSourceId(), pixels(this.window.getBounds()));
		let origin = this.window.getBounds();
		if (image && !isOpaque(image)) {
			// Where the window's own picture is see-through, the system composes a backdrop of its
			// own behind it, as Windows does Mica, which only a picture of the screen shows.
			image = await this.onTop(() => this.composed('screen', String(display.id), pixels(display.bounds)));
			origin = display.bounds;
		}
		if (!image) {
			return undefined;
		}
		const ratio = image.getSize().width / origin.width;
		image = image.crop({
			x: Math.round((content.x - origin.x) * ratio),
			y: Math.round((content.y - origin.y) * ratio),
			width: Math.round(content.width * ratio),
			height: Math.round(content.height * ratio),
		});
		writeFileSync(join(this.output, `${name}-native.png`), image.toPNG());
		const size = image.getSize();
		const zoom = this.window.webContents.getZoomFactor();
		const bitmap = image.toBitmap();
		const colors: NativeColors = {};
		for (const [label, point] of Object.entries(points)) {
			const x = Math.min(size.width - 1, Math.max(0, Math.round(point.x * zoom * ratio)));
			const y = Math.min(size.height - 1, Math.max(0, Math.round(point.y * zoom * ratio)));
			// Chromium's bitmaps are BGRA on every platform the app ships on.
			const offset = (y * size.width + x) * 4;
			colors[label] = `#${[bitmap[offset + 2], bitmap[offset + 1], bitmap[offset]].map(value => value.toString(16).padStart(2, '0')).join('')}`;
		}
		const file = join(this.output, 'native-colors.json');
		const recorded = existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) as Record<string, NativeColors> : {};
		writeFileSync(file, `${JSON.stringify({ ...recorded, [name]: colors }, undefined, '\t')}\n`);
		return colors;
	}

	/** Runs `capture` with the window above every other, as it was before afterwards. */
	private async onTop<T>(capture: () => Promise<T>): Promise<T> {
		const above = this.window.isAlwaysOnTop();
		this.window.setAlwaysOnTop(true);
		this.window.moveTop();
		await this.renderer.whenRendered();
		try {
			return await capture();
		} finally {
			this.window.setAlwaysOnTop(above);
		}
	}

	/**
	 * The picture of the window or screen `id` from the system. Right after
	 * the window changes, such as losing focus, the system can answer with a
	 * black frame before it has composed one, so a black picture is asked for
	 * again, for a second at most; `undefined` if no picture comes.
	 */
	private async composed(type: 'window' | 'screen', id: string, thumbnailSize: Electron.Size): Promise<Electron.NativeImage | undefined> {
		const deadline = Date.now() + 1000;
		for (; ;) {
			const sources = await desktopCapturer.getSources({ types: [type], thumbnailSize, fetchWindowIcons: false });
			const image = sources.find(source => type === 'window' ? source.id === id : source.display_id === id)?.thumbnail;
			if (image && !image.isEmpty() && image.toBitmap().some((value, index) => index % 4 !== 3 && value > 1)) {
				return image;
			}
			if (Date.now() > deadline) {
				return undefined;
			}
			await new Promise(resolve => setTimeout(resolve, 100));
		}
	}
}
