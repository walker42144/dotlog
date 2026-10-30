import { readFile, writeFile, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve, relative, isAbsolute, basename } from "node:path";
import { pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import { after, test } from "node:test";
import assert from "node:assert/strict";
import ts from "typescript";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

// Exercise the production component with React's renderer, without a separate
// markdown implementation or a browser that could execute untrusted markup.
const require = createRequire(import.meta.url);
const dir = await mkdtemp(join(tmpdir(), "dotlog-memo-test-"));
const relativeDir = relative(resolve(tmpdir()), resolve(dir));
assert.ok(relativeDir && !relativeDir.startsWith("..") && !isAbsolute(relativeDir)
  && basename(dir).startsWith("dotlog-memo-test-"));
after(() => rm(dir, { recursive: true, force: true }));
const source = (await readFile(new URL("../src/components/MemoContent.tsx", import.meta.url), "utf8"))
  .replace(/^import "[^"\n]+\.css";\r?\n/gm, "");
const js = ts.transpileModule(source, { compilerOptions: {
  jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022
} }).outputText.replace(/from "([^".][^"]*)"/g, (_, specifier) =>
  `from "${pathToFileURL(require.resolve(specifier)).href}"`);
const compiled = join(dir, "memo-content.mjs");
await writeFile(compiled, js);
const { MemoContent, formatMemoTime } = await import(pathToFileURL(compiled).href);
const render = (content) => renderToStaticMarkup(createElement(MemoContent, { content }));

test("headings, bold, quotes, lists and tables render as elements", () => {
  const html = render("# 标题\n\n> 引用\n\n- **重点**\n\n| 内容 | 状态 |\n| --- | --- |\n| 公式 | 完成 |");
  for (const tag of ["h1", "blockquote", "ul", "strong", "table"]) assert.match(html, new RegExp(`<${tag}[ >]`));
});
test("internal comments are hidden and HTML or javascript links cannot execute", () => {
  const html = render('**记录**\n\n<!-- dotlog:segment=secret-marker -->\n\n<script>alert(1)</script>\n\n[危险](javascript:alert(1))\n\n![附件](https://remote.test/private.png)');
  for (const text of ["secret-marker", "<script", "javascript:", "<img", "remote.test"]) assert.ok(!html.includes(text), text);
  assert.match(html, /附件/);
});
test("math uses local KaTeX markup", () => {
  const html = render("$$\nA=\\frac{1}{2}\\int r^2\\,d\\theta\n$$");
  assert.match(html, /class="katex"/);
  assert.match(html, /<math/);
});
test("ISO and legacy epoch timestamps format equally, malformed dates stay readable", () => {
  assert.equal(formatMemoTime("2026-10-29T10:11:58Z"), formatMemoTime("1793268718"));
  assert.equal(formatMemoTime(""), "时间未知");
  assert.equal(formatMemoTime("not-a-date"), "时间未知");
});
