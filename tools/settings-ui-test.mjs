// Kiểm tra trang Cài đặt trong app thật: thanh cuộn, nút sao chép API, phiên bản. Cách dùng: node tools/settings-ui-test.mjs <cổng CDP>
import { chromium } from "playwright";

const browser = await chromium.connectOverCDP(`http://127.0.0.1:${process.argv[2]}`);
const page = browser.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith("devtools"));
const results = [];
const check = (name, ok, extra = "") => { results.push(ok); console.log(`${ok ? "PASS" : "FAIL"}  ${name} ${extra}`); };

await page.click('[data-tab="settings"]');
await page.waitForTimeout(1000);

const scroll = await page.evaluate(() => {
  const el = document.querySelector('.page[data-page="settings"]');
  el.scrollTop = 0;
  const before = el.scrollTop;
  el.scrollTop = el.scrollHeight;
  return { client: el.clientHeight, height: el.scrollHeight, moved: el.scrollTop > before, overflowY: getComputedStyle(el).overflowY };
});
check("Cài đặt cuộn được", scroll.height > scroll.client && scroll.moved && scroll.overflowY === "auto", JSON.stringify(scroll));

const lastVisible = await page.evaluate(() => {
  const api = document.querySelector("#api-example");
  api.scrollIntoView();
  const r = api.getBoundingClientRect();
  return r.bottom <= window.innerHeight + 2 && r.height > 20;
});
check("kéo xuống thấy khung API ở cuối", lastVisible);

await page.click("#api-copy-url");
await page.waitForTimeout(400);
const t1 = await page.textContent("#toast");
check("nút sao chép địa chỉ báo thành công", t1.includes("Đã sao chép địa chỉ"), `(${t1})`);
await page.click("#api-copy-guide");
await page.waitForTimeout(800);
const t2 = await page.textContent("#toast");
check("nút sao chép hướng dẫn báo thành công", t2.includes("Đã sao chép hướng dẫn"), `(${t2})`);
await page.click("#api-copy-example");
await page.waitForTimeout(400);
check("nút sao chép ví dụ báo thành công", (await page.textContent("#toast")).includes("Đã sao chép ví dụ"));

const version = await page.textContent("#app-version");
check("hiện số phiên bản thật", /^v\d+\.\d+\.\d+$/.test(version), `(${version})`);
console.log("trạng thái cập nhật:", await page.textContent("#update-state"));

console.log(results.every(Boolean) ? "\nTẤT CẢ ĐẠT" : "\nCÓ MỤC KHÔNG ĐẠT");
await browser.close();
process.exit(results.every(Boolean) ? 0 : 1);
