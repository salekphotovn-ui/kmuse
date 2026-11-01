# Muse Tool

Giao diện desktop tối, nhẹ, gồm bốn khu vực chính:

- Tạo ảnh
- Tạo video
- Tài khoản
- Cài đặt

Frontend dùng HTML/CSS/JavaScript thuần với Vite. Bộ khung Tauri 2 nằm trong `src-tauri` để đóng gói ứng dụng Windows ở giai đoạn tiếp theo.

## Chạy giao diện

```powershell
npm install
npm run dev
```

Mở `http://localhost:1420` nếu trình duyệt không tự mở.

## Kiểm tra và build frontend

```powershell
npm run check
npm test
npm run build
```

Kết quả frontend được tạo trong `dist`.

## Chạy bằng Tauri

Máy cần cài Rust và các Build Tools cho Windows trước khi chạy:

```powershell
npm run tauri dev
```

Khi hoàn thiện chức năng thật, có thể tạo bộ cài NSIS/MSI bằng:

```powershell
npm run tauri build
```

## Trạng thái hiện tại

Giao diện và các tương tác mẫu đã hoạt động. Các nút tạo ảnh, tạo video, mở tài khoản, lưu cookie, mã hóa dữ liệu và cập nhật từ xa chưa được nối với backend thật.

Mã launcher cookie thử nghiệm cũ vẫn được giữ tại `src/index.js` và có thể chạy riêng bằng `npm run session:start -- --cookies <file.json>`. Chỉ sử dụng cookie của chính tài khoản bạn; không chia sẻ hoặc commit file cookie vào Git.

## Cài đặt trên máy khác

Tải `Muse-Tool_x.y.z_x64-setup.exe` ở trang [Releases](https://github.com/salekphotovn-ui/kmuse/releases/latest) rồi chạy. Bộ cài không cần quyền quản trị, cài vào thư mục của người dùng hiện tại. Máy cần có Google Chrome (hoặc Microsoft Edge).

Dữ liệu của từng máy (tài khoản, hồ sơ Chrome, hàng chờ, cài đặt) nằm ngoài mã nguồn và không bị ghi đè khi cập nhật. Ảnh và video tạo ra nằm trong thư mục `output` cạnh ứng dụng.

## Cập nhật từ xa

Ứng dụng tự kiểm tra bản mới khi mở (có thể tắt trong Cài đặt → Cập nhật ứng dụng) và có nút "Kiểm tra cập nhật". Khi có bản mới, bấm "Cập nhật ngay": ứng dụng tải, kiểm tra chữ ký, cài và tự khởi động lại. Nút này từ chối chạy khi đang có việc để không làm gián đoạn.

### Ra bản mới (dành cho người phát triển)

```powershell
git add -A; git commit -m "Mô tả thay đổi"
node tools/release.mjs 1.0.1 "Ghi chú ngắn"
```

Lệnh trên nâng số phiên bản, gắn tag `v1.0.1` và đẩy lên GitHub. GitHub Actions (`.github/workflows/release.yml`) tự build bộ cài, ký và đăng bản phát hành kèm `latest.json`. Các máy đã cài sẽ thấy bản mới sau vài phút.

### Khóa ký bản cập nhật

Bản cập nhật được ký bằng khóa bí mật `kmuse.key` (mặc định ở `%USERPROFILE%\.tauri`, **không** nằm trong repo). Khóa công khai nằm trong `src-tauri/tauri.conf.json`. Khóa bí mật cũng được lưu ở GitHub Secrets với tên `TAURI_SIGNING_PRIVATE_KEY`. **Hãy sao lưu file khóa**: mất khóa thì các máy đã cài không nhận được bản cập nhật nữa và phải cài lại thủ công.

Build bộ cài trên máy cục bộ:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content "$env:USERPROFILE\.tauri\kmuse.key" -Raw
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
npx tauri build
```

Bộ cài nằm ở `src-tauri/target/release/bundle/nsis/`.
