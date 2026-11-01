// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Deletes whole declarations from copied source and changes nothing else, so a
 * pruned copy can be re-derived from upstream and compared byte for byte.
 *
 * A name is a declaration (`delta`), one inside a namespace (`Event.map`), a
 * member of a class or interface (`MenuId.CommandPalette`,
 * `Event.ChainableSynthesis.reduce`), or an `export { name }` specifier. A declaration goes with the comments attached to it (no blank line
 * between) and one of the blank lines around it. Import specifiers that only
 * the deleted code used go too; imports upstream already left unused stay.
 */
import ts from 'typescript';

interface Range {
	readonly start: number;
	readonly end: number;
}

function declarationName(node: ts.Node): string | undefined {
	const named = ts.isFunctionDeclaration(node) || ts.isClassDeclaration(node) || ts.isInterfaceDeclaration(node)
		|| ts.isTypeAliasDeclaration(node) || ts.isEnumDeclaration(node) || ts.isModuleDeclaration(node);
	if (named && node.name) {
		return node.name.text;
	}
	if (ts.isVariableStatement(node) && node.declarationList.declarations.length === 1) {
		const name = node.declarationList.declarations[0].name;
		return ts.isIdentifier(name) ? name.text : undefined;
	}
	return undefined;
}

function memberName(member: ts.Node): string | undefined {
	if (ts.isClassElement(member) || ts.isTypeElement(member)) {
		const name = member.name;
		if (name && (ts.isIdentifier(name) || ts.isPrivateIdentifier(name) || ts.isStringLiteral(name))) {
			return name.text;
		}
		return undefined;
	}
	return declarationName(member);
}

function membersOf(node: ts.Node): readonly ts.Node[] | undefined {
	return ts.isClassDeclaration(node) || ts.isInterfaceDeclaration(node) ? node.members : undefined;
}

/** The lines `node` occupies, with the comments attached to it and the rest of its last line. */
function lineRange(text: string, node: ts.Node): Range {
	// Comments directly above, with no blank line between, belong to the node; a file's notice never does.
	let first = node.getStart();
	const comments = ts.getLeadingCommentRanges(text, node.getFullStart()) ?? [];
	for (let index = comments.length - 1; index >= 0; index--) {
		const comment = comments[index];
		const between = text.slice(comment.end, first);
		const notice = comment.pos === 0 && /^(\/\*-{3}|\/\/ -{3})/.test(text);
		if (notice || /\n[ \t]*\n/.test(between)) {
			break;
		}
		first = comment.pos;
	}
	const lineStart = text.lastIndexOf('\n', first - 1) + 1;
	const start = /^[ \t]*$/.test(text.slice(lineStart, first)) ? lineStart : first;
	let end = node.getEnd();
	const rest = /^[ \t]*(\/\/[^\n]*)?(\r?\n|$)/.exec(text.slice(end));
	if (rest) {
		end += rest[0].length;
	}
	return { start, end };
}

/**
 * Widens a range of whole lines over one blank line when removing it would
 * leave two blank lines together, or one against the edge of a block or file.
 */
function withBlankLine(text: string, range: Range): Range {
	const head = text.slice(0, range.start);
	const tail = text.slice(range.end);
	const blankBefore = /\n[ \t]*\n$/.exec(head);
	const blankAfter = /^[ \t]*\r?\n/.exec(tail);
	const opensBlock = range.start === 0 || /[{[(][ \t]*\r?\n$/.test(head);
	const closesBlock = tail.length === 0 || /^[ \t]*[}\])]/.test(tail);
	if (blankAfter && (blankBefore || opensBlock)) {
		return { start: range.start, end: range.end + blankAfter[0].length };
	}
	if (blankBefore && closesBlock) {
		return { start: range.start - (blankBefore[0].length - 1), end: range.end };
	}
	return range;
}

function removeRanges(text: string, ranges: readonly Range[]): string {
	let result = text;
	for (const range of [...ranges].sort((left, right) => right.start - left.start)) {
		result = result.slice(0, range.start) + result.slice(range.end);
	}
	return result;
}

/**
 * Import bindings that nothing in `source` refers to. Names are resolved by
 * scope, so a parameter or a member that shares an import's name does not keep
 * it; a JSDoc `{@link name}` does, as in upstream's own text.
 */
function unreferencedImports(source: ts.SourceFile): Set<string> {
	const host = ts.createCompilerHost({});
	const program = ts.createProgram({
		rootNames: [source.fileName],
		options: { noResolve: true, noLib: true, allowJs: true, types: [] },
		host: {
			...host,
			getSourceFile: name => name === source.fileName ? source : undefined,
			fileExists: name => name === source.fileName,
			readFile: () => undefined,
		},
	});
	const checker = program.getTypeChecker();
	const bindings = new Map<ts.Symbol, string>();
	for (const binding of importBindings(source)) {
		const symbol = checker.getSymbolAtLocation(binding.identifier);
		if (symbol) {
			bindings.set(symbol, binding.name);
		}
	}
	const used = new Set<string>();
	const visit = (node: ts.Node): void => {
		if (ts.isImportDeclaration(node)) {
			return;
		}
		if (ts.isIdentifier(node)) {
			const symbol = checker.getSymbolAtLocation(node);
			const name = symbol && bindings.get(symbol);
			if (name !== undefined) {
				used.add(name);
			}
		}
		ts.forEachChild(node, visit);
	};
	ts.forEachChild(source, visit);
	for (const match of source.text.matchAll(/@link\s+([A-Za-z_$][\w$]*)/g)) {
		used.add(match[1]);
	}
	return new Set([...bindings.values()].filter(name => !used.has(name)));
}

interface ImportBinding {
	readonly name: string;
	readonly identifier: ts.Identifier;
	readonly declaration: ts.ImportDeclaration;
	readonly specifier?: ts.ImportSpecifier;
}

function importBindings(source: ts.SourceFile): ImportBinding[] {
	const bindings: ImportBinding[] = [];
	for (const statement of source.statements) {
		if (!ts.isImportDeclaration(statement) || !statement.importClause) {
			continue;
		}
		const clause = statement.importClause;
		if (clause.name) {
			bindings.push({ name: clause.name.text, identifier: clause.name, declaration: statement });
		}
		const named = clause.namedBindings;
		if (named && ts.isNamespaceImport(named)) {
			bindings.push({ name: named.name.text, identifier: named.name, declaration: statement });
		} else if (named) {
			for (const specifier of named.elements) {
				bindings.push({ name: specifier.name.text, identifier: specifier.name, declaration: statement, specifier });
			}
		}
	}
	return bindings;
}


/** Removes the import bindings named in `names`, whole declarations when none is left. */
function removeImports(text: string, names: ReadonlySet<string>): string {
	const source = ts.createSourceFile('pruned.ts', text, ts.ScriptTarget.Latest, true);
	const ranges: Range[] = [];
	const byDeclaration = new Map<ts.ImportDeclaration, ImportBinding[]>();
	for (const binding of importBindings(source)) {
		const list = byDeclaration.get(binding.declaration) ?? [];
		list.push(binding);
		byDeclaration.set(binding.declaration, list);
	}
	for (const [declaration, bindings] of byDeclaration) {
		const removed = bindings.filter(binding => names.has(binding.name));
		if (!removed.length) {
			continue;
		}
		if (removed.length === bindings.length) {
			ranges.push(lineRange(text, declaration));
			continue;
		}
		const named = declaration.importClause?.namedBindings;
		if (!named || !ts.isNamedImports(named) || removed.some(binding => !binding.specifier)) {
			throw new Error(`Cannot remove only part of the import at ${String(source.getLineAndCharacterOfPosition(declaration.getStart()).line + 1)}`);
		}
		const elements = named.elements;
		const keep = elements.filter(element => !names.has(element.name.text));
		if (!keep.length) {
			ranges.push(lineRange(text, declaration));
			continue;
		}
		ranges.push(...listElementRanges(elements, removed.map(binding => binding.specifier).filter(specifier => specifier !== undefined)));
	}
	return removeRanges(text, mergeRanges(ranges));
}

/**
 * The text that removes `removed` from a comma-separated list with at least one
 * element left: before the last kept element an element goes with the separator
 * after it, past it with the one before. A trailing comma stays.
 */
function listElementRanges(elements: readonly ts.Node[], removed: readonly ts.Node[]): Range[] {
	const kept = elements.filter(element => !removed.includes(element));
	const lastKept = elements.indexOf(kept[kept.length - 1]);
	return removed.map(element => {
		const index = elements.indexOf(element);
		return index < lastKept
			? { start: elements[index].getStart(), end: elements[index + 1].getStart() }
			: { start: elements[index - 1].getEnd(), end: elements[index].getEnd() };
	});
}

/** Joins overlapping ranges, which adjacent specifiers removed together produce. */
function mergeRanges(ranges: readonly Range[]): Range[] {
	const sorted = [...ranges].sort((left, right) => left.start - right.start);
	const merged: Range[] = [];
	for (const range of sorted) {
		const last = merged.at(-1);
		if (last && range.start <= last.end) {
			merged[merged.length - 1] = { start: last.start, end: Math.max(last.end, range.end) };
		} else {
			merged.push(range);
		}
	}
	return merged;
}

/** `content` without the declarations in `names`; throws for a name it cannot find. */
export function pruneSource(path: string, content: string, names: readonly string[]): string {
	const source = ts.createSourceFile(path, content, ts.ScriptTarget.Latest, true);
	const wanted = new Set(names);
	const found = new Set<string>();
	const ranges: Range[] = [];
	const remove = (node: ts.Node, name: string): void => {
		ranges.push(lineRange(content, node));
		// Whatever was declared inside goes with it.
		for (const other of wanted) {
			if (other === name || other.startsWith(`${name}.`)) {
				found.add(other);
			}
		}
	};
	const visit = (statements: readonly ts.Statement[], prefix: string): void => {
		for (const statement of statements) {
			const local = declarationName(statement);
			const name = local === undefined ? undefined : prefix + local;
			if (name !== undefined && wanted.has(name)) {
				remove(statement, name);
				continue;
			}
			if (ts.isVariableStatement(statement) && statement.declarationList.declarations.length > 1) {
				const declarations = statement.declarationList.declarations;
				const removedHere = declarations.filter(declaration => ts.isIdentifier(declaration.name) && wanted.has(prefix + declaration.name.text));
				removedHere.forEach(declaration => found.add(prefix + (declaration.name as ts.Identifier).text));
				if (removedHere.length === declarations.length) {
					ranges.push(lineRange(content, statement));
				} else {
					ranges.push(...listElementRanges(declarations, removedHere));
				}
				continue;
			}
			if (ts.isExportDeclaration(statement) && statement.exportClause && ts.isNamedExports(statement.exportClause)) {
				const elements = statement.exportClause.elements;
				const removedHere = elements.filter(element => wanted.has(prefix + element.name.text));
				if (removedHere.length && removedHere.length === elements.length) {
					ranges.push(lineRange(content, statement));
				} else {
					ranges.push(...listElementRanges(elements, removedHere));
				}
				removedHere.forEach(element => found.add(prefix + element.name.text));
				continue;
			}
			if (name === undefined) {
				continue;
			}
			if (ts.isModuleDeclaration(statement) && statement.body && ts.isModuleBlock(statement.body)) {
				visit(statement.body.statements, `${name}.`);
				continue;
			}
			for (const member of membersOf(statement) ?? []) {
				const qualified = `${name}.${memberName(member) ?? ''}`;
				if (wanted.has(qualified)) {
					remove(member, qualified);
				}
			}
		}
	};
	visit(source.statements, '');
	const missing = names.filter(name => !found.has(name));
	if (missing.length) {
		throw new Error(`${path}: no declaration named ${missing.join(', ')}`);
	}
	const before = unreferencedImports(source);
	// Neighbouring declarations go as one block, so the blank lines around it are judged once.
	// Widening can join neighbours separated by a blank line; widen the joined block again until it settles.
	let blocks = mergeRanges(ranges);
	let joined: boolean;
	do {
		const widened = mergeRanges(blocks.map(range => withBlankLine(content, range)));
		joined = widened.length < blocks.length;
		blocks = widened;
	} while (joined);
	const pruned = removeRanges(content, blocks);
	const after = unreferencedImports(ts.createSourceFile(path, pruned, ts.ScriptTarget.Latest, true));
	const orphaned = new Set([...after].filter(name => !before.has(name)));
	return orphaned.size ? removeImports(pruned, orphaned) : pruned;
}
