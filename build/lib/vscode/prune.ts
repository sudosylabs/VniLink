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
 * A name is a top-level declaration (`delta`), a member of a top-level class,
 * interface or namespace (`MenuId.CommandPalette`), or an `export { name }`
 * specifier. A declaration goes with the comments attached to it (no blank line
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
	if (ts.isClassDeclaration(node) || ts.isInterfaceDeclaration(node)) {
		return node.members;
	}
	if (ts.isModuleDeclaration(node) && node.body && ts.isModuleBlock(node.body)) {
		return node.body.statements;
	}
	return undefined;
}

/** The lines `node` occupies, with its attached comments and the rest of its last line. */
function lineRange(text: string, node: ts.Node): Range {
	const trivia = text.slice(node.getFullStart(), node.getStart());
	let start: number;
	const gap = trivia.search(/\n[ \t]*\n(?![\s\S]*\n[ \t]*\n)/);
	if (gap >= 0) {
		start = node.getFullStart() + trivia.indexOf('\n', gap + 1) + 1;
	} else if (trivia.includes('\n')) {
		start = node.getFullStart() + trivia.indexOf('\n') + 1;
	} else {
		start = node.getStart();
	}
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

/** Identifiers outside import declarations. */
function referencedNames(source: ts.SourceFile): Set<string> {
	const names = new Set<string>();
	const visit = (node: ts.Node): void => {
		if (ts.isImportDeclaration(node) || ts.isImportEqualsDeclaration(node)) {
			return;
		}
		if (ts.isIdentifier(node)) {
			names.add(node.text);
		}
		ts.forEachChild(node, visit);
	};
	ts.forEachChild(source, visit);
	// JSDoc `{@link name}` references keep an import alive in upstream's own text.
	for (const match of source.text.matchAll(/@link\s+([A-Za-z_$][\w$]*)/g)) {
		names.add(match[1]);
	}
	return names;
}

interface ImportBinding {
	readonly name: string;
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
			bindings.push({ name: clause.name.text, declaration: statement });
		}
		const named = clause.namedBindings;
		if (named && ts.isNamespaceImport(named)) {
			bindings.push({ name: named.name.text, declaration: statement });
		} else if (named) {
			for (const specifier of named.elements) {
				bindings.push({ name: specifier.name.text, declaration: statement, specifier });
			}
		}
	}
	return bindings;
}

function unusedImports(source: ts.SourceFile): Set<string> {
	const referenced = referencedNames(source);
	return new Set(importBindings(source).map(binding => binding.name).filter(name => !referenced.has(name)));
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
		for (const specifier of removed.map(binding => binding.specifier).filter(specifier => specifier !== undefined)) {
			const index = elements.indexOf(specifier);
			if (index < elements.length - 1) {
				ranges.push({ start: elements[index].getStart(), end: elements[index + 1].getStart() });
			} else {
				ranges.push({ start: elements[index - 1].getEnd(), end: elements[index].getEnd() });
			}
		}
	}
	return removeRanges(text, mergeRanges(ranges));
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

function exportSpecifierRange(specifier: ts.ExportSpecifier, text: string): Range {
	const declaration = specifier.parent.parent;
	const elements = specifier.parent.elements;
	if (elements.length === 1) {
		return lineRange(text, declaration);
	}
	const index = elements.indexOf(specifier);
	return index < elements.length - 1
		? { start: specifier.getStart(), end: elements[index + 1].getStart() }
		: { start: elements[index - 1].getEnd(), end: specifier.getEnd() };
}

/** `content` without the declarations in `names`; throws for a name it cannot find. */
export function pruneSource(path: string, content: string, names: readonly string[]): string {
	const source = ts.createSourceFile(path, content, ts.ScriptTarget.Latest, true);
	const wanted = new Set(names);
	const found = new Set<string>();
	const ranges: Range[] = [];
	const remove = (node: ts.Node, name: string): void => {
		ranges.push(lineRange(content, node));
		found.add(name);
	};
	for (const statement of source.statements) {
		const name = declarationName(statement);
		if (name !== undefined && wanted.has(name)) {
			remove(statement, name);
			continue;
		}
		if (ts.isExportDeclaration(statement) && statement.exportClause && ts.isNamedExports(statement.exportClause)) {
			const removedHere = statement.exportClause.elements.filter(element => wanted.has(element.name.text));
			if (removedHere.length === statement.exportClause.elements.length) {
				remove(statement, removedHere[0].name.text);
				removedHere.forEach(element => found.add(element.name.text));
			} else {
				for (const element of removedHere) {
					ranges.push(exportSpecifierRange(element, content));
					found.add(element.name.text);
				}
			}
			continue;
		}
		if (name === undefined) {
			continue;
		}
		for (const member of membersOf(statement) ?? []) {
			const qualified = `${name}.${memberName(member) ?? ''}`;
			if (wanted.has(qualified)) {
				remove(member, qualified);
			}
		}
	}
	const missing = names.filter(name => !found.has(name));
	if (missing.length) {
		throw new Error(`${path}: no declaration named ${missing.join(', ')}`);
	}
	const before = unusedImports(source);
	// Neighbouring declarations go as one block, so the blank lines around it are judged once.
	const pruned = removeRanges(content, mergeRanges(ranges).map(range => withBlankLine(content, range)));
	const after = unusedImports(ts.createSourceFile(path, pruned, ts.ScriptTarget.Latest, true));
	const orphaned = new Set([...after].filter(name => !before.has(name)));
	return orphaned.size ? removeImports(pruned, orphaned) : pruned;
}
