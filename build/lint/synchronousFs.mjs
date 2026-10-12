export const synchronousFsRule = {
	meta: { type: 'problem', schema: [], messages: { synchronous: 'Use asynchronous filesystem APIs in application processes.' } },
	create(context) {
		return {
			ImportDeclaration(node) {
				if (!['fs', 'node:fs'].includes(node.source.value)) {
					return;
				}
				for (const specifier of node.specifiers) {
					if (specifier.type === 'ImportSpecifier' && /Sync$/.test(specifier.imported.name ?? specifier.imported.value)) {
						context.report({ node: specifier, messageId: 'synchronous' });
					} else if (specifier.type !== 'ImportSpecifier') {
						for (const variable of context.sourceCode.getDeclaredVariables(node)) {
							if (variable.name !== specifier.local.name) {
								continue;
							}
							for (const reference of variable.references) {
								const member = reference.identifier.parent;
								if (member?.type !== 'MemberExpression' || member.object !== reference.identifier) {
									continue;
								}
								const name = member.computed ? member.property.value : member.property.name;
								if (typeof name === 'string' && /Sync$/.test(name)) {
									context.report({ node: member, messageId: 'synchronous' });
								}
							}
						}
					}
				}
			},
		};
	},
};
export default { rules: { 'no-sync-fs': synchronousFsRule } };
