import { chromium } from "playwright";

const port = process.argv[2];
if (!port) throw new Error("Missing Chrome debugging port");

const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
const pages = browser.contexts().flatMap((context) => context.pages());
const page = pages.find((candidate) => candidate.url().includes("muse.ai"));
if (!page) throw new Error("Muse page not found");

const responses = [];
page.on("response", async (response) => {
  const headers = response.request().headers();
  if (headers["next-action"] || response.url().includes("muse.ai")) {
    responses.push({ status: response.status(), url: response.url(), action: headers["next-action"] || null });
  }
});

const closePanel = page.getByRole("button", { name: "Close panel" });
if (await closePanel.count()) await closePanel.first().click();
await page.getByRole("button", { name: "Settings" }).click();
await page.getByText("Settings", { exact: true }).click();
await page.waitForTimeout(8000);
await page.screenshot({ path: "tools/muse-settings-state.png" });

const usage = await page.evaluate(() => {
  const text = (document.body.innerText || "").replace(/\r/g, "");
  const lines = text.split("\n").map((line) => line.trim()).filter(Boolean);
  const start = lines.findIndex((line) => line === "Usage");
  const block = start >= 0 ? lines.slice(start, start + 12) : [];
  return {
    loggedIn: !/Log in or create an account/i.test(text),
    tier: block.find((line) => /plan$/i.test(line)) || null,
    plan: block.find((line) => /% used/i.test(line) && !/tokens left/i.test(line)) || null,
    reset: block.find((line) => /reset/i.test(line)) || null,
    extra: block.find((line) => /tokens left/i.test(line)) || null,
    block,
    controls: [...document.querySelectorAll("button,[role=button]")].map((element) => ({
      text: (element.innerText || "").trim(),
      aria: element.getAttribute("aria-label"),
      title: element.getAttribute("title"),
      testid: element.getAttribute("data-testid"),
    })).filter((item) => item.text || item.aria || item.title || item.testid).slice(0, 80),
  };
});

usage.responses = responses.slice(-30);

console.log(JSON.stringify(usage, null, 2));
await browser.close();
