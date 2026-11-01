import { chromium } from "playwright";
const b = await chromium.connectOverCDP(`http://127.0.0.1:${process.argv[2]}`);
const pages = b.contexts().flatMap(c => c.pages());
console.log(pages.map(p => p.url()));
const page = pages.find(p => p.url().includes("muse.ai"));
await page.screenshot({ path: "tools/probe.png" });
const info = await page.evaluate(() => ({
  text: document.body.innerText.slice(0, 1500),
  controls: [...document.querySelectorAll("button,[role=button],textarea,input,[contenteditable=true],[role=tab],[role=menuitem]")].map(e => ({tag: e.tagName, text: (e.innerText||"").trim().slice(0,40), aria: e.getAttribute("aria-label"), ph: e.getAttribute("placeholder"), tid: e.getAttribute("data-testid")})).filter(x => x.text||x.aria||x.ph||x.tid).slice(0, 80)
}));
console.log(JSON.stringify(info, null, 1));
await b.close();
