export const importExtensionsRule = {
	meta: { type: 'problem', schema: [], messages: { extension: 'Relative TypeScript imports must end in .js, .css, or .json with a JSON import attribute.' } },
	create(context) {
		function check(node, value) {
			if (typeof value === 'string' && value.startsWith('.') && !/\.(js|css)$/.test(value)) {
				context.report({ node, messageId: 'extension' });
			}
		}
		return {
			ImportDeclaration: node => {
				const json = node.source.value.endsWith('.json') && node.attributes?.some(attribute => attribute.key.name === 'type' && attribute.value.value === 'json');
				if (!json) {
					check(node.source, node.source.value);
				}
			},
			ExportNamedDeclaration: node => {
				if (node.source) {
					check(node.source, node.source.value);
				}
			},
			ExportAllDeclaration: node => check(node.source, node.source.value),
			ImportExpression: node => check(node.source, node.source.value),
			TSImportType: node => {
				const literal = node.argument ?? node.source;
				check(node, literal?.value ?? literal?.literal?.value);
			},
		};
	},
};

export default { rules: { 'import-extensions': importExtensionsRule } };
