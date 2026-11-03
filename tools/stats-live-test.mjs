// Chạy KMUSE.exe với dữ liệu thử rồi gọi thẳng các lệnh mới qua WebView2 (cổng 9333).
import { chromium } from "playwright";
const browser = await chromium.connectOverCDP("http://127.0.0.1:9333");
const page = browser.contexts()[0].pages()[0];
const call = (cmd, args = {}) => page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
console.log("stats", JSON.stringify(await call("get_runtime_stats")));
console.log("auto0", JSON.stringify(await call("get_auto_clean")));
console.log("auto1", JSON.stringify(await call("set_auto_clean", { enabled: true, deleteFiles: false })));
console.log("cancel_waiting", await call("cancel_waiting"));
await page.waitForTimeout(500);
console.log("chip", await page.evaluate(() => document.querySelector("#stat-threads")?.innerText + " | " + document.querySelector("#stat-ram")?.innerText));
await browser.close();
