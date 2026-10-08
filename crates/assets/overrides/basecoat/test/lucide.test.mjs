import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { buildLucideCatalog } from "../build-lucide.mjs";

test("the published catalog contains every canonical icon once, with safe geometric SVG", async () => {
  const nodes = JSON.parse(await readFile(new URL("../node_modules/lucide-static/icon-nodes.json", import.meta.url)));
  const catalog = buildLucideCatalog(nodes);
  assert.equal(catalog.length, 1869);
  assert.deepEqual(catalog.map(icon => icon.name), Object.keys(nodes).sort());
  assert.ok(catalog.some(icon => icon.name === "house"));
  assert.ok(!catalog.some(icon => icon.name === "home"), "legacy aliases should not duplicate picker choices");
  for (const icon of catalog) {
    assert.ok(icon.label.length > 0);
    assert.match(icon.svg, /^<svg [^>]+>.+<\/svg>$/);
    assert.match(icon.svg, /stroke="currentColor"/);
    assert.doesNotMatch(icon.svg, /<(?:script|style|image|use|foreignObject)|\bon\w+=|href=|url\(/i);
  }
});

test("unexpected package nodes, attributes and markup values fail the build", () => {
  const inputs = [
    { bad: [["script", { d: "M0 0" }]] },
    { bad: [["path", { d: "M0 0", onclick: "alert(1)" }]] },
    { bad: [["path", { d: '\"><script>alert(1)</script>' }]] },
    { bad: [["circle", { fill: "url(https://example.com/icon.svg)" }]] },
    { bad: [["use", { href: "https://example.com/icon.svg" }]] },
    { 'bad"name': [["path", { d: "M0 0" }]] },
  ];
  for (const nodes of inputs) assert.throws(() => buildLucideCatalog(nodes), /Lucide/);
});
