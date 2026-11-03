// Ảnh minh họa thanh trên cùng với số luồng và RAM (dữ liệu giả, chạy giao diện trong trình duyệt).
import { chromium } from "playwright";
import http from "node:http";
import { readFileSync } from "node:fs";
import { extname, join } from "node:path";
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 520 } });
await page.addInitScript(() => {
  window.__TAURI_INTERNALS__ = { invoke: async (cmd) => {
    if (cmd === "get_runtime_stats") return { running: 7, waiting: 12, processes: 31, ram_bytes: 3.4 * 1024 ** 3 };
    if (cmd === "get_app_version") return "1.1.3";
    if (/^(list|get|queue)/.test(cmd) || cmd.includes("jobs") || cmd.includes("accounts")) return [];
    return null;
  } };
});
const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };
const server = http.createServer((req, res) => {
  const file = join("ui", req.url === "/" ? "index.html" : req.url.split("?")[0]);
  try { res.writeHead(200, { "content-type": types[extname(file)] || "application/octet-stream" }); res.end(readFileSync(file)); } catch { res.writeHead(404); res.end(); }
}).listen(0);
await page.goto(`http://127.0.0.1:${server.address().port}/`);
await page.waitForTimeout(1500);
await page.screenshot({ path: process.argv[2] || "stats-demo.png" });
await browser.close();
server.close();
