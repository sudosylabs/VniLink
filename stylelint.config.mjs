import { upstreamIgnores } from './build/sourceScope.mjs';
export default {
	ignoreFiles: upstreamIgnores,
	extends: ['stylelint-config-standard'],
	rules: {
		'property-no-unknown': [true, { ignoreProperties: ['app-region', '-webkit-app-region'] }],
		'custom-property-pattern': '^[a-z][a-z0-9]*(-[a-z0-9]+)*$',
		'selector-class-pattern': '^[a-z][a-z0-9]*(-[a-z0-9]+)*$',
	},
};
