// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { PermissionKind } from '../../../platform/permissions/common/permissions.js';

/** What this computer is for: controlling the others, being reached by them, or both. */
export type OnboardingRole = 'control' | 'reachable' | 'both';

/** The services a reachable computer may share, by setting, in the order the step lists them. */
export const sharingSettings = ['sharing.screen', 'sharing.files', 'sharing.clipboard', 'sharing.terminal', 'sharing.camera', 'sharing.microphone', 'sharing.systemAudio'] as const;

export type SharingSetting = typeof sharingSettings[number];

/** What the person chose; each starts from the saved setting. */
export interface OnboardingChoices {
	readonly computerName: string;
	readonly role: OnboardingRole;
	readonly sharing: Readonly<Record<SharingSetting, boolean>>;
	/** Paired computers use what is shared without the person here accepting them each time. */
	readonly unattended: boolean;
	readonly openAtLogin: boolean;
	readonly runInBackground: boolean;
}

export type OnboardingStep = 'welcome' | 'name' | 'role' | 'access' | 'permissions' | 'done';

export function reachable(role: OnboardingRole): boolean {
	return role !== 'control';
}

/** The steps for `role`: what paired computers may use is asked only of a computer they can reach. */
export function onboardingSteps(role: OnboardingRole, permissions: readonly PermissionKind[]): readonly OnboardingStep[] {
	return ['welcome', 'name', 'role', ...reachable(role) ? ['access' as const] : [], ...permissions.length ? ['permissions' as const] : [], 'done'];
}

/**
 * The operating-system permissions the choices need: the screen and, on
 * macOS, Accessibility to be seen and controlled; the camera and microphone
 * only when shared. Controlling other computers needs none here.
 */
export function neededPermissions(choices: OnboardingChoices, available: readonly PermissionKind[]): readonly PermissionKind[] {
	if (!reachable(choices.role)) {
		return [];
	}
	const needed: PermissionKind[] = [];
	if (choices.sharing['sharing.screen']) {
		needed.push('screen', 'accessibility');
	}
	if (choices.sharing['sharing.camera']) {
		needed.push('camera');
	}
	if (choices.sharing['sharing.microphone']) {
		needed.push('microphone');
	}
	return needed.filter(kind => available.includes(kind));
}

/**
 * The settings the choices write, in order. A computer only for controlling
 * accepts no connections and leaves what it would share as it was; start at
 * sign-in and running in the background are written only where `writable`
 * says this launch can change them.
 */
export function onboardingSettings(choices: OnboardingChoices, writable: (key: string) => boolean): readonly (readonly [key: string, value: unknown])[] {
	const settings: [string, unknown][] = [['application.computerName', choices.computerName.trim()]];
	settings.push(['application.acceptConnections', reachable(choices.role)]);
	if (reachable(choices.role)) {
		settings.push(...sharingSettings.map((key): [string, unknown] => [key, choices.sharing[key]]));
		settings.push(['sharing.unattended', choices.unattended]);
		settings.push(['application.openAtLogin', choices.openAtLogin], ['application.runInBackground', choices.runInBackground]);
	}
	return settings.filter(([key]) => writable(key));
}
