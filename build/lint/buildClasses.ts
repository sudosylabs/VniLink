import type { TSESLint, TSESTree } from '@typescript-eslint/utils';

/**
 * Build code is plain functions composed with `define`, `series` and `parallel`, as
 * VS Code's build is. A class is allowed only where an object owns operating-system
 * resources for a lifetime: processes, file watchers, a development session or a socket.
 */
export const buildClassesRule: TSESLint.RuleModule<'class', [{ allow: string[] }]> = {
	defaultOptions: [{ allow: [] }],
	meta: {
		type: 'problem',
		schema: [{ type: 'object', properties: { allow: { type: 'array', items: { type: 'string' } } }, additionalProperties: false }],
		messages: { class: 'Write build steps as functions; classes are reserved for owners of processes, watchers, sessions or sockets.' },
	},
	create(context) {
		const allow = context.options.at(0)?.allow ?? [];
		const filename = context.filename.split('\\').join('/');
		if (allow.some(path => filename.endsWith(`/${path}`))) {
			return {};
		}
		const report = (node: TSESTree.ClassDeclaration | TSESTree.ClassExpression): void => {
			context.report({ node, messageId: 'class' });
		};
		return { ClassDeclaration: report, ClassExpression: report };
	},
};

export default { rules: { 'build-classes': buildClassesRule } };
