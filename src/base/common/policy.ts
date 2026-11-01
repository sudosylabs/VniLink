/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/


export type PolicyName = string;

export type PolicyValue = string | number | boolean;
export type ManagedSettingValue = PolicyValue;

export interface IPolicy {

	/**
	 * The policy name.
	 */
	readonly name: PolicyName;
}

/**
 * A subordinate attachment to an existing {@link IPolicy} (the "owner"). A setting may declare a
 * `policyReference` instead of a full `policy` to be governed by a policy owned by another setting,
 * letting a single enterprise policy lock more than one setting (e.g. gating an agent in both the
 * editor window and the Agents window).
 *
 * A reference is a pure pointer: it carries no policy semantics of its own. The owner is the single
 * source of truth for the policy's catalog metadata *and* its runtime behaviour (type, value
 * callback, etc.); a reference only contributes the policy name so the setting is gated and the OS
 * policy watcher observes the name in processes where the owner is not loaded.
 */
export interface IPolicyReference {

	/** The name of the owning {@link IPolicy} this setting attaches to. */
	readonly name: PolicyName;
}
