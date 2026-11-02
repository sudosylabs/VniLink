// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { $ } from '../../../../../base/browser/dom.js';
import { timeout } from '../../../../../base/common/async.js';
import { Emitter } from '../../../../../base/common/event.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import type { NativeDevice, TerminalShells, VideoCodecCaps } from '../../../../../platform/engine/common/engine.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { Registry } from '../../../../../platform/registry/common/platform.js';
import { IApplicationService } from '../../../../services/application/applicationService.js';
import { IConnectionService, type RunningRelays } from '../../../../services/connection/connectionService.js';
import { IWorkbenchConfigurationService } from '../../../../services/configuration/configurationService.js';
import { TestConfigurationService } from '../../../../services/configuration/test/common/testConfigurationService.js';
import { INativeRuntimeService } from '../../../../services/nativeRuntime/nativeRuntimeService.js';
import { IPermissionService } from '../../../../services/permissions/permissionService.js';
import { TestPermissionService } from '../../../../services/permissions/test/common/testPermissionService.js';
import { IRemoteDesktopService } from '../../../../services/remoteDesktop/remoteDesktopService.js';
import { ITerminalHostService } from '../../../../services/terminal/terminalHostService.js';
import { Extensions, type IHomePage, type IHomePagesRegistry } from '../../../home/homePages.js';
import '../../settings.contribution.js';

/** Lets every promise callback a change started finish before the test reads the page. */
export function settle(): Promise<void> {
	return timeout(0);
}

/** Types `value` into `input`, as the user does, announcing it with `event`. */
export function edit(input: HTMLInputElement | HTMLSelectElement, value: string, event: 'input' | 'change' = input instanceof HTMLSelectElement ? 'change' : 'input'): void {
	input.value = value;
	input.dispatchEvent(new Event(event));
}

/** Reads a control again; assertions after an event must not reuse what TypeScript narrowed before it. */
export function current(control: HTMLInputElement | HTMLSelectElement): { readonly value: string; readonly disabled: boolean } {
	return { value: control.value, disabled: control.disabled };
}

/** Flips a switch or check box, as a click does. */
export function toggle(input: HTMLInputElement): void {
	input.checked = !input.checked;
	input.dispatchEvent(new Event('change'));
}

/** Clicks `target` as the user's pointer does. */
export function click(target: Element): void {
	target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

/** The element `selector` finds in `root`, failing the test when there is none. */
// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters
export function element<T extends Element = HTMLElement>(root: ParentNode, selector: string): T {
	const found = root.querySelector(selector);
	assert.ok(found, `${selector} is missing`);
	return found as T;
}

export const devices: readonly NativeDevice[] = [{ id: 'camera-one', name: 'Camera <one>', kind: 'camera' }, { id: 'mic-one', name: 'Microphone', kind: 'microphone' }];

export const desktopCodecs: VideoCodecCaps = {
	encode_baseline: false, encode_high: false, decode_baseline: true, decode_high: false, max_width: 3840, max_height: 2160, hardware: true, device: 'test', chroma444: false, additional: [
		{ codec: 'Vp9', encode: true, decode: true, hardware: false },
		{ codec: 'Av1', encode: false, decode: true, hardware: false },
		{ codec: 'H265', encode: true, decode: false, hardware: true },
	],
};

/** How long a typed value waits before it is applied, with room to spare over the page's pause. */
const editDelay = 650;

/**
 * Home's settings pages, built over test services as the workbench builds
 * them and shown in a Home content area. Settings live in `configuration`,
 * operating-system access in `permissions`; the other services answer from
 * the fields below.
 */
export class SettingsFixture extends Disposable {
	public readonly configuration = this._register(new TestConfigurationService());
	public readonly permissions = this._register(new TestPermissionService());
	public chosenFolder: string | null = '/Recordings';
	public openedDocumentation = 0;
	public running: RunningRelays = { publicRelay: true, urls: [] };
	/** What this computer's engine reports it can offer; changing it models installing a shell. */
	public shells: TerminalShells = { shells: [{ id: '/bin/zsh', label: 'zsh' }, { id: '/bin/bash', label: 'bash' }], automatic: '/bin/zsh', current: '/bin/zsh' };
	private readonly instantiationService = this._register(new TestInstantiationService(undefined, true));
	private readonly host = $('div.home-content');

	constructor() {
		super();
		this.instantiationService.stub(IWorkbenchConfigurationService, this.configuration);
		this.instantiationService.stub(IPermissionService, this.permissions);
		this.instantiationService.stub(INativeRuntimeService, {
			available: true,
			recordingDirectory: action => Promise.resolve(action === 'read' ? '/Pictures/VniLink' : this.chosenFolder),
			devices: kind => Promise.resolve(devices.filter(device => kind === 'camera' ? device.kind === 'camera' : device.kind !== 'camera')),
			powerStatus: () => Promise.resolve([]),
		});
		this.instantiationService.stub(IRemoteDesktopService, { available: true, codecs: () => Promise.resolve(desktopCodecs) });
		this.instantiationService.stub(IApplicationService, {
			restartRequired: false,
			onDidChangeRestartRequired: this._register(new Emitter<void>()).event,
			restart: () => Promise.resolve(),
			openRelayDocumentation: () => {
				this.openedDocumentation++;
				return Promise.resolve();
			},
		});
		this.instantiationService.stub(IConnectionService, { getRunningRelays: () => Promise.resolve(this.running) });
		this.instantiationService.stub(ITerminalHostService, { shells: () => Promise.resolve(this.shells) });
		document.body.append(this.host);
	}

	/**
	 * Moves time past the pause a typed value waits for before it is applied.
	 * Only inside `runWithFakedTimers`, where the wait is virtual.
	 */
	public settleEdit(): Promise<void> {
		return timeout(editDelay);
	}

	/** Creates the registered settings page `id`, shows it, and disposes it with the fixture. */
	public open(id: string): IHomePage {
		const descriptor = Registry.as<IHomePagesRegistry>(Extensions.HomePages).getPage(id);
		assert.ok(descriptor, `Settings page ${id} is not registered`);
		const page = this._register(this.instantiationService.createInstance<IHomePage>(descriptor.ctorDescriptor));
		this.host.append(page.element);
		return page;
	}

	public override dispose(): void {
		super.dispose();
		this.host.remove();
	}
}
