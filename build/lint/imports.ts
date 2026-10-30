// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { AST_NODE_TYPES, type TSESLint, type TSESTree } from '@typescript-eslint/utils';

export const importExtensionsRule: TSESLint.RuleModule<'extension'> = {
	defaultOptions: [],
	meta: { type: 'problem', schema: [], messages: { extension: 'Relative TypeScript imports must end in .js, .css, or .json with a JSON import attribute.' } },
	create(context) {
		function check(node: TSESTree.Node, value: unknown): void {
			if (typeof value === 'string' && value.startsWith('.') && !/\.(js|css)$/.test(value)) {
				context.report({ node, messageId: 'extension' });
			}
		}
		return {
			ImportDeclaration: node => {
				const json = node.source.value.endsWith('.json') && node.attributes.some(attribute => (attribute.key.type === AST_NODE_TYPES.Identifier ? attribute.key.name : attribute.key.value) === 'type' && attribute.value.value === 'json');
				if (!json) {
					check(node.source, node.source.value);
				}
			},
			ExportNamedDeclaration: node => {
				if (node.source) {
					check(node.source, node.source.value);
				}
			},
			ExportAllDeclaration: node => {
				check(node.source, node.source.value);
			},
			ImportExpression: node => {
				check(node.source, node.source.type === AST_NODE_TYPES.Literal ? node.source.value : undefined);
			},
			TSImportType: node => {
				check(node, node.source.value);
			},
		};
	},
};

export default { rules: { 'import-extensions': importExtensionsRule } };
