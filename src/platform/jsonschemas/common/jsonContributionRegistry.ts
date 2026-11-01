/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { Emitter } from '../../../base/common/event.js';
import { IJSONSchema } from '../../../base/common/jsonSchema.js';
import { Disposable, DisposableStore, toDisposable } from '../../../base/common/lifecycle.js';
import * as platform from '../../registry/common/platform.js';

export const Extensions = {
	JSONContribution: 'base.contributions.json'
};

export interface IJSONContributionRegistry {

	/**
	 * Register a schema to the registry.
	 */
	registerSchema(uri: string, unresolvedSchemaContent: IJSONSchema, store?: DisposableStore): void;
}



function normalizeId(id: string) {
	if (id.length > 0 && id.charAt(id.length - 1) === '#') {
		return id.substring(0, id.length - 1);
	}
	return id;
}



class JSONContributionRegistry extends Disposable implements IJSONContributionRegistry {

	private readonly schemasById: { [id: string]: IJSONSchema } = {};

	private readonly _onDidChangeSchema = this._register(new Emitter<string>());

	public registerSchema(uri: string, unresolvedSchemaContent: IJSONSchema, store?: DisposableStore): void {
		const normalizedUri = normalizeId(uri);
		this.schemasById[normalizedUri] = unresolvedSchemaContent;
		this._onDidChangeSchema.fire(uri);

		if (store) {
			store.add(toDisposable(() => {
				delete this.schemasById[normalizedUri];
				this._onDidChangeSchema.fire(uri);
			}));
		}
	}
}

const jsonContributionRegistry = new JSONContributionRegistry();
platform.Registry.add(Extensions.JSONContribution, jsonContributionRegistry);
