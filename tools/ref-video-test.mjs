import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";

const extract = fs.readFileSync("src-tauri/src/extract.js", "utf8");
const jobs = [
  { port: 19158, image: "output/image-1793300743938.webp", motion: "the fox turns its head slowly and snow falls gently" },
  { port: 19405, image: "output/image-1793300568241.webp", motion: "gentle waves move and the boat rocks slightly as the sun rises" },
  { port: 19259, image: "output/image-1793300558215.webp", motion: "the window glows warmly and smoke drifts from the chimney" },
];
const videoSig = (page) => page.evaluate(() => {
  const n = [...document.querySelectorAll('[aria-label^="Open animated video"]')];
  const e = n[n.length - 1];
  return e ? e.getAttribute("aria-label") + "|" + (e.getAttribute("data-hatch-video-src") || "") : "";
});

const run = async ({ port, image, motion }, i) => {
  await new Promise((r) => setTimeout(r, i * 2000));
  const t0 = Date.now();
  const log = (m) => console.log(`[${port} ${path.basename(image)}] +${((Date.now() - t0) / 1000).toFixed(0)}s ${m}`);
  const b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const page = b.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes("muse.ai"));
  try {
    await page.bringToFront();
    await page.keyboard.press("Escape");
    const before = await videoSig(page);
    await page.locator("input[type=file]").first().setInputFiles(image);
    await page.waitForTimeout(2500);
    await page.screenshot({ path: `tools/ref-attached-${port}.png` });
    const box = page.locator('textarea[aria-label="Message"]');
    await box.fill(`Video: use the attached image as the reference and animate it as a 10 second video clip: ${motion}. Keep the same subject and style. 16:9. No text.`);
    await box.press("Enter");
    log("sent with reference");
    while (Date.now() - t0 < 420000) {
      await page.waitForTimeout(3000);
      if (port === 19259) await page.bringToFront();
      const now = await videoSig(page);
      if (now && now !== before) break;
    }
    const now = await videoSig(page);
    if (!now || now === before) return log("TIMEOUT: no new video");
    await page.waitForTimeout(1000);
    const r = await page.evaluate(`${extract}("video")`);
    if (r.error) return log("EXTRACT ERROR " + r.error);
    const out = `output/refvideo-${port}.mp4`;
    fs.writeFileSync(out, Buffer.from(r.b64, "base64"));
    if (r.poster) fs.writeFileSync(`output/refvideo-${port}-poster.jpg`, Buffer.from(r.poster.split(",")[1], "base64"));
    log(`OK ${out} ${(r.b64.length * 0.75 / 1048576).toFixed(1)}MB`);
  } catch (e) {
    log("ERR " + e.message.slice(0, 160));
  }
  await b.close();
};

await Promise.all(jobs.map(run));
