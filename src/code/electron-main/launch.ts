// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { product } from '../../platform/product/common/product.js';
import { selfCheckOutput } from './selfCheck.js';

/** What a launch knows before it starts anything. */
export interface LaunchContext {
	readonly argv: readonly string[];
	/** Whether this is an installed build rather than the source checkout. */
	readonly built: boolean;
	readonly windows: boolean;
	/** Whether Chromium was given the command-line switch `name`. */
	hasSwitch(name: string): boolean;
	/** `VNILINK_TEST_PROFILE`, the profile a smoke launch must run in. */
	readonly testProfile: string | undefined;
}

/** What `VniLinkMain` does once the process has its identity. */
type LaunchAction =
	/** A built application exits rather than accept `argument`. */
	| { readonly kind: 'refuse'; readonly argument: string }
	| { readonly kind: 'invalid'; readonly reason: string }
	/** `--self-check`, writing its report to `output`. */
	| { readonly kind: 'selfCheck'; readonly output: string }
	/** Runs the application, in the `userData` profile when one is given. */
	| { readonly kind: 'start'; readonly userData: string | undefined; readonly options: { readonly temporaryEngine: boolean; readonly automationDriver: boolean } };

/** The process identity, set before any service reads a path, and what the launch then does. */
interface LaunchPlan {
	readonly name: string;
	/** Windows groups taskbar buttons and notifications by this id. */
	readonly appUserModelId: string | undefined;
	/**
	 * The Linux desktop entry: Wayland's app id and X11's `WM_CLASS`, which
	 * desktops match to an installed `.desktop` file for the icon and portals.
	 * The source checkout has its own, so it never takes an installed build's
	 * grants or entry.
	 */
	readonly desktopName: string;
	readonly action: LaunchAction;
}

/**
 * A built application exposes no automation or debugging surface: the smoke
 * driver and Chromium's remote debugging would let anyone who can start it
 * drive the workbench and its engine.
 */
function refusedSwitch(context: LaunchContext): string | undefined {
	if (context.argv.includes('--smoke-test')) {
		return '--smoke-test';
	}
	return ['remote-debugging-port', 'remote-debugging-pipe'].find(name => context.hasSwitch(name));
}

function launchAction(context: LaunchContext): LaunchAction {
	const refused = context.built ? refusedSwitch(context) : undefined;
	if (refused) {
		return { kind: 'refuse', argument: refused };
	}
	const output = selfCheckOutput(context.argv);
	if (output !== undefined) {
		return { kind: 'selfCheck', output };
	}
	const smoke = context.argv.includes('--smoke-test');
	if (smoke && !context.testProfile) {
		return { kind: 'invalid', reason: '--smoke-test requires an isolated VNILINK_TEST_PROFILE' };
	}
	return { kind: 'start', userData: smoke ? context.testProfile : undefined, options: { temporaryEngine: smoke, automationDriver: smoke } };
}

/** Decides a launch from its arguments; smoke isolation is explicit and required. */
export function planLaunch(context: LaunchContext): LaunchPlan {
	return {
		name: product.name,
		appUserModelId: context.windows ? product.appId : undefined,
		desktopName: desktopName(context.built),
		action: launchAction(context),
	};
}

/** The product's desktop entry when built; from the source checkout, the same name with `.dev` before `.desktop`. */
export function desktopName(built: boolean): string {
	return built ? product.linux.desktopFileName : product.linux.desktopFileName.replace(/\.desktop$/, '.dev.desktop');
}
