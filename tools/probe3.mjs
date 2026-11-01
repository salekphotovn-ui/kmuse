import { chromium } from "playwright";
import fs from "node:fs";
const b = await chromium.connectOverCDP(`http://127.0.0.1:${process.argv[2]}`);
const page = b.contexts().flatMap(c => c.pages()).find(p => p.url().includes("muse.ai"));
await page.keyboard.press("Escape");
const fn = fs.readFileSync("src-tauri/src/extract.js","utf8");
for (const kind of ["image","video"]) {
  const r = await page.evaluate(`${fn}(${JSON.stringify(kind)})`);
  console.log(kind, r.error || r.type, r.label, r.b64?.length, r.html);
}
await b.close();
