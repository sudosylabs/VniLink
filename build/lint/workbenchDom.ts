// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { AST_NODE_TYPES, type TSESLint } from '@typescript-eslint/utils';

/**
 * Workbench views build DOM with VS Code's helpers (`$`, `append`, `h`) and
 * reusable widgets, and trigger behavior through commands rather than by
 * clicking their own elements.
 */
export const workbenchDomRule: TSESLint.RuleModule<'createElement' | 'click'> = {
	defaultOptions: [],
	meta: {
		type: 'problem',
		schema: [],
		messages: {
			createElement: 'Build workbench DOM with `$`, `append` or `h` from base/browser/dom, or with a base/browser/ui widget.',
			click: 'Run the command or method behind this element instead of clicking it from code.',
		},
	},
	create(context) {
		return {
			CallExpression(node) {
				const callee = node.callee;
				if (callee.type !== AST_NODE_TYPES.MemberExpression || callee.computed) {
					return;
				}
				if (callee.property.type !== AST_NODE_TYPES.Identifier) {
					return;
				}
				if (callee.property.name === 'createElement' && callee.object.type === AST_NODE_TYPES.Identifier && callee.object.name === 'document') {
					context.report({ node, messageId: 'createElement' });
				} else if (callee.property.name === 'click' && node.arguments.length === 0) {
					context.report({ node, messageId: 'click' });
				}
			},
		};
	},
};

export default { rules: { 'workbench-dom': workbenchDomRule } };
