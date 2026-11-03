/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: no product-defined extension unification; VniLink has no extensions.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { IStringDictionary } from '../../../base/common/collections.js';
import { Emitter, Event } from '../../../base/common/event.js';
import { IJSONSchema } from '../../../base/common/jsonSchema.js';
import * as types from '../../../base/common/types.js';
import * as nls from '../../../nls.js';
import { Extensions as JSONExtensions, IJSONContributionRegistry } from '../../jsonschemas/common/jsonContributionRegistry.js';
import { Registry } from '../../registry/common/platform.js';
import { IPolicy, IPolicyReference, ManagedSettingValue, PolicyName } from '../../../base/common/policy.js';
import { Disposable } from '../../../base/common/lifecycle.js';

export enum EditPresentationTypes {
	Multiline = 'multilineText',
	Singleline = 'singlelineText'
}

export const Extensions = {
	Configuration: 'base.contributions.configuration'
};

export type ManagedSettingsPresentationValue = ManagedSettingValue | readonly string[] | Readonly<Record<string, readonly string[]>>;

/**
 * Declares that a setting's value should be mirrored into the agent host's root
 * configuration, removing the need to hand-write a forwarder per setting.
 *
 * Only the setting's *globally-scoped* value is mirrored — the resolution order
 * is policy, then user, then application, then default. Workspace and folder
 * values are deliberately ignored: the agent host root config is shared by every
 * window connected to a host, so a workspace-specific value would leak across
 * unrelated windows on a last-writer-wins basis.
 *
 * A setting that genuinely needs per-workspace behavior should not use this;
 * push it through session config instead, where the session → parent → host
 * chain resolves precedence correctly.
 */
export interface IAgentHostConfigurationSync {

	/** The agent host root configuration key to write the value to. */
	readonly key: string;

	/**
	 * Maps the globally-scoped setting value to the value written to {@link key}.
	 * Defaults to the identity, which is what most settings want: the resolver
	 * already skips layers whose value does not conform to the declared `type`
	 * and falls back to the registered default, so a transform is only needed to
	 * change the *shape* of the value (for example mapping an enum to a
	 * different representation the agent host expects).
	 */
	readonly transform?: (value: unknown) => unknown;

	/** Which Agent Host targets receive this setting. Defaults to {@link AgentHostConfigurationSyncScope.All}. */
	readonly scope?: AgentHostConfigurationSyncScope;
}

export const enum AgentHostConfigurationSyncScope {
	/** Mirror to every Agent Host connection. */
	All = 'all',
	/** Mirror only to the local utility-process Agent Host. */
	Local = 'local',
	/** Mirror to the ambient Agent Host, whether local or colocated with a remote extension host. */
	Ambient = 'ambient',
}

export interface IConfigurationRegistry {

	/**
	 * Register a configuration to the registry.
	 */
	registerConfiguration(configuration: IConfigurationNode): IConfigurationNode;

	/**
	 * Register multiple configurations to the registry.
	 */
	registerConfigurations(configurations: IConfigurationNode[], validate?: boolean): void;

	/**
	 * Event that fires whenever a configuration has been
	 * registered.
	 */
	readonly onDidSchemaChange: Event<void>;

	/**
	 * Event that fires whenever a configuration has been
	 * registered.
	 */
	readonly onDidUpdateConfiguration: Event<{ properties: ReadonlySet<string>; defaultsOverrides?: boolean }>;

	/**
	 * Returns all configurations settings of all configuration nodes contributed to this registry.
	 */
	getConfigurationProperties(): IStringDictionary<IRegisteredConfigurationPropertySchema>;

	/**
	 * Returns the owning setting key per policy name (at most one owner per name).
	 */
	getPolicyConfigurations(): Map<PolicyName, string>;

	/**
	 * Returns the {@link IAgentHostConfigurationSync} descriptor per setting key
	 * for every setting that declares one. Includes settings hidden from the
	 * Settings UI via `included: false`, which are absent from
	 * {@link getConfigurationProperties}.
	 */
	getAgentHostSyncConfigurations(): Map<string, IAgentHostConfigurationSync>;
}

export const enum ConfigurationScope {
	/**
	 * Application specific configuration, which can be configured only in default profile user settings.
	 */
	APPLICATION = 1,
	/**
	 * Machine specific configuration, which can be configured only in local and remote user settings.
	 */
	MACHINE,
	/**
	 * An application machine specific configuration, which can be configured only in default profile user settings and remote user settings.
	 */
	APPLICATION_MACHINE,
	/**
	 * Window specific configuration, which can be configured in the user or workspace settings.
	 */
	WINDOW,
	/**
	 * Resource specific configuration, which can be configured in the user, workspace or folder settings.
	 */
	RESOURCE,
	/**
	 * Resource specific configuration that can be configured in language specific settings
	 */
	LANGUAGE_OVERRIDABLE,
	/**
	 * Machine specific configuration that can also be configured in workspace or folder settings.
	 */
	MACHINE_OVERRIDABLE,
}


export interface IConfigurationPropertySchema extends IJSONSchema {

	scope?: ConfigurationScope;

	/**
	 * When restricted, value of this configuration will be read only from trusted sources.
	 * For eg., If the workspace is not trusted, then the value of this configuration is not read from workspace settings file.
	 */
	restricted?: boolean;

	/**
	 * When `false` this property is excluded from the registry. Default is to include.
	 */
	included?: boolean;

	/**
	 * Keep this setting visible in the Settings editor even when it has a deprecation message and is not configured.
	 * Defaults to false; use for advance deprecation warnings.
	 */
	deprecationMessageShowInSettings?: boolean;

	/**
	 * List of tags associated to the property.
	 *  - A tag can be used for filtering
	 *  - Use `experimental` tag for marking the setting as experimental.
	 */
	tags?: string[];

	/**
	 * When enabled this setting is ignored during sync and user can override this.
	 */
	ignoreSync?: boolean;

	/**
	 * When enabled this setting is ignored during sync and user cannot override this.
	 */
	disallowSyncIgnore?: boolean;

	/**
	 * Disallow extensions to contribute configuration default value for this setting.
	 */
	disallowConfigurationDefault?: boolean;

	/**
	 * Labels for enumeration items
	 */
	enumItemLabels?: string[];

	/**
	 * Optional keywords used for search purposes.
	 */
	keywords?: string[];

	/**
	 * When specified, controls the presentation format of string settings.
	 * Otherwise, the presentation format defaults to `singleline`.
	 */
	editPresentation?: EditPresentationTypes;

	/**
	 * When specified, gives an order number for the setting
	 * within the settings editor. Otherwise, the setting is placed at the end.
	 */
	order?: number;

	/**
	 * When specified, this setting's value can always be overwritten by
	 * a system-wide policy. Exactly one setting may *own* a given policy name.
	 */
	policy?: IPolicy;

	/**
	 * When specified, this setting is governed by a policy owned by another setting.
	 * A setting must not declare both `policy` and `policyReference`.
	 * The non-null types must match the owning setting (enforced when exporting the policy catalog).
	 */
	policyReference?: IPolicyReference;

	/**
	 * Projects runtime-managed restrictions into the Settings UI without changing configuration values.
	 * Return undefined when editable. Runtime policy enforcement remains authoritative.
	 */
	managedSettingsPresentation?: (read: (key: string) => ManagedSettingValue | undefined) => ManagedSettingsPresentationValue | undefined;

	/**
	 * When specified, this setting's globally-scoped value is mirrored into the
	 * agent host's root configuration automatically, without any per-setting
	 * plumbing. See {@link IAgentHostConfigurationSync}.
	 */
	agentHost?: IAgentHostConfigurationSync;

	/**
	 * When specified, this setting's default value can always be overwritten by
	 * an experiment.
	 */
	experiment?: {
		/**
		 * The mode of the experiment.
		 * - `startup`: The setting value is updated to the experiment value only on startup.
		 * - `auto`: The setting value is updated to the experiment value automatically (whenever the experiment value changes).
		 */
		mode: 'startup' | 'auto';

		/**
		 * The name of the experiment. By default, this is `config.${settingId}`
		 */
		name?: string;
	};

	/**
	 * When specified, provides configuration overrides for the Agents window.
	 */
	agentsWindow?: {
		/**
		 * Override default value for this setting in the Agents window.
		 */
		default?: unknown;

		/**
		 * When `true`, this setting is read-only in the Agents window
		 * and cannot be changed by the user.
		 */
		readOnly?: boolean;
	};
}

export interface IExtensionInfo {
	id: string;
	displayName?: string;
}

export interface IConfigurationNode {
	id?: string;
	order?: number;
	type?: string | string[];
	title?: string;
	description?: string;
	properties?: IStringDictionary<IConfigurationPropertySchema>;
	allOf?: IConfigurationNode[];
	scope?: ConfigurationScope;
	extensionInfo?: IExtensionInfo;
	restrictedProperties?: string[];
}

export type ConfigurationDefaultSource = IExtensionInfo | string;

export type ConfigurationDefaultValueSource = ConfigurationDefaultSource | Map<string, ConfigurationDefaultSource>;

export type IRegisteredConfigurationPropertySchema = IConfigurationPropertySchema & {
	section?: {
		id?: string;
		title?: string;
		order?: number;
		extensionInfo?: IExtensionInfo;
	};
	defaultDefaultValue?: unknown;
	source?: ConfigurationDefaultSource; // Source of the Property
	defaultValueSource?: ConfigurationDefaultValueSource; // Source of the Default Value
};

export interface IConfigurationDefaultOverride {
	readonly value: unknown;
	readonly source?: ConfigurationDefaultSource;  // Source of the default override
}

export interface IConfigurationDefaultOverrideValue {
	readonly value: unknown;
	readonly source?: ConfigurationDefaultValueSource;
}

export const allSettings: { properties: IStringDictionary<IConfigurationPropertySchema>; patternProperties: IStringDictionary<IConfigurationPropertySchema> } = { properties: {}, patternProperties: {} };
export const applicationSettings: { properties: IStringDictionary<IConfigurationPropertySchema>; patternProperties: IStringDictionary<IConfigurationPropertySchema> } = { properties: {}, patternProperties: {} };
export const applicationMachineSettings: { properties: IStringDictionary<IConfigurationPropertySchema>; patternProperties: IStringDictionary<IConfigurationPropertySchema> } = { properties: {}, patternProperties: {} };
export const machineSettings: { properties: IStringDictionary<IConfigurationPropertySchema>; patternProperties: IStringDictionary<IConfigurationPropertySchema> } = { properties: {}, patternProperties: {} };
export const machineOverridableSettings: { properties: IStringDictionary<IConfigurationPropertySchema>; patternProperties: IStringDictionary<IConfigurationPropertySchema> } = { properties: {}, patternProperties: {} };
export const windowSettings: { properties: IStringDictionary<IConfigurationPropertySchema>; patternProperties: IStringDictionary<IConfigurationPropertySchema> } = { properties: {}, patternProperties: {} };
export const resourceSettings: { properties: IStringDictionary<IConfigurationPropertySchema>; patternProperties: IStringDictionary<IConfigurationPropertySchema> } = { properties: {}, patternProperties: {} };

export const resourceLanguageSettingsSchemaId = 'vscode://schemas/settings/resourceLanguage';

const contributionRegistry = Registry.as<IJSONContributionRegistry>(JSONExtensions.JSONContribution);

class ConfigurationRegistry extends Disposable implements IConfigurationRegistry {

	private readonly configurationDefaultsOverrides: Map<string, { configurationDefaultOverrides: IConfigurationDefaultOverride[]; configurationDefaultOverrideValue?: IConfigurationDefaultOverrideValue }>;
	private readonly defaultLanguageConfigurationOverridesNode: IConfigurationNode;
	private readonly configurationContributors: IConfigurationNode[];
	private readonly configurationProperties: IStringDictionary<IRegisteredConfigurationPropertySchema>;
	private readonly policyConfigurations: Map<PolicyName, string>;
	private readonly policyReferenceConfigurations: Map<PolicyName, Set<string>>;
	private readonly agentHostSyncConfigurations: Map<string, IAgentHostConfigurationSync>;
	/**
	 * Agent-host-mirrored or experimental setting keys per node hidden with
	 * `included: false`. Registration deletes those keys from the node's
	 * `properties`, so deregistration has no other way to find them.
	 */
	private readonly excludedConfigurationKeys = new Map<IConfigurationNode, Set<string>>();
	private readonly excludedConfigurationProperties: IStringDictionary<IRegisteredConfigurationPropertySchema>;
	private readonly resourceLanguageSettingsSchema: IJSONSchema;

	private readonly _onDidSchemaChange = this._register(new Emitter<void>());
	readonly onDidSchemaChange: Event<void> = this._onDidSchemaChange.event;

	private readonly _onDidUpdateConfiguration = this._register(new Emitter<{ properties: ReadonlySet<string>; defaultsOverrides?: boolean }>());
	readonly onDidUpdateConfiguration = this._onDidUpdateConfiguration.event;

	constructor() {
		super();
		this.configurationDefaultsOverrides = new Map();
		this.defaultLanguageConfigurationOverridesNode = {
			id: 'defaultOverrides',
			title: nls.localize('defaultLanguageConfigurationOverrides.title', "Default Language Configuration Overrides"),
			properties: {}
		};
		this.configurationContributors = [this.defaultLanguageConfigurationOverridesNode];
		this.resourceLanguageSettingsSchema = {
			properties: {},
			patternProperties: {},
			additionalProperties: true,
			allowTrailingCommas: true,
			allowComments: true
		};
		this.configurationProperties = {};
		this.policyConfigurations = new Map<PolicyName, string>();
		this.policyReferenceConfigurations = new Map<PolicyName, Set<string>>();
		this.agentHostSyncConfigurations = new Map<string, IAgentHostConfigurationSync>();
		this.excludedConfigurationProperties = {};

		contributionRegistry.registerSchema(resourceLanguageSettingsSchemaId, this.resourceLanguageSettingsSchema);
		this.registerOverridePropertyPatternKey();
	}

	public registerConfiguration(configuration: IConfigurationNode, validate: boolean = true): IConfigurationNode {
		this.registerConfigurations([configuration], validate);
		return configuration;
	}

	public registerConfigurations(configurations: IConfigurationNode[], validate: boolean = true): void {
		const properties = new Set<string>();
		this.doRegisterConfigurations(configurations, validate, properties);

		contributionRegistry.registerSchema(resourceLanguageSettingsSchemaId, this.resourceLanguageSettingsSchema);
		this._onDidSchemaChange.fire();
		this._onDidUpdateConfiguration.fire({ properties });
	}

	private doRegisterConfigurations(configurations: IConfigurationNode[], validate: boolean, bucket: Set<string>): void {

		configurations.forEach(configuration => {

			this.validateAndRegisterProperties(configuration, validate, configuration.extensionInfo, configuration.restrictedProperties, undefined, bucket);

			this.configurationContributors.push(configuration);
			this.registerJSONConfiguration(configuration);
		});
	}

	private validateAndRegisterProperties(configuration: IConfigurationNode, validate: boolean = true, extensionInfo: IExtensionInfo | undefined, restrictedProperties: string[] | undefined, scope: ConfigurationScope = ConfigurationScope.WINDOW, bucket: Set<string>): void {
		scope = types.isUndefinedOrNull(configuration.scope) ? scope : configuration.scope;
		const properties = configuration.properties;
		if (properties) {
			for (const key in properties) {
				const property: IRegisteredConfigurationPropertySchema = properties[key];
				property.section = {
					id: configuration.id,
					title: configuration.title,
					order: configuration.order,
					extensionInfo: configuration.extensionInfo
				};
				if (validate && validateProperty(key, property, extensionInfo?.id)) {
					delete properties[key];
					continue;
				}

				property.source = extensionInfo;

				// update default value
				property.defaultDefaultValue = properties[key].default;
				this.updatePropertyDefaultValue(key, property);

				// update scope
				if (OVERRIDE_PROPERTY_REGEX.test(key)) {
					property.scope = undefined; // No scope for overridable properties `[${identifier}]`
				} else {
					property.scope = types.isUndefinedOrNull(property.scope) ? scope : property.scope;
					property.restricted = types.isUndefinedOrNull(property.restricted) ? !!restrictedProperties?.includes(key) : property.restricted;
				}

				if (property.experiment) {
					if (!property.tags?.some(tag => tag.toLowerCase() === 'onexp')) {
						property.tags = property.tags ?? [];
						property.tags.push('onExP');
					}
				} else if (property.tags?.some(tag => tag.toLowerCase() === 'onexp')) {
					console.error(`Invalid tag 'onExP' found for property '${key}'. Please use 'experiment' property instead.`);
					property.experiment = { mode: 'startup' };
				}

				const excluded = properties[key].hasOwnProperty('included') && !properties[key].included;
				const policyName = properties[key].policy?.name;
				const policyReferenceName = properties[key].policyReference?.name;
				const agentHostSync = properties[key].agentHost;

				if (agentHostSync) {
					this.agentHostSyncConfigurations.set(key, agentHostSync);
				}

				if (excluded) {
					this.excludedConfigurationProperties[key] = properties[key];
					if (policyName) {
						this.policyConfigurations.set(policyName, key);
						bucket.add(key);
					}
					if (policyReferenceName) {
						this.addPolicyReferenceConfiguration(policyReferenceName, key);
						bucket.add(key);
					}
					if (agentHostSync || property.experiment) {
						// Hidden settings can participate in experiments and host
						// mirroring without entering the Settings UI schemas.
						bucket.add(key);
						let excludedKeys = this.excludedConfigurationKeys.get(configuration);
						if (!excludedKeys) {
							excludedKeys = new Set<string>();
							this.excludedConfigurationKeys.set(configuration, excludedKeys);
						}
						excludedKeys.add(key);
					}
					delete properties[key];
				} else {
					bucket.add(key);
					if (policyName) {
						this.policyConfigurations.set(policyName, key);
					}
					if (policyReferenceName) {
						this.addPolicyReferenceConfiguration(policyReferenceName, key);
					}
					this.configurationProperties[key] = properties[key];
					if (!properties[key].deprecationMessage && properties[key].markdownDeprecationMessage) {
						// If not set, default deprecationMessage to the markdown source
						properties[key].deprecationMessage = properties[key].markdownDeprecationMessage;
					}
				}


			}
		}
		const subNodes = configuration.allOf;
		if (subNodes) {
			for (const node of subNodes) {
				this.validateAndRegisterProperties(node, validate, extensionInfo, restrictedProperties, scope, bucket);
			}
		}
	}

	private addPolicyReferenceConfiguration(policyName: PolicyName, key: string): void {
		let keys = this.policyReferenceConfigurations.get(policyName);
		if (!keys) {
			keys = new Set<string>();
			this.policyReferenceConfigurations.set(policyName, keys);
		}
		keys.add(key);
	}

	getConfigurationProperties(): IStringDictionary<IRegisteredConfigurationPropertySchema> {
		return this.configurationProperties;
	}

	getPolicyConfigurations(): Map<PolicyName, string> {
		return this.policyConfigurations;
	}

	getAgentHostSyncConfigurations(): Map<string, IAgentHostConfigurationSync> {
		return this.agentHostSyncConfigurations;
	}

	private registerJSONConfiguration(configuration: IConfigurationNode) {
		const register = (configuration: IConfigurationNode) => {
			const properties = configuration.properties;
			if (properties) {
				for (const key in properties) {
					this.updateSchema(key, properties[key]);
				}
			}
			const subNodes = configuration.allOf;
			subNodes?.forEach(register);
		};
		register(configuration);
	}

	private updateSchema(key: string, property: IConfigurationPropertySchema): void {
		allSettings.properties[key] = property;
		switch (property.scope) {
			case ConfigurationScope.APPLICATION:
				applicationSettings.properties[key] = property;
				break;
			case ConfigurationScope.MACHINE:
				machineSettings.properties[key] = property;
				break;
			case ConfigurationScope.APPLICATION_MACHINE:
				applicationMachineSettings.properties[key] = property;
				break;
			case ConfigurationScope.MACHINE_OVERRIDABLE:
				machineOverridableSettings.properties[key] = property;
				break;
			case ConfigurationScope.WINDOW:
				windowSettings.properties[key] = property;
				break;
			case ConfigurationScope.RESOURCE:
				resourceSettings.properties[key] = property;
				break;
			case ConfigurationScope.LANGUAGE_OVERRIDABLE:
				resourceSettings.properties[key] = property;
				this.resourceLanguageSettingsSchema.properties![key] = property;
				break;
		}
	}

	private registerOverridePropertyPatternKey(): void {
		const resourceLanguagePropertiesSchema: IJSONSchema = {
			type: 'object',
			description: nls.localize('overrideSettings.defaultDescription', "Configure editor settings to be overridden for a language."),
			errorMessage: nls.localize('overrideSettings.errorMessage', "This setting does not support per-language configuration."),
			$ref: resourceLanguageSettingsSchemaId,
		};
		allSettings.patternProperties[OVERRIDE_PROPERTY_PATTERN] = resourceLanguagePropertiesSchema;
		applicationSettings.patternProperties[OVERRIDE_PROPERTY_PATTERN] = resourceLanguagePropertiesSchema;
		applicationMachineSettings.patternProperties[OVERRIDE_PROPERTY_PATTERN] = resourceLanguagePropertiesSchema;
		machineSettings.patternProperties[OVERRIDE_PROPERTY_PATTERN] = resourceLanguagePropertiesSchema;
		machineOverridableSettings.patternProperties[OVERRIDE_PROPERTY_PATTERN] = resourceLanguagePropertiesSchema;
		windowSettings.patternProperties[OVERRIDE_PROPERTY_PATTERN] = resourceLanguagePropertiesSchema;
		resourceSettings.patternProperties[OVERRIDE_PROPERTY_PATTERN] = resourceLanguagePropertiesSchema;
		this._onDidSchemaChange.fire();
	}

	private updatePropertyDefaultValue(key: string, property: IRegisteredConfigurationPropertySchema): void {
		const configurationdefaultOverride = this.configurationDefaultsOverrides.get(key)?.configurationDefaultOverrideValue;
		let defaultValue = undefined;
		let defaultSource = undefined;
		if (configurationdefaultOverride
			&& (!property.disallowConfigurationDefault || !configurationdefaultOverride.source) // Prevent overriding the default value if the property is disallowed to be overridden by configuration defaults from extensions
		) {
			defaultValue = configurationdefaultOverride.value;
			defaultSource = configurationdefaultOverride.source;
		}
		if (types.isUndefined(defaultValue)) {
			defaultValue = property.defaultDefaultValue;
			defaultSource = undefined;
		}
		if (types.isUndefined(defaultValue)) {
			defaultValue = getDefaultValue(property.type);
		}
		property.default = defaultValue;
		property.defaultValueSource = defaultSource;
	}
}

const OVERRIDE_IDENTIFIER_PATTERN = `\\[([^\\]]+)\\]`;
export const OVERRIDE_PROPERTY_PATTERN = `^(${OVERRIDE_IDENTIFIER_PATTERN})+$`;
export const OVERRIDE_PROPERTY_REGEX = new RegExp(OVERRIDE_PROPERTY_PATTERN);

export function getDefaultValue(type: string | string[] | undefined) {
	const t = Array.isArray(type) ? type[0] : <string>type;
	switch (t) {
		case 'boolean':
			return false;
		case 'integer':
		case 'number':
			return 0;
		case 'string':
			return '';
		case 'array':
			return [];
		case 'object':
			return {};
		default:
			return null;
	}
}

const configurationRegistry = new ConfigurationRegistry();
Registry.add(Extensions.Configuration, configurationRegistry);

export function validateProperty(property: string, schema: IRegisteredConfigurationPropertySchema, extensionId?: string): string | null {
	if (!property.trim()) {
		return nls.localize('config.property.empty', "Cannot register an empty property");
	}
	if (OVERRIDE_PROPERTY_REGEX.test(property)) {
		return nls.localize('config.property.languageDefault', "Cannot register '{0}'. This matches property pattern '\\\\[.*\\\\]$' for describing language specific editor settings. Use 'configurationDefaults' contribution.", property);
	}
	if (configurationRegistry.getConfigurationProperties()[property] !== undefined && (!extensionId || !EXTENSION_UNIFICATION_EXTENSION_IDS.has(extensionId.toLowerCase()))) {
		return nls.localize('config.property.duplicate', "Cannot register '{0}'. This property is already registered.", property);
	}
	if (schema.policy && schema.policyReference) {
		return nls.localize('config.policy.bothPolicyAndReference', "Cannot register '{0}'. A setting must not declare both 'policy' and 'policyReference'.", property);
	}
	if (schema.policy?.name && configurationRegistry.getPolicyConfigurations().get(schema.policy?.name) !== undefined) {
		return nls.localize('config.policy.duplicate', "Cannot register '{0}'. The associated policy {1} is already registered with {2}. To attach another setting to the same policy, use 'policyReference'.", property, schema.policy?.name, configurationRegistry.getPolicyConfigurations().get(schema.policy?.name));
	}
	if (schema.agentHost) {
		for (const [owner, sync] of configurationRegistry.getAgentHostSyncConfigurations()) {
			if (sync.key === schema.agentHost.key && owner !== property) {
				return nls.localize('config.agentHost.duplicate', "Cannot register '{0}'. The agent host configuration key '{1}' is already mirrored from '{2}'.", property, schema.agentHost.key, owner);
			}
		}
	}
	return null;
}

// Used for extension unification. Should be removed when complete.
export const EXTENSION_UNIFICATION_EXTENSION_IDS: Set<string> = new Set();
