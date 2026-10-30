// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { builtinModules } from 'node:module';
import { dirname, relative, resolve, sep } from 'node:path';
import { AST_NODE_TYPES, type TSESLint, type TSESTree } from '@typescript-eslint/utils';

const builtins = new Set(builtinModules.map(name => name.replace(/^node:/, '')));

export const layersRule: TSESLint.RuleModule<'boundary'> = {
	defaultOptions: [],
	meta: { type: 'problem', schema: [], messages: { boundary: '{{reason}}: {{specifier}}' } },
	create(context) {
		const filename = context.filename;
		const source = relative(process.cwd(), filename).split(sep).join('/');
		// The workbench is renderer-only, so it has no environment folders of its own.
		const browser = source === 'src/renderer.ts' || source.includes('/browser/') || source.startsWith('src/workbench/');
		const common = source.includes('/common/');
		const base = source.startsWith('src/base/');
		const platform = source.startsWith('src/platform/');
		const app = source.startsWith('src/code/');
		function check(node: TSESTree.Node, specifier: unknown): void {
			if (typeof specifier !== 'string') {
				return;
			}
			const native = specifier === 'electron' || specifier.startsWith('electron/') || specifier.startsWith('node:') || builtins.has(specifier);
			const target = specifier.startsWith('.') ? relative(process.cwd(), resolve(dirname(filename), specifier.replace(/\.js$/, '.ts'))).split(sep).join('/') : specifier;
			let reason: string | undefined;
			if (specifier.startsWith('@phosphor-icons/') && source !== 'src/base/browser/ui/icon/iconAssets.ts') {
				reason = 'Use product icons through the shared icon helper';
			} else if ((browser || common) && native) {
				reason = 'Browser/common code cannot import Node or Electron';
			} else if ((browser || common) && (target.startsWith('src/code/') || target.includes('/electron-main/') || target.includes('/electron-utility/') || (target === 'src/preload' || target === 'src/preload.ts'))) {
				reason = 'Browser/common code cannot import privileged implementation';
			} else if (source.includes('/node/') && (target.includes('/browser/') || target.includes('/electron-main/') || target.includes('/electron-utility/') || target === 'src/preload.ts' || target === 'src/renderer.ts')) {
				reason = 'Node services cannot depend on Electron or browser implementation';
			} else if (source.includes('/electron-utility/') && (target.includes('/electron-main/') || target.includes('/browser/') || target === 'src/preload.ts' || target === 'src/renderer.ts')) {
				reason = 'Utility processes cannot import main, preload or browser implementation';
			} else if (common && (target.includes('/browser/') || target === 'src/renderer' || target === 'src/renderer.ts')) {
				reason = 'Common code cannot depend on DOM implementation';
			} else if (base && /^src\/(code|platform|workbench)\//.test(target)) {
				reason = 'Base controls cannot depend on application services or features';
			} else if (platform && /^src\/(code|workbench)\//.test(target)) {
				reason = 'Platform services cannot depend on the application or workbench';
			} else if (app && (target.includes('/browser/') || target === 'src/renderer' || target === 'src/renderer.ts')) {
				reason = 'Electron application code cannot import browser implementation';
			}
			if (reason) {
				context.report({ node, messageId: 'boundary', data: { reason, specifier } });
			}
		}
		return {
			ImportDeclaration: node => {
				check(node, node.source.value);
			},
			ExportNamedDeclaration: node => {
				if (node.source) {
					check(node, node.source.value);
				}
			},
			ExportAllDeclaration: node => {
				check(node, node.source.value);
			},
			ImportExpression: node => {
				check(node, node.source.type === AST_NODE_TYPES.Literal ? node.source.value : undefined);
			},
			TSImportType: node => {
				check(node, node.source.value);
			},
			CallExpression: node => {
				const argument = node.arguments.at(0);
				if (node.callee.type === AST_NODE_TYPES.Identifier && node.callee.name === 'require' && argument?.type === AST_NODE_TYPES.Literal) {
					check(node, argument.value);
				}
			},
		};
	},
};

export default { rules: { layers: layersRule } };
