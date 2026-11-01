import { chromium } from "playwright";
const b = await chromium.connectOverCDP(`http://127.0.0.1:${process.argv[2]}`);
const page = b.contexts().flatMap(c => c.pages()).find(p => p.url().includes("muse.ai"));
const sel = '[aria-label^="Open generated image"]';
const before = await page.locator(sel).count();
console.log("images before", before);
const box = page.locator('textarea[aria-label="Message"]');
await box.fill("Ảnh: a red apple on a white background, semi-realistic painted illustration, 16:9 widescreen. No text, no watermark.");
await box.press("Enter");
const t0 = Date.now();
while (Date.now() - t0 < 180000) {
  await page.waitForTimeout(3000);
  const n = await page.locator(sel).count();
  if (n > before) break;
}
console.log("images after", await page.locator(sel).count(), (Date.now()-t0)/1000 + "s");
await page.screenshot({ path: "tools/after-image.png" });
const last = page.locator(sel).last();
console.log(await last.getAttribute("aria-label"));
await last.click();
await page.waitForTimeout(2000);
await page.screenshot({ path: "tools/viewer.png" });
console.log(JSON.stringify(await page.evaluate(() => [...document.querySelectorAll("button,a,[role=dialog] img,[role=menuitem]")].map(e=>({t:e.tagName,aria:e.getAttribute("aria-label"),text:(e.innerText||"").trim().slice(0,30),href:e.href||e.src||null})).filter(x=>x.aria||x.text||x.href).slice(-30)),null,1));
await b.close();
