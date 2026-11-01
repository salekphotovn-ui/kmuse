// Bấm thử mục "Dọn dẹp" trong app thật (chạy với dữ liệu giả trong thư mục tạm).
// Cách dùng: node tools/clean-ui-test.mjs <thư mục sandbox> <cổng CDP của WebView2>
import { chromium } from "playwright";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const [sbx, port] = process.argv.slice(2);
const out = join(sbx, "app", "output");
const prof = join(sbx, "data", "Roaming", "com.musetool.desktop", "profiles", "acc1", "chrome");
const results = [];
const check = (name, ok) => { results.push(ok); console.log(`${ok ? "PASS" : "FAIL"}  ${name}`); };

const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
const page = browser.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith("devtools"));
await page.click('[data-tab="settings"]');
await page.waitForTimeout(1500);

const info = await page.textContent("#clean-info");
console.log("info:", info);
check("đếm 3 việc lịch sử", /Lịch sử: 3 việc/.test(info));
check("đếm 5 file kết quả (không tính file riêng)", /File đã tạo: 5 \(7 MB\)/.test(info));
check("tính cache ~6 MB", /Cache trình duyệt: 6 MB/.test(info));

// 1. cache
await page.click("#clean-cache");
await page.waitForSelector("#confirm-dialog[open]");
check("hộp xác nhận cache hiện", (await page.textContent("#confirm-title")).includes("cache"));
await page.click("#confirm-ok");
await page.waitForTimeout(1500);
check("cache Chrome đã xóa", !existsSync(join(prof, "Default", "Cache")) && !existsSync(join(prof, "GrShaderCache")));
check("đăng nhập (Cookies, Local Storage) còn nguyên", existsSync(join(prof, "Default", "Cookies")) && existsSync(join(prof, "Default", "Local Storage", "leveldb")));
console.log("toast:", await page.textContent("#toast"));

// 2. lịch sử: hủy trước, rồi đồng ý
await page.click("#clean-history");
await page.waitForSelector("#confirm-dialog[open]");
await page.click("#confirm-cancel");
await page.waitForTimeout(800);
check("bấm Hủy thì chưa xóa gì", /Lịch sử: 3 việc/.test(await page.textContent("#clean-info")));
await page.click("#clean-history");
await page.waitForSelector("#confirm-dialog[open]");
await page.click("#confirm-ok");
await page.waitForTimeout(1500);
const q = JSON.parse(readFileSync(join(sbx, "data", "Roaming", "com.musetool.desktop", "queue.json"), "utf8"));
check("chỉ còn việc đang chờ trong hàng chờ", q.jobs.length === 1 && q.jobs[0].status === "waiting");
check("ảnh thu nhỏ của việc đã xong bị xóa, của việc chờ còn", !existsSync(join(out, ".thumbs", "1.txt")) && !existsSync(join(out, ".thumbs", "3.txt")) && existsSync(join(out, ".thumbs", "4.txt")));
check("file ảnh/video vẫn còn", existsSync(join(out, "image-1.webp")) && existsSync(join(out, "video-3.mp4")));
console.log("toast:", await page.textContent("#toast"));

// 3. xóa cả file
await page.click("#clean-all");
await page.waitForSelector("#confirm-dialog[open]");
await page.click("#confirm-ok");
await page.waitForTimeout(1500);
check("file kết quả đã xóa", !existsSync(join(out, "image-1.webp")) && !existsSync(join(out, "image-2.webp")) && !existsSync(join(out, "video-3.mp4")) && !existsSync(join(out, "refs", "ref-b.png")));
check("ảnh tham chiếu của việc đang chờ được giữ", existsSync(join(out, "refs", "ref-a.png")));
check("file riêng của người dùng được giữ", existsSync(join(out, "my-own-picture.png")));
console.log("toast:", await page.textContent("#toast"));
console.log("info sau cùng:", await page.textContent("#clean-info"));
await page.screenshot({ path: join(sbx, "clean-after.png") });

console.log(results.every(Boolean) ? "\nTẤT CẢ ĐẠT" : "\nCÓ MỤC KHÔNG ĐẠT");
await browser.close();
process.exit(results.every(Boolean) ? 0 : 1);
