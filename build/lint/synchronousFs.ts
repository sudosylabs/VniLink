import { AST_NODE_TYPES, type TSESLint } from '@typescript-eslint/utils';

export const synchronousFsRule: TSESLint.RuleModule<'synchronous'> = {
	defaultOptions: [],
	meta: { type: 'problem', schema: [], messages: { synchronous: 'Use asynchronous filesystem APIs in application processes.' } },
	create(context) {
		return {
			ImportDeclaration(node) {
				if (!['fs', 'node:fs'].includes(node.source.value)) {
					return;
				}
				for (const specifier of node.specifiers) {
					if (specifier.type === AST_NODE_TYPES.ImportSpecifier && (specifier.imported.type === AST_NODE_TYPES.Identifier ? specifier.imported.name : specifier.imported.value).endsWith('Sync')) {
						context.report({ node: specifier, messageId: 'synchronous' });
					} else if (specifier.type !== AST_NODE_TYPES.ImportSpecifier) {
						for (const variable of context.sourceCode.getDeclaredVariables(node)) {
							if (variable.name !== specifier.local.name) {
								continue;
							}
							for (const reference of variable.references) {
								const member = reference.identifier.parent;
								if (member.type !== AST_NODE_TYPES.MemberExpression || member.object !== reference.identifier) {
									continue;
								}
								const property = member.property;
								const name = property.type === AST_NODE_TYPES.Literal ? property.value
									: property.type === AST_NODE_TYPES.Identifier && !member.computed ? property.name
										: undefined;
								if (typeof name === 'string' && name.endsWith('Sync')) {
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
