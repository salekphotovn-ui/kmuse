// Ảnh minh họa mục tự động dọn dẹp trong Cài đặt (dữ liệu giả, giao diện chạy trong trình duyệt).
import { chromium } from "playwright";
import http from "node:http";
import { readFileSync } from "node:fs";
import { extname, join } from "node:path";
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
await page.addInitScript(() => {
  window.__TAURI_INTERNALS__ = { invoke: async (cmd) => {
    if (cmd === "get_auto_clean") return { enabled: true, delete_files: false, last_ms: Date.now() - 3600e3, next_ms: Date.now() + 47 * 3600e3, last_summary: "12 mục lịch sử, 0 file, 9 cache, giải phóng 840 MB" };
    if (cmd === "get_storage_info") return { finished_jobs: 54, result_files: 70, result_bytes: 2.1 * 1024 ** 3, cache_bytes: 0.8 * 1024 ** 3, cache_busy: 0 };
    if (cmd === "get_runtime_stats") return { running: 3, waiting: 0, processes: 20, ram_bytes: 2.2 * 1024 ** 3 };
    if (cmd === "get_app_version") return "1.1.3";
    if (cmd === "get_run_settings") return { auto_open: true, hidden: false, idle_minutes: 5 };
    if (/^(list|queue)|jobs|accounts/.test(cmd)) return { jobs: [], total: 0 };
    return null;
  } };
});
const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };
const server = http.createServer((req, res) => {
  const file = join("ui", req.url === "/" ? "index.html" : req.url.split("?")[0]);
  try { res.writeHead(200, { "content-type": types[extname(file)] || "application/octet-stream" }); res.end(readFileSync(file)); } catch { res.writeHead(404); res.end(); }
}).listen(0);
await page.goto(`http://127.0.0.1:${server.address().port}/`);
await page.click('[data-tab="settings"]');
await page.waitForTimeout(800);
await page.evaluate(() => document.querySelector("#auto-clean").scrollIntoView({ block: "center" }));
await page.waitForTimeout(300);
await page.screenshot({ path: process.argv[2] });
await browser.close(); server.close();
