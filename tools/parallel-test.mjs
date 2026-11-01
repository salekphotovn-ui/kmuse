import { chromium } from "playwright";
import fs from "node:fs";
const ports = process.argv.slice(2);
const extract = fs.readFileSync("src-tauri/src/extract.js", "utf8");
const sel = '[aria-label^="Open generated image"]';
const sig = (page) => page.evaluate((s) => { const n = [...document.querySelectorAll(s)]; const e = n[n.length - 1]; return e ? e.getAttribute("aria-label") + "|" + ((e.querySelector("img") || {}).src || "") : ""; }, sel);
const prompts = ["a blue sailboat on a calm lake at dawn", "a small wooden cabin in a snowy forest", "a field of yellow sunflowers under a clear sky"];
fs.mkdirSync("output", { recursive: true });
const run = async (port, i) => {
  await new Promise((r) => setTimeout(r, i * 6000));
  const t0 = Date.now();
  const log = (m) => console.log(`[${port}] +${((Date.now() - t0) / 1000).toFixed(0)}s ${m}`);
  const b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const page = b.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes("muse.ai"));
  try {
    await page.keyboard.press("Escape");
    const before = await sig(page);
    const box = page.locator('textarea[aria-label="Message"]');
    await box.fill(`Ảnh: ${prompts[i % 3]}. 16:9 aspect ratio. No text, no watermark.`);
    await box.press("Enter");
    log("sent");
    while (Date.now() - t0 < 240000) {
      await page.waitForTimeout(3000);
      const now = await sig(page);
      if (now && now !== before) break;
    }
    const now = await sig(page);
    if (!now || now === before) return log("TIMEOUT");
    await page.waitForTimeout(2000);
    const r = await page.evaluate(`${extract}("image")`);
    if (r.error) return log("EXTRACT ERROR " + r.error);
    const file = `output/test-${port}.webp`;
    fs.writeFileSync(file, Buffer.from(r.b64, "base64"));
    log(`OK ${file} ${(r.b64.length * 0.75 / 1024).toFixed(0)}KB`);
  } catch (e) { log("ERR " + e.message.slice(0, 120)); }
  await b.close();
};
await Promise.all(ports.map(run));
