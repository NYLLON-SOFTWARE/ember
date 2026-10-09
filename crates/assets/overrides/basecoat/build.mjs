import { build } from "esbuild";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { buildLucideCatalog } from "./build-lucide.mjs";

const root = fileURLToPath(new URL(".", import.meta.url));
const checking = process.argv.includes("--check");
const temporary = await mkdtemp(join(tmpdir(), "ember-basecoat-"));
const license = `/*!\n${await readFile(join(root, "LICENSES.txt"), "utf8")}\n*/\n`;
const generated = new Map();

try {
  execFileSync(process.execPath, [
    join(root, "node_modules/@tailwindcss/cli/dist/index.mjs"),
    "--input", "src/theme.css", "--output", join(temporary, "app.css"), "--minify",
  ], { cwd: root, stdio: "inherit" });
  generated.set("app.css", license + await readFile(join(temporary, "app.css"), "utf8"));
  for (const entry of ["app", "theme-init", "workspace-appearance"]) {
    const output = await build({
      absWorkingDir: root,
      entryPoints: [`src/${entry}.js`],
      bundle: true,
      minify: true,
      format: "iife",
      target: "es2022",
      write: false,
      // Turbo retains head scripts. Also tolerate reexecution without reinstalling listeners
      // or replacing Basecoat's registry while live components still use it.
      banner: { js: entry === "app" ? `${license}(()=>{if(window.__emberBasecoatLoaded)return;window.__emberBasecoatLoaded=true;` : "" },
      footer: { js: entry === "app" ? "})();" : "" },
    });
    generated.set(entry === "workspace-appearance" ? "../ember/appearance.js" : `${entry}.js`, output.outputFiles[0].text);
  }

  const lucide = join(root, "node_modules/lucide-static");
  const nodes = JSON.parse(await readFile(join(lucide, "icon-nodes.json"), "utf8"));
  generated.set("../lucide/catalog.json", JSON.stringify(buildLucideCatalog(nodes)) + "\n");
  generated.set("../lucide/LICENSE.txt", await readFile(join(lucide, "LICENSE"), "utf8"));

  for (const [name, output] of generated) {
    const path = join(root, name);
    if (checking) {
      const current = await readFile(path, "utf8").catch(() => "");
      if (current !== output) throw new Error(`${name} is stale; run npm run build in ${root}`);
    } else {
      await mkdir(dirname(path), { recursive: true });
      await writeFile(path, output);
    }
    console.log(`${name}: ${Buffer.byteLength(output)} bytes${checking ? " (verified)" : ""}`);
  }
} finally {
  await rm(temporary, { recursive: true, force: true });
}
