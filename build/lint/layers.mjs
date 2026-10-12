import { builtinModules } from 'node:module';
import { dirname, relative, resolve, sep } from 'node:path';
const builtins = new Set(builtinModules.map(name => name.replace(/^node:/, '')));
export const layersRule = {
	meta: { type: 'problem', schema: [], messages: { boundary: '{{reason}}: {{specifier}}' } },
	create(context) {
		const filename = context.filename;
		const source = relative(process.cwd(), filename).split(sep).join('/');
		const browser = source === 'src/renderer.ts' || source.includes('/browser/');
		const common = source.includes('/common/');
		const base = source.startsWith('src/base/');
		const platform = source.startsWith('src/platform/');
		const app = source.startsWith('src/app/');
		function check(node, specifier) {
			if (typeof specifier !== 'string') {
				return;
			}
			const native = specifier === 'electron' || specifier.startsWith('electron/') || specifier.startsWith('node:') || builtins.has(specifier);
			const target = specifier.startsWith('.') ? relative(process.cwd(), resolve(dirname(filename), specifier.replace(/\.js$/, '.ts'))).split(sep).join('/') : specifier;
			let reason;
			if ((browser || common) && native) {
				reason = 'Browser/common code cannot import Node or Electron';
			} else if ((browser || common) && (target.startsWith('src/app/') || target.includes('/electron-main/') || target.includes('/electron-utility/') || (target === 'src/preload' || target === 'src/preload.ts'))) {
				reason = 'Browser/common code cannot import privileged implementation';
			} else if (source.includes('/node/') && (target.includes('/browser/') || target.includes('/electron-main/') || target.includes('/electron-utility/') || target === 'src/preload.ts' || target === 'src/renderer.ts')) {
				reason = 'Node services cannot depend on Electron or browser implementation';
			} else if (source.includes('/electron-utility/') && (target.includes('/electron-main/') || target.includes('/browser/') || target === 'src/preload.ts' || target === 'src/renderer.ts')) {
				reason = 'Utility processes cannot import main, preload or browser implementation';
			} else if (common && (target.includes('/browser/') || target === 'src/renderer' || target === 'src/renderer.ts')) {
				reason = 'Common code cannot depend on DOM implementation';
			} else if (base && /^src\/(app|platform|workbench)\//.test(target)) {
				reason = 'Base controls cannot depend on application services or features';
			} else if (platform && /^src\/(app|workbench)\//.test(target)) {
				reason = 'Platform services cannot depend on the application or workbench';
			} else if (app && (target.includes('/browser/') || target === 'src/renderer' || target === 'src/renderer.ts')) {
				reason = 'Electron application code cannot import browser implementation';
			}
			if (reason) {
				context.report({ node, messageId: 'boundary', data: { reason, specifier } });
			}
		}
		return {
			ImportDeclaration: node => check(node, node.source.value),
			ExportNamedDeclaration: node => {
				if (node.source) {
					check(node, node.source.value);
				}
			},
			ExportAllDeclaration: node => check(node, node.source.value),
			ImportExpression: node => check(node, node.source.value),
			TSImportType: node => {
				const literal = node.argument ?? node.source; check(node, literal?.value ?? literal?.literal?.value);
			},
			CallExpression: node => {
				if (node.callee.type === 'Identifier' && node.callee.name === 'require') {
					check(node, node.arguments[0]?.value);
				}
			},
		};
	},
};
export default { rules: { layers: layersRule } };
