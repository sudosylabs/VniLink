// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Finds declarations in `src/` that nothing uses, with the TypeScript checker
 * over the root `tsconfig.json` (application and test TypeScript).
 *
 * Test code is `test/**` and every `test` folder under `src/`, as VS Code
 * places a module's tests in `<layer>/../test/<layer>/`. Its declarations are
 * not candidates and its uses count as uses by tests.
 *
 * Candidates are top-level declarations, members of namespaces, and class and
 * interface members. Liveness is a mark from roots: a use in module-level code
 * or in a test is a root, and a use inside a declaration counts once that
 * declaration is live, so code used only by unused code is reported too. A
 * member is live with any member it implements, overrides or structurally
 * satisfies: a call through `IDisposable` keeps `dispose` of every class
 * assignable to it.
 *
 * The checker cannot see uses by string: IPC dispatch on a method name, or a
 * `.mjs` test that bundles a source file. `build/lint/unused-allowlist.json`
 * names those, each with its reason; an entry that is no longer needed fails.
 */
import ts from 'typescript';
import { readFile } from 'node:fs/promises';
import { join, relative, sep } from 'node:path';
import { readManifest } from './vscode/manifest.ts';

export interface UnusedDeclaration {
	/** `name`, or `Owner.member` for a member. */
	readonly name: string;
	readonly path: string;
	readonly line: number;
	/** Live only through a use in `test/`. */
	readonly testOnly: boolean;
}

export interface UnusedReport {
	readonly unused: readonly UnusedDeclaration[];
	/** Exports of application files that only their own file uses. */
	readonly localExports: readonly UnusedDeclaration[];
}

interface Candidate {
	readonly name: string;
	readonly declarations: ts.Declaration[];
	readonly path: string;
	readonly line: number;
	readonly exported: boolean;
	readonly owner?: Candidate;
}

interface Use {
	readonly targets: readonly ts.Declaration[];
	/** The innermost candidate that encloses the use, if any. */
	readonly within: Candidate | undefined;
	readonly test: boolean;
	readonly file: string;
}

/** A union's members, a promise's value and the type itself: where an object literal's properties may land. */
function constituents(type: ts.Type): ts.Type[] {
	const all = type.isUnion() ? type.types : [type];
	return all.flatMap(member => {
		const awaited = member.getProperty('then') && (member as ts.TypeReference).typeArguments?.[0];
		return awaited ? [member, awaited] : [member];
	});
}

/** The object literal an initializer holds, through `as const` and parentheses. */
function objectLiteralOf(expression: ts.Expression): ts.ObjectLiteralExpression | undefined {
	let current = expression;
	while (ts.isAsExpression(current) || ts.isSatisfiesExpression(current) || ts.isParenthesizedExpression(current)) {
		current = current.expression;
	}
	return ts.isObjectLiteralExpression(current) ? current : undefined;
}

function literalNames(type: ts.Type): string[] {
	return (type.isUnion() ? type.types : [type]).filter(constituent => constituent.isStringLiteral()).map(constituent => constituent.value);
}

const memberKinds = new Set([
	ts.SyntaxKind.MethodDeclaration, ts.SyntaxKind.PropertyDeclaration, ts.SyntaxKind.GetAccessor, ts.SyntaxKind.SetAccessor,
	ts.SyntaxKind.MethodSignature, ts.SyntaxKind.PropertySignature,
]);

function slash(path: string): string {
	return path.split(sep).join('/');
}

function nameText(name: ts.Node | undefined): string | undefined {
	return name && (ts.isIdentifier(name) || ts.isPrivateIdentifier(name) || ts.isStringLiteral(name)) ? name.text : undefined;
}

function isExported(node: ts.Node): boolean {
	return ts.canHaveModifiers(node) && (ts.getModifiers(node) ?? []).some(modifier => modifier.kind === ts.SyntaxKind.ExportKeyword);
}

function isDeclarationName(node: ts.Identifier | ts.PrivateIdentifier): boolean {
	const parent = node.parent;
	if (ts.isImportSpecifier(parent) || ts.isImportClause(parent) || ts.isNamespaceImport(parent) || ts.isImportEqualsDeclaration(parent)) {
		return true;
	}
	if (ts.isExportSpecifier(parent)) {
		return true;
	}
	// Every node whose `name` is this identifier declares it, except a member access.
	return 'name' in parent && parent.name === node && !ts.isPropertyAccessExpression(parent) && !ts.isMetaProperty(parent);
}

/** `name` in `const { name } = value`, which reads `value.name`. */
function isShorthandBinding(node: ts.Identifier): boolean {
	return ts.isBindingElement(node.parent) && !node.parent.propertyName && ts.isObjectBindingPattern(node.parent.parent);
}

/** Computes the report for the program rooted at `root`'s `tsconfig.json`. */
export async function findUnused(root: string, allowed: ReadonlySet<string> = new Set()): Promise<UnusedReport> {
	const host: ts.ParseConfigFileHost = {
		...ts.sys,
		onUnRecoverableConfigFileDiagnostic: diagnostic => {
			throw new Error(ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n'));
		},
	};
	const parsed = ts.getParsedCommandLineOfConfigFile(join(root, 'tsconfig.json'), {}, host);
	if (!parsed) {
		throw new Error('tsconfig.json could not be read');
	}
	const program = ts.createProgram({ rootNames: parsed.fileNames, options: parsed.options });
	const checker = program.getTypeChecker();
	// TypeScript names files with forward slashes on every OS; compare in that form.
	const src = `${slash(join(root, 'src'))}/`;
	const test = `${slash(join(root, 'test'))}/`;
	const files = program.getSourceFiles().filter(file => file.fileName.startsWith(src) || file.fileName.startsWith(test));
	const isTestCode = (file: ts.SourceFile): boolean => file.fileName.startsWith(test)
		|| (file.fileName.startsWith(src) && slash(relative(src, file.fileName)).split('/').includes('test'));

	// Verbatim and pruned copies; adapted and derived files are application code.
	const manifest = await readManifest(root);
	const verbatim = new Set(manifest.files.filter(file => !file.adapted).map(file => slash(join(root, file.path))));
	const copied = (file: ts.SourceFile): boolean => verbatim.has(slash(file.fileName));
	const candidates: Candidate[] = [];
	const byDeclaration = new Map<ts.Node, Candidate>();
	const position = (node: ts.Node): { path: string; line: number } => {
		const file = node.getSourceFile();
		return { path: slash(relative(root, file.fileName)), line: file.getLineAndCharacterOfPosition(node.getStart()).line + 1 };
	};
	const addCandidate = (name: string, declarations: ts.Declaration[], exported: boolean, owner?: Candidate): Candidate => {
		const existing = byDeclaration.get(declarations[0]);
		if (existing) {
			return existing;
		}
		const candidate: Candidate = { name, declarations, ...position(declarations[0]), exported, owner };
		candidates.push(candidate);
		for (const declaration of declarations) {
			byDeclaration.set(declaration, candidate);
		}
		return candidate;
	};
	const declarationsOf = (node: ts.Declaration, name: ts.Node | undefined): ts.Declaration[] => {
		const symbol = name ? checker.getSymbolAtLocation(name) : undefined;
		return (symbol?.declarations ?? [node]).filter(declaration => declaration.getSourceFile() === node.getSourceFile());
	};
	const collectMembers = (owner: Candidate, members: readonly ts.Node[]): void => {
		for (const member of members) {
			if (!memberKinds.has(member.kind)) {
				continue;
			}
			const declaration = member as ts.ClassElement | ts.TypeElement;
			const name = nameText(declaration.name);
			const declared = ts.canHaveModifiers(member) && (ts.getModifiers(member) ?? []).some(modifier => modifier.kind === ts.SyntaxKind.DeclareKeyword);
			if (name === undefined || declared) {
				continue;
			}
			// An application interface's properties state a data contract, often one checked field by field.
			if (ts.isPropertySignature(member) && !copied(member.getSourceFile())) {
				continue;
			}
			addCandidate(`${owner.name}.${name}`, declarationsOf(declaration, declaration.name), owner.exported, owner);
		}
	};
	const collectStatements = (statements: readonly ts.Statement[], owner?: Candidate): void => {
		for (const statement of statements) {
			const exported = isExported(statement) || (owner?.exported ?? false);
			const prefix = owner ? `${owner.name}.` : '';
			// A re-export is a declaration of its own: a facade's name goes when nothing imports it.
			if (ts.isExportDeclaration(statement) && statement.exportClause && ts.isNamedExports(statement.exportClause)) {
				for (const specifier of statement.exportClause.elements) {
					addCandidate(specifier.name.text, [specifier], true, owner);
				}
				continue;
			}
			if (ts.isVariableStatement(statement)) {
				for (const declaration of statement.declarationList.declarations) {
					if (!ts.isIdentifier(declaration.name)) {
						continue;
					}
					const variable = addCandidate(prefix + declaration.name.text, [declaration], exported, owner);
					// A copied table such as `codiconsLibrary` loses the entries nothing names.
					const literal = declaration.initializer && objectLiteralOf(declaration.initializer);
					// With a declared type, its entries are read through that type instead.
					if (literal && !declaration.type && copied(statement.getSourceFile())) {
						for (const property of literal.properties.filter(ts.isPropertyAssignment)) {
							const name = nameText(property.name);
							if (name !== undefined) {
								addCandidate(`${variable.name}.${name}`, [property], exported, variable);
							}
						}
					}
				}
				continue;
			}
			const declaration = ts.isFunctionDeclaration(statement) || ts.isClassDeclaration(statement) || ts.isInterfaceDeclaration(statement)
				|| ts.isTypeAliasDeclaration(statement) || ts.isEnumDeclaration(statement) || ts.isModuleDeclaration(statement);
			if (!declaration || !statement.name || !ts.isIdentifier(statement.name) || statement.flags & ts.NodeFlags.GlobalAugmentation) {
				continue;
			}
			const candidate = addCandidate(prefix + statement.name.text, declarationsOf(statement, statement.name), exported, owner);
			if (ts.isClassDeclaration(statement) || ts.isInterfaceDeclaration(statement)) {
				collectMembers(candidate, statement.members);
			} else if (ts.isModuleDeclaration(statement) && statement.body && ts.isModuleBlock(statement.body)) {
				collectStatements(statement.body.statements, candidate);
			}
		}
	};
	for (const file of files) {
		if (file.fileName.startsWith(src) && !file.isDeclarationFile && !isTestCode(file)) {
			collectStatements(file.statements);
		}
	}

	// Members that one use keeps alive together: implemented, overridden and structurally satisfied ones.
	const group = new Map<Candidate, Set<Candidate>>();
	const join_ = (left: Candidate, right: Candidate): void => {
		const a = group.get(left) ?? new Set([left]);
		const b = group.get(right) ?? new Set([right]);
		if (a === b) {
			return;
		}
		for (const member of b) {
			a.add(member);
			group.set(member, a);
		}
		group.set(left, a);
	};
	const external = new Set<ts.Declaration>();
	const relatedDeclarations = (declaration: ts.Declaration): ts.Declaration[] => {
		const owner = declaration.parent;
		const name = nameText((declaration as ts.NamedDeclaration).name);
		if (name === undefined || !(ts.isClassLike(owner) || ts.isInterfaceDeclaration(owner))) {
			return [];
		}
		const related: ts.Declaration[] = [];
		const visit = (type: ts.Type, depth: number): void => {
			if (depth > 16) {
				return;
			}
			for (const constituent of type.isUnionOrIntersection() ? type.types : [type]) {
				const property = checker.getPropertyOfType(constituent, name);
				related.push(...(property?.declarations ?? []));
				for (const base of constituent.isClassOrInterface() ? checker.getBaseTypes(constituent) : []) {
					visit(base, depth + 1);
				}
			}
		};
		for (const clause of owner.heritageClauses ?? []) {
			for (const expression of clause.types) {
				visit(checker.getTypeAtLocation(expression), 0);
			}
		}
		return related;
	};
	for (const candidate of candidates) {
		if (!candidate.owner) {
			continue;
		}
		for (const declaration of candidate.declarations) {
			for (const related of relatedDeclarations(declaration)) {
				const other = byDeclaration.get(related);
				if (other) {
					join_(candidate, other);
				} else if (related !== declaration) {
					external.add(declaration);
				}
			}
		}
	}
	const uses: Use[] = [];
	const enclosing = (node: ts.Node): Candidate | undefined => {
		for (let current: ts.Node = node.parent; !ts.isSourceFile(current); current = current.parent) {
			const candidate = byDeclaration.get(current);
			if (candidate) {
				return candidate;
			}
		}
		return undefined;
	};
	const record = (symbol: ts.Symbol | undefined, node: ts.Node, isTest: boolean, file: string): void => {
		const targets: ts.Declaration[] = [];
		for (let current = symbol, depth = 0; current && depth < 8; depth++) {
			targets.push(...(current.declarations ?? []));
			if (!(current.flags & ts.SymbolFlags.Alias)) {
				break;
			}
			current = checker.getImmediateAliasedSymbol(current);
		}
		if (targets.length) {
			uses.push({ targets, within: enclosing(node), test: isTest, file });
		}
	};
	for (const file of files) {
		const isTest = isTestCode(file);
		const visit = (node: ts.Node): void => {
			if (ts.isIdentifier(node) && ts.isShorthandPropertyAssignment(node.parent) && node.parent.name === node) {
				// `{ name }` reads the variable `name`.
				record(checker.getShorthandAssignmentValueSymbol(node.parent), node, isTest, file.fileName);
			} else if ((ts.isIdentifier(node) || ts.isPrivateIdentifier(node)) && !isDeclarationName(node)) {
				record(checker.getSymbolAtLocation(node), node, isTest, file.fileName);
			} else if (ts.isIdentifier(node) && isShorthandBinding(node)) {
				// `const { name } = value` reads `value.name`.
				const property = checker.getPropertyOfType(checker.getTypeAtLocation(node.parent.parent), node.text);
				record(property, node, isTest, file.fileName);
			} else if (ts.isSpreadAssignment(node) && checker.getContextualType(node.parent)) {
				// Spread into a typed object, every property passes on to whoever reads that type.
				for (const property of checker.getTypeAtLocation(node.expression).getProperties()) {
					record(property, node, isTest, file.fileName);
				}
			} else if (ts.isObjectLiteralElementLike(node) && !ts.isSpreadAssignment(node) && ts.isObjectLiteralExpression(node.parent)) {
				// Writing a property of a contextual type passes it to whoever reads that type, often outside `src/`.
				const name = nameText(node.name);
				const contextual = checker.getContextualType(node.parent);
				if (name !== undefined && contextual) {
					for (const constituent of constituents(contextual)) {
						record(checker.getPropertyOfType(constituent, name), node, isTest, file.fileName);
					}
				}
			} else if (ts.isElementAccessExpression(node)) {
				// `value[key]` with `key` a union of names reads each of them; any other key may read any property.
				const objectType = checker.getTypeAtLocation(node.expression);
				const names = literalNames(checker.getTypeAtLocation(node.argumentExpression));
				const properties = names.length ? names.map(name => checker.getPropertyOfType(objectType, name)) : objectType.getProperties();
				for (const property of properties) {
					record(property, node, isTest, file.fileName);
				}
			} else if (ts.isCallExpression(node) || ts.isNewExpression(node)) {
				// An object handed to a function, as to `Object.values`, may have any of its properties read.
				for (const argument of node.arguments ?? []) {
					if (ts.isIdentifier(argument) || ts.isPropertyAccessExpression(argument)) {
						const type = checker.getTypeAtLocation(argument);
						if (type.getSymbol()?.flags !== undefined && (type.getSymbol()?.flags ?? 0) & ts.SymbolFlags.ObjectLiteral) {
							for (const property of type.getProperties()) {
								record(property, node, isTest, file.fileName);
							}
						}
					}
				}
			} else if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.InKeyword && ts.isStringLiteral(node.left)) {
				const objectType = checker.getTypeAtLocation(node.right);
				for (const constituent of objectType.isUnion() ? objectType.types : [objectType]) {
					record(checker.getPropertyOfType(constituent, node.left.text), node, isTest, file.fileName);
				}
			} else if (ts.isTypeOperatorNode(node) && node.operator === ts.SyntaxKind.KeyOfKeyword) {
				// `keyof T` and mapped types over it reach every property of `T`.
				for (const property of checker.getTypeAtLocation(node.type).getProperties()) {
					record(property, node, isTest, file.fileName);
				}
			} else if ((ts.isTypeReferenceNode(node) || ts.isExpressionWithTypeArguments(node)) && node.typeArguments && node.typeArguments.length >= 2) {
				// `Pick<T, 'a' | 'b'>` and `Omit<T, 'a'>` name properties of `T` by string.
				const objectType = checker.getTypeAtLocation(node.typeArguments[0]);
				for (const name of literalNames(checker.getTypeAtLocation(node.typeArguments[1]))) {
					record(checker.getPropertyOfType(objectType, name), node, isTest, file.fileName);
				}
			}
			ts.forEachChild(node, visit);
		};
		visit(file);
	}

	// Structural satisfaction: a class member is kept by a used member of the same name in an
	// interface or type literal the class is assignable to (`dispose` through `IDisposable`).
	const usedTypeMembers = new Map<string, Set<ts.Node>>();
	for (const use of uses) {
		for (const target of use.targets) {
			const parent = target.parent;
			const name = nameText((target as ts.NamedDeclaration).name);
			if (name !== undefined && (ts.isInterfaceDeclaration(parent) || ts.isTypeLiteralNode(parent))) {
				const set = usedTypeMembers.get(name) ?? new Set();
				set.add(parent);
				usedTypeMembers.set(name, set);
			}
		}
	}
	const structural = new Set<Candidate>();
	for (const candidate of candidates) {
		const declaration = candidate.declarations[0];
		const owner = declaration.parent;
		const name = nameText((declaration as ts.NamedDeclaration).name);
		const types = name === undefined ? undefined : usedTypeMembers.get(name);
		if (!types || !ts.isClassLike(owner)) {
			continue;
		}
		const symbol = owner.name ? checker.getSymbolAtLocation(owner.name) : undefined;
		if (!symbol) {
			continue;
		}
		const modifiers = ts.canHaveModifiers(declaration) ? ts.getModifiers(declaration) ?? [] : [];
		const isStatic = modifiers.some(modifier => modifier.kind === ts.SyntaxKind.StaticKeyword);
		const classType = isStatic ? checker.getTypeOfSymbolAtLocation(symbol, owner) : checker.getDeclaredTypeOfSymbol(symbol);
		for (const type of types) {
			const interfaceSymbol = ts.isInterfaceDeclaration(type) ? checker.getSymbolAtLocation(type.name) : undefined;
			const targetType = interfaceSymbol ? checker.getDeclaredTypeOfSymbol(interfaceSymbol) : checker.getTypeAtLocation(type);
			// A generic interface is satisfied when the class has all its required members.
			const generic = ts.isInterfaceDeclaration(type) && (type.typeParameters?.length ?? 0) > 0;
			const required = targetType.getProperties().filter(property => !(property.flags & ts.SymbolFlags.Optional));
			const hasRequired = required.length > 0 && required.every(property => checker.getPropertyOfType(classType, property.name));
			if (checker.isTypeAssignableTo(classType, targetType) || (generic && hasRequired)) {
				structural.add(candidate);
				break;
			}
		}
	}
	// Members the type system reads although no code does: a property typed by its owner's type
	// parameter carries it for inference (`ServiceIdentifier<T>.type`), and a literal-typed one
	// discriminates a union (`lazy: true`). They live as long as their owner.
	const typeLevel = new Map<Candidate, Candidate[]>();
	for (const candidate of candidates) {
		const declaration = candidate.declarations[0];
		const owner = declaration.parent;
		if (!candidate.owner || !(ts.isPropertySignature(declaration) || ts.isPropertyDeclaration(declaration)) || !declaration.type) {
			continue;
		}
		const ownParameters = ts.isInterfaceDeclaration(owner) || ts.isClassLike(owner) ? owner.typeParameters ?? [] : [];
		const parameters = new Set(ownParameters.map(parameter => parameter.name.text));
		let typeLevelMember = ts.isLiteralTypeNode(declaration.type);
		const visit = (node: ts.Node): void => {
			if (ts.isTypeReferenceNode(node) && ts.isIdentifier(node.typeName) && parameters.has(node.typeName.text)) {
				typeLevelMember = true;
			}
			ts.forEachChild(node, visit);
		};
		visit(declaration.type);
		if (typeLevelMember) {
			typeLevel.set(candidate.owner, [...(typeLevel.get(candidate.owner) ?? []), candidate]);
		}
	}
	const allowedCandidate = (candidate: Candidate): boolean => allowed.has(`${candidate.path}#${candidate.name}`);
	const ignoredMember = (candidate: Candidate): boolean => {
		const name = candidate.name.slice(candidate.name.lastIndexOf('.') + 1);
		// `_serviceBrand` types a service identifier; dependency injection and its lint rule need it unread.
		const implicit = name === 'toJSON' || name === 'toString' || name === '_serviceBrand';
		return candidate.owner !== undefined && (implicit || candidate.declarations.some(declaration => external.has(declaration)));
	};
	// A declaration that names itself from inside, as a class constructing itself, is not used by that.
	const encloses = (candidate: Candidate, within: Candidate | undefined): boolean => {
		for (let current = within; current; current = current.owner) {
			if (current === candidate) {
				return true;
			}
		}
		return false;
	};
	const mark = (includeTests: boolean): Set<Candidate> => {
		const live = new Set<Candidate>();
		const pending: Candidate[] = [];
		const activate = (candidate: Candidate): void => {
			for (const member of group.get(candidate) ?? [candidate]) {
				if (!live.has(member)) {
					live.add(member);
					pending.push(member);
					(typeLevel.get(member) ?? []).forEach(activate);
				}
			}
		};
		for (const candidate of candidates) {
			if (structural.has(candidate) || ignoredMember(candidate) || allowedCandidate(candidate)) {
				activate(candidate);
			}
		}
		const waiting = new Map<Candidate, Use[]>();
		const apply = (use: Use): void => {
			for (const target of use.targets) {
				const candidate = byDeclaration.get(target);
				if (candidate && !encloses(candidate, use.within)) {
					activate(candidate);
					// A member used from outside keeps its owner, the class or namespace holding it.
					for (let owner = candidate.owner; owner && !encloses(owner, use.within); owner = owner.owner) {
						activate(owner);
					}
				}
			}
		};
		for (const use of uses) {
			if (use.test && !includeTests) {
				continue;
			}
			if (!use.within) {
				apply(use);
			} else {
				const list = waiting.get(use.within) ?? [];
				list.push(use);
				waiting.set(use.within, list);
			}
		}
		for (let candidate = pending.pop(); candidate; candidate = pending.pop()) {
			for (const use of waiting.get(candidate) ?? []) {
				apply(use);
			}
		}
		return live;
	};
	const live = mark(true);
	const liveWithoutTests = mark(false);
	const dead = candidates.filter(candidate => !live.has(candidate));
	const deadSet = new Set(dead);
	// Report the outermost dead declaration only; its members go with it.
	const outermost = dead.filter(candidate => !candidate.owner || !deadSet.has(candidate.owner));
	const ownerLive = (candidate: Candidate): boolean => !candidate.owner || liveWithoutTests.has(candidate.owner);
	// An allowed owner's entry also covers the members tests use: without the entry the owner alone is reported.
	const ownerAllowed = (candidate: Candidate): boolean => {
		for (let owner = candidate.owner; owner; owner = owner.owner) {
			if (allowedCandidate(owner)) {
				return true;
			}
		}
		return false;
	};
	const testOnly = candidates.filter(candidate => live.has(candidate) && !liveWithoutTests.has(candidate)
		&& ownerLive(candidate) && !ownerAllowed(candidate));
	const unused = [...outermost, ...testOnly]
		.map(candidate => ({ name: candidate.name, path: candidate.path, line: candidate.line, testOnly: live.has(candidate) }));
	const usedOutside = new Set<Candidate>();
	for (const use of uses) {
		for (const target of use.targets) {
			const candidate = byDeclaration.get(target);
			if (candidate && candidate.declarations[0].getSourceFile().fileName !== use.file) {
				usedOutside.add(candidate);
			}
		}
	}
	const localExports = candidates
		.filter(candidate => candidate.exported && !candidate.owner && live.has(candidate) && !usedOutside.has(candidate))
		.map(candidate => ({ name: candidate.name, path: candidate.path, line: candidate.line, testOnly: false }));
	const order = (left: UnusedDeclaration, right: UnusedDeclaration): number => left.path.localeCompare(right.path, 'en') || left.line - right.line;
	return { unused: unused.sort(order), localExports: localExports.sort(order) };
}

interface AllowlistEntry {
	readonly path: string;
	readonly name: string;
	readonly reason: string;
}

export const allowlistPath = 'build/lint/unused-allowlist.json';

/** The `check:unused` task: fails on an unused declaration or a stale allowlist entry. */
export async function checkUnused(root: string, args: readonly string[] = []): Promise<void> {
	const allowlist = JSON.parse(await readFile(join(root, allowlistPath), 'utf8')) as AllowlistEntry[];
	const allowed = new Set(allowlist.map(entry => `${entry.path}#${entry.name}`));
	const report = await findUnused(root, allowed);
	if (args.includes('--local-exports')) {
		for (const entry of report.localExports) {
			console.log(`${entry.path}:${String(entry.line)} ${entry.name}`);
		}
		return;
	}
	const strict = allowlist.length ? await findUnused(root) : report;
	const stillNeeded = new Set(strict.unused.map(entry => `${entry.path}#${entry.name}`));
	const stale = allowlist.filter(entry => !stillNeeded.has(`${entry.path}#${entry.name}`));
	const problems = [
		...report.unused.map(entry => `${entry.path}:${String(entry.line)} ${entry.name} is ${entry.testOnly ? 'used only by tests' : 'unused'}`),
		...stale.map(entry => `${allowlistPath}: ${entry.path}#${entry.name} is used now; remove the entry`),
	];
	if (problems.length) {
		throw new Error(`${String(problems.length)} unused declarations:\n${problems.join('\n')}`);
	}
	console.log('No unused declarations.');
}
