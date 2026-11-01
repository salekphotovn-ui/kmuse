import { chromium } from "playwright";
const b = await chromium.connectOverCDP(`http://127.0.0.1:${process.argv[2]}`);
const page = b.contexts().flatMap(c => c.pages()).find(p => p.url().includes("muse.ai"));
await page.locator('[aria-label^="Open animated video"]').last().click();
await page.waitForTimeout(2500);
console.log(JSON.stringify(await page.evaluate(async () => {
  const v = document.querySelector("video"); const i = document.querySelector("[role=dialog] img, img[src^=blob]");
  const src = v?.currentSrc || v?.src || i?.src;
  const r = await fetch(src); const buf = await r.arrayBuffer();
  return { video: v?.currentSrc, vsrc: v?.src, img: i?.src, type: r.headers.get("content-type"), size: buf.byteLength };
})));
await page.getByRole("button",{name:"Close",exact:true}).first().click();
await b.close();
