import { chromium } from "playwright";
const b = await chromium.connectOverCDP(`http://127.0.0.1:${process.argv[2]}`);
const page = b.contexts().flatMap(c => c.pages()).find(p => p.url().includes("muse.ai"));
const [dl] = await Promise.all([
  page.waitForEvent("download", { timeout: 20000 }),
  page.getByRole("button", { name: "Download" }).first().click(),
]);
const out = `output/${dl.suggestedFilename()}`;
await dl.saveAs(out);
console.log("saved", out);
await b.close();
