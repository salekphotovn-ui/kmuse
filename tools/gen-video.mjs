import { chromium } from "playwright";
const b = await chromium.connectOverCDP(`http://127.0.0.1:${process.argv[2]}`);
const page = b.contexts().flatMap(c => c.pages()).find(p => p.url().includes("muse.ai"));
const close = page.getByRole("button", { name: "Close", exact: true });
if (await close.count()) await close.first().click().catch(()=>{});
const sel = '[aria-label^="Open animated video"]';
const before = await page.locator(sel).count();
console.log("videos before", before);
const box = page.locator('textarea[aria-label="Message"]');
await box.fill("Video: animate a red apple on a white background with a slow camera push-in, 16:9, 10 seconds. No text.");
await box.press("Enter");
const t0 = Date.now();
while (Date.now() - t0 < 420000) {
  await page.waitForTimeout(5000);
  if (await page.locator(sel).count() > before) break;
}
console.log("videos after", await page.locator(sel).count(), (Date.now()-t0)/1000 + "s");
await page.screenshot({ path: "tools/after-video.png" });
const last = page.locator(sel).last();
console.log(await last.getAttribute("aria-label"));
await last.click();
await page.waitForTimeout(2500);
const [dl] = await Promise.all([
  page.waitForEvent("download", { timeout: 30000 }),
  page.getByRole("button", { name: "Download" }).first().click(),
]);
const out = `output/${dl.suggestedFilename()}`;
await dl.saveAs(out);
console.log("saved", out);
await b.close();
