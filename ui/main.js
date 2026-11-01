const tabs = [...document.querySelectorAll("[data-tab]")];
const pages = [...document.querySelectorAll("[data-page]")];
const toast = document.querySelector("#toast");
let toastTimer;

function showToast(message) {
  window.clearTimeout(toastTimer);
  toast.textContent = message;
  toast.classList.add("is-visible");
  toastTimer = window.setTimeout(() => toast.classList.remove("is-visible"), 2600);
}

function activateTab(name) {
  tabs.forEach((tab) => {
    const active = tab.dataset.tab === name;
    tab.classList.toggle("is-active", active);
    tab.setAttribute("aria-selected", String(active));
  });

  pages.forEach((page) => {
    const active = page.dataset.page === name;
    page.classList.toggle("is-active", active);
    page.hidden = !active;
  });

  window.localStorage.setItem("muse-tool.active-tab", name);
}

tabs.forEach((tab) => tab.addEventListener("click", () => activateTab(tab.dataset.tab)));

const savedTab = window.localStorage.getItem("muse-tool.active-tab");
if (savedTab && tabs.some((tab) => tab.dataset.tab === savedTab)) activateTab(savedTab);

document.querySelectorAll("[data-file-trigger]").forEach((button) => {
  const input = document.querySelector(`#${button.dataset.fileTrigger}`);
  button.addEventListener("click", () => input?.click());
  input?.addEventListener("change", () => {
    const label = button.querySelector("[data-file-label]");
    if (input.files?.[0] && label) label.textContent = input.files[0].name;
  });
});

const tauriInvoke = window.__TAURI_INTERNALS__?.invoke;
const museAccounts = new Map();
const thumbs = new Map();
const thumbRequested = new Set();
const imageViewerDialog = document.querySelector("#image-viewer-dialog");
let jobsState = { jobs: [], paused: false, now_ms: Date.now() };
let viewedJobId = null;

function escapeHTML(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

function splitPrompts(value) {
  return String(value).split(/\r?\n\s*\r?\n+/).map((prompt) => prompt.trim()).filter(Boolean);
}

function resultTitle(prompt) {
  const cleanPrompt = String(prompt).replace(/\.{3}$/, "").trim();
  return cleanPrompt.length > 34 ? `${cleanPrompt.slice(0, 34)}…` : cleanPrompt;
}

// Queue state lives in the Rust backend; each panel only renders the jobs of its kind.
const panels = {
  image: {
    kind: "image", noun: "ảnh", tag: "IMG", typical: 40, page: 1, filter: "all", selected: new Set(),
    list: "#image-queue-list", pageLabel: "#queue-page-label", prev: "#queue-prev", next: "#queue-next", visible: "#queue-visible-count", pause: "#toggle-queue",
    grid: "#image-results-grid", count: "#results-count", filterCount: (s) => `#filter-${s}-count`, summary: (s) => `#summary-${s}`, filterAttr: "data-queue-filter", form: "#image-form",
    sceneClass: "result-scene", referenceInput: "#image-reference", openFolder: "#open-results-folder", download: "#download-selected-results",
  },
  video: {
    kind: "video", noun: "video", tag: "VID", typical: 100, page: 1, filter: "all", selected: new Set(),
    list: "#video-queue-list", pageLabel: "#video-queue-page-label", prev: "#video-queue-prev", next: "#video-queue-next", visible: "#video-queue-visible-count", pause: "#toggle-video-queue",
    grid: "#video-results-grid", count: "#video-results-count", filterCount: (s) => `#video-filter-${s}-count`, summary: (s) => `#video-summary-${s}`, filterAttr: "data-video-filter", form: "#video-form",
    sceneClass: "video-result-scene", referenceInput: "#video-reference", openFolder: "#open-video-results-folder", download: "#download-selected-videos",
  },
};

function applyDynamicStyles(root) {
  // The CSP blocks inline style attributes, so thumbnails and progress widths are applied through the DOM.
  root.querySelectorAll("[data-thumb-job]").forEach((element) => {
    const thumb = thumbs.get(Number(element.dataset.thumbJob));
    if (thumb) element.style.backgroundImage = `url("${thumb}")`;
  });
  root.querySelectorAll("[data-width]").forEach((element) => { element.style.width = `${element.dataset.width}%`; });
}

function jobProgress(job, panel) {
  if (job.status === "done") return 100;
  if (job.status !== "running" || !job.started_ms) return 0;
  return Math.min(95, Math.round(((jobsState.now_ms - job.started_ms) / 1000 / panel.typical) * 100));
}

function jobTiming(job) {
  if (job.status === "running" && job.started_ms) return `${Math.max(0, Math.round((jobsState.now_ms - job.started_ms) / 1000))} giây`;
  if (job.status === "waiting") return job.message || "Đang chờ";
  if (job.status === "done" && job.finished_ms && job.started_ms) return `Xong sau ${Math.round((job.finished_ms - job.started_ms) / 1000)} giây`;
  return job.message || "";
}

function statusView(job, progress) {
  if (job.status === "running") return { label: `Đang tạo · ${progress}%`, icon: job.kind === "video" ? "i-video" : "i-refresh", action: null };
  if (job.status === "waiting") return { label: "Đang chờ", icon: "i-more", action: "Hủy", actionName: "cancel" };
  if (job.status === "done") return { label: "Hoàn thành", icon: job.kind === "video" ? "i-video" : "i-image", action: "Mở", actionName: "open" };
  return { label: "Lỗi", icon: "i-x", action: "Thử lại", actionName: "retry" };
}

function renderPanel(panel) {
  const jobs = jobsState.jobs.filter((job) => job.kind === panel.kind);
  const counts = { all: jobs.length };
  for (const status of ["running", "waiting", "done", "error"]) counts[status] = jobs.filter((job) => job.status === status).length;
  for (const status of ["all", "running", "waiting", "done", "error"]) document.querySelector(panel.filterCount(status)).textContent = counts[status];
  for (const status of ["running", "waiting", "done", "error"]) document.querySelector(panel.summary(status)).textContent = counts[status];

  const filtered = jobs.filter((job) => panel.filter === "all" || job.status === panel.filter);
  const pageSize = 4;
  const pageCount = Math.max(1, Math.ceil(filtered.length / pageSize));
  panel.page = Math.min(panel.page, pageCount);
  const visible = filtered.slice((panel.page - 1) * pageSize, panel.page * pageSize);
  document.querySelector(panel.visible).textContent = `Hiển thị ${visible.length} / ${filtered.length} công việc`;
  document.querySelector(panel.pageLabel).textContent = `${panel.page} / ${pageCount}`;
  document.querySelector(panel.prev).disabled = panel.page <= 1;
  document.querySelector(panel.next).disabled = panel.page >= pageCount;
  document.querySelector(panel.pause).textContent = jobsState.paused ? "Tiếp tục nhận việc" : "Tạm dừng nhận việc";

  const list = document.querySelector(panel.list);
  list.innerHTML = visible.length ? visible.map((job) => {
    const progress = jobProgress(job, panel);
    const view = statusView(job, progress);
    const owner = job.status === "waiting" ? "Chờ tài khoản rảnh" : (job.account || "—");
    const bar = job.status === "running" ? `<div class="queue-progress"><span data-width="${progress}"></span></div>` : "";
    const action = view.action ? `<button class="${job.status === "error" ? "queue-action is-error" : "queue-action"}" type="button" data-job-action="${view.actionName}">${view.action}</button>` : "";
    return `
      <article class="queue-job" data-job-id="${job.id}">
        <span class="queue-thumb is-${job.status}"><svg aria-hidden="true"><use href="#${view.icon}"></use></svg></span>
        <div class="queue-job-main">
          <div><strong>#${panel.tag}-${job.id}</strong><span class="queue-status is-${job.status}"><span></span>${view.label}</span></div>
          <p>${escapeHTML(job.prompt)}</p>
          <small>${escapeHTML(owner)} <b>•</b> ${escapeHTML(jobTiming(job))}</small>
        </div>
        ${action}${bar}
      </article>`;
  }).join("") : '<div class="queue-empty">Không có công việc ở trạng thái này.</div>';
  applyDynamicStyles(list);

  const results = jobs.filter((job) => job.status === "done");
  document.querySelector(panel.count).textContent = `${results.length} ${panel.noun} · Mới nhất hiển thị trước`;
  const grid = document.querySelector(panel.grid);
  if (!results.length) {
    grid.innerHTML = `<div class="result-empty">${panel.noun[0].toUpperCase()}${panel.noun.slice(1)} hoàn thành sẽ tự động xuất hiện tại đây.</div>`;
    return;
  }
  grid.innerHTML = results.slice(0, 12).map((job) => {
    const chosen = panel.selected.has(job.id);
    return `
      <article class="result-card${chosen ? " is-selected" : ""}" data-result-id="${job.id}">
        <button class="result-open" type="button" data-open-result="${job.id}" aria-label="Xem ${panel.noun} ${escapeHTML(resultTitle(job.prompt))}">
          <span class="result-image ${panel.sceneClass}-${job.id % 4}" data-thumb-job="${job.id}"></span>
          <span class="result-copy"><strong>${escapeHTML(resultTitle(job.prompt))}</strong><small>${escapeHTML(job.account || "—")} · ${escapeHTML(jobTiming(job))}</small></span>
        </button>
        <button class="result-select" type="button" data-select-result="${job.id}" aria-label="${chosen ? "Bỏ chọn" : "Chọn"} ${panel.noun}"><svg aria-hidden="true"><use href="#i-check"></use></svg></button>
      </article>`;
  }).join("");
  applyDynamicStyles(grid);
}

function renderAllPanels() {
  Object.values(panels).forEach(renderPanel);
}

async function loadThumbs() {
  for (const job of jobsState.jobs.filter((item) => item.status === "done" && !thumbRequested.has(item.id))) {
    thumbRequested.add(job.id);
    const thumb = await tauriInvoke("get_thumb", { id: job.id }).catch(() => null);
    if (thumb) { thumbs.set(job.id, thumb); renderAllPanels(); }
  }
}

async function refreshJobs() {
  if (!tauriInvoke) return;
  try {
    jobsState = await tauriInvoke("list_jobs");
    renderAllPanels();
    loadThumbs();
  } catch (error) {
    showToast(`Không đọc được hàng đợi: ${String(error)}`);
  }
}

function openImageViewer(job) {
  if (!job || job.status !== "done") return;
  viewedJobId = job.id;
  const preview = document.querySelector("#image-viewer-preview");
  preview.className = `image-viewer-preview result-scene-${job.id % 4}`;
  preview.style.backgroundImage = thumbs.get(job.id) ? `url("${thumbs.get(job.id)}")` : "";
  document.querySelector("#image-viewer-title").textContent = resultTitle(job.prompt);
  document.querySelector("#image-viewer-prompt").textContent = job.prompt;
  document.querySelector("#image-viewer-meta").textContent = `${job.account || "—"} · ${job.ratio} · ${jobTiming(job)}`;
  imageViewerDialog.showModal();
}

function openFile(job) {
  if (job.file) tauriInvoke?.("open_media_file", { path: job.file }).catch((error) => showToast(String(error)));
}

function readBase64(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(",")[1]);
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

for (const panel of Object.values(panels)) {
  // Optional reference image: the file is copied into the results folder, and only that path is sent to Muse.
  const referenceInput = document.querySelector(panel.referenceInput);
  const referenceLabel = referenceInput.previousElementSibling.querySelector("[data-file-label]");
  const referenceDefault = referenceLabel.textContent;
  panel.reference = null;
  panel.setReference = (path, label) => {
    panel.reference = path;
    referenceLabel.textContent = path ? label : referenceDefault;
    if (!path) referenceInput.value = "";
  };
  referenceInput.addEventListener("change", async () => {
    const file = referenceInput.files?.[0];
    if (!file) return;
    if (!tauriInvoke) return showToast("Hãy mở KMUSE trên Windows để dùng ảnh tham chiếu.");
    try {
      panel.setReference(await tauriInvoke("save_reference", { name: file.name, data: await readBase64(file) }), file.name);
    } catch (error) {
      panel.setReference(null);
      showToast(String(error));
    }
  });

  document.querySelector(panel.form).addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.currentTarget;
    const prompts = splitPrompts(form.elements.prompt.value);
    if (!prompts.length) {
      showToast(`Anh hãy nhập mô tả ${panel.kind === "video" ? "video" : "hình ảnh"} trước.`);
      form.elements.prompt.focus();
      return;
    }
    if (!tauriInvoke) return showToast("Hãy mở KMUSE trên Windows để tạo thật.");
    const quantity = Number(form.elements.quantity.value);
    const account = form.elements.account.value === "auto" ? null : form.elements.account.value;
    try {
      const added = await tauriInvoke("enqueue_jobs", { kind: panel.kind, prompts, ratio: form.elements.ratio.value, quantity, account, reference: panel.reference });
      panel.filter = "all";
      panel.page = 1;
      document.querySelectorAll(`[${panel.filterAttr}]`).forEach((button) => button.classList.toggle("is-active", button.getAttribute(panel.filterAttr) === "all"));
      form.elements.prompt.value = "";
      panel.setReference(null);
      showToast(`Đã thêm ${added} công việc từ ${prompts.length} prompt.`);
      refreshJobs();
    } catch (error) {
      showToast(String(error));
    }
  });

  document.querySelectorAll(`[${panel.filterAttr}]`).forEach((button) => button.addEventListener("click", () => {
    panel.filter = button.getAttribute(panel.filterAttr);
    panel.page = 1;
    document.querySelectorAll(`[${panel.filterAttr}]`).forEach((item) => item.classList.toggle("is-active", item === button));
    renderPanel(panel);
  }));
  document.querySelector(panel.prev).addEventListener("click", () => { panel.page -= 1; renderPanel(panel); });
  document.querySelector(panel.next).addEventListener("click", () => { panel.page += 1; renderPanel(panel); });

  document.querySelector(panel.pause).addEventListener("click", async () => {
    await tauriInvoke?.("set_paused", { paused: !jobsState.paused });
    showToast(jobsState.paused ? "Đã tiếp tục phân luồng hàng đợi." : "Đã tạm dừng phân luồng công việc mới.");
    refreshJobs();
  });

  document.querySelector(panel.list).addEventListener("click", async (event) => {
    const action = event.target.closest("[data-job-action]");
    const row = event.target.closest("[data-job-id]");
    if (!action || !row) return;
    const job = jobsState.jobs.find((item) => item.id === Number(row.dataset.jobId));
    if (!job) return;
    try {
      if (action.dataset.jobAction === "cancel") await tauriInvoke("cancel_job", { id: job.id });
      else if (action.dataset.jobAction === "retry") await tauriInvoke("retry_job", { id: job.id });
      else if (panel.kind === "image") openImageViewer(job);
      else openFile(job);
    } catch (error) {
      showToast(String(error));
    }
    refreshJobs();
  });

  document.querySelector(panel.grid).addEventListener("click", (event) => {
    const selectButton = event.target.closest("[data-select-result]");
    if (selectButton) {
      const id = Number(selectButton.dataset.selectResult);
      if (panel.selected.has(id)) panel.selected.delete(id); else panel.selected.add(id);
      renderPanel(panel);
      return;
    }
    const openButton = event.target.closest("[data-open-result]");
    if (!openButton) return;
    const job = jobsState.jobs.find((item) => item.id === Number(openButton.dataset.openResult));
    if (panel.kind === "image") openImageViewer(job); else if (job) openFile(job);
  });

  document.querySelector(panel.openFolder).addEventListener("click", () => tauriInvoke?.("open_output_folder"));
  document.querySelector(panel.download).addEventListener("click", () => {
    const count = jobsState.jobs.filter((job) => job.kind === panel.kind && job.status === "done" && panel.selected.has(job.id)).length;
    showToast(count ? `${count} ${panel.noun} đã chọn đã được lưu sẵn trong thư mục kết quả.` : `Anh hãy chọn ít nhất một ${panel.noun}.`);
  });
}

document.querySelector("#use-as-reference").addEventListener("click", () => {
  const job = jobsState.jobs.find((item) => item.id === viewedJobId);
  if (!job?.file) return;
  panels.video.setReference(job.file, `Ảnh #${job.id}`);
  imageViewerDialog.close();
  activateTab("create-video");
  showToast(`Đã chọn ảnh #${job.id} làm tham chiếu. Nhập mô tả chuyển động rồi thêm vào hàng đợi.`);
});
document.querySelector("#close-image-viewer").addEventListener("click", () => imageViewerDialog.close());
document.querySelector("#open-viewed-folder").addEventListener("click", () => tauriInvoke?.("open_output_folder"));
document.querySelector("#download-viewed-result").addEventListener("click", () => {
  const job = jobsState.jobs.find((item) => item.id === viewedJobId);
  if (job) openFile(job);
});
document.querySelector("#delete-viewed-result").addEventListener("click", async () => {
  if (viewedJobId === null) return;
  await tauriInvoke?.("delete_job", { id: viewedJobId });
  imageViewerDialog.close();
  showToast(`Đã xóa #IMG-${viewedJobId} khỏi danh sách (file vẫn nằm trong thư mục kết quả).`);
  refreshJobs();
});

window.setInterval(refreshJobs, 1000);
refreshJobs();


const accountDialog = document.querySelector("#account-dialog");
const accountForm = document.querySelector("#account-form");
const accountList = document.querySelector("#account-list");
const warnInput = document.querySelector("#warn-percent");
const usageWarning = document.querySelector("#usage-warning");
const accountsNav = document.querySelector('[data-tab="accounts"]');
let lowAccountIds = new Set();
try { const saved = Number(window.localStorage.getItem("warnPercent")); if (saved >= 50 && saved <= 100) warnInput.value = String(saved); } catch { /* storage unavailable */ }

// Share of the account's capacity already used: the weekly plan allowance, or the extra tokens once those run out last.
// An account keeps working while either meter has room, so the lower of the two counts.
function usedPercent(account) {
  const pct = (text) => { const m = /(\d+(?:\.\d+)?)%\s*used/i.exec(text || ""); return m ? Number(m[1]) : null; };
  const plan = pct(account.plan_usage);
  let extra = pct(account.additional_tokens);
  if (extra === null && /\b0 tokens left/i.test(account.additional_tokens || "")) extra = 100;
  const known = [plan, extra].filter((v) => v !== null);
  return known.length ? Math.min(...known) : null;
}

function renderAccounts(accounts) {
  const threshold = Math.min(100, Math.max(50, Number(warnInput.value) || 95));
  const low = accounts.filter((account) => { const used = usedPercent(account); return used !== null && used >= threshold; });
  const lowIds = new Set(low.map((account) => account.id));
  const fresh = low.filter((account) => !lowAccountIds.has(account.id));
  lowAccountIds = lowIds;
  usageWarning.hidden = !low.length;
  usageWarning.textContent = low.length ? `⚠ ${low.length} tài khoản sắp hết token: ${low.map((account) => account.name).join(", ")} — nên thay tài khoản` : "";
  accountsNav.classList.toggle("has-alert", low.length > 0);
  if (fresh.length) showToast(`⚠ Sắp hết token: ${fresh.map((account) => account.name).join(", ")}. Nên thay tài khoản.`);
  const subtitle = document.querySelector('[data-page="accounts"] .page-heading p');
  const ready = accounts.filter((a) => a.status === "Đã kết nối" || a.status.startsWith("Sẵn sàng")).length;
  if (subtitle) subtitle.textContent = `Tổng ${accounts.length} tài khoản · ${ready} sẵn sàng. Theo dõi trạng thái và mở nhanh từng phiên làm việc.`;
  if (!accounts.length) {
    accountList.innerHTML = '<div class="account-empty">Chưa có tài khoản. Thêm tài khoản rồi đăng nhập Muse trong hồ sơ riêng.</div>';
    return;
  }
  accountList.innerHTML = accounts.map((account, index) => `
    <article class="account-row${lowIds.has(account.id) ? " is-low" : ""}" data-account-id="${escapeHTML(account.id)}">
      <span class="avatar">${index === 0 ? "MC" : `M${String(index).padStart(2, "0")}`}</span>
      <div class="account-detail"><strong>${escapeHTML(account.name)}${lowIds.has(account.id) ? `<span class="low-badge">Sắp hết ${usedPercent(account)}%</span>` : ""}</strong><small>${escapeHTML(account.plan_usage || "Hồ sơ trình duyệt riêng trên máy này")}</small>${account.additional_tokens ? `<small class="account-tokens">Token còn lại: ${escapeHTML(account.additional_tokens)}</small>` : ""}</div>
      <span class="health ${account.status === "Đã kết nối" || account.status.startsWith("Sẵn sàng") ? "is-good" : "is-warning"}"><span></span>${escapeHTML(account.status)}</span>
      <button class="button button-soft check-account" type="button">Kiểm tra</button>
      <button class="button button-soft open-account" type="button">Đăng nhập / Mở Muse</button>
      <button class="icon-button remove-account" type="button" aria-label="Xóa ${escapeHTML(account.name)}"><svg><use href="#i-x" /></svg></button>
    </article>`).join("");
}


function syncMuseAccounts(accounts) {
  const usable = accounts.filter((account) => account.status === "Đã kết nối" || account.status.startsWith("Sẵn sàng") || account.status === "Chưa mở cửa sổ Muse");
  museAccounts.clear();
  usable.forEach((account) => museAccounts.set(account.id, account.name));
  document.querySelectorAll('select[name="account"]').forEach((select) => {
    select.innerHTML = '<option value="auto">Tự động chọn tài khoản đang rảnh</option>' + usable.map((account) => `<option value="${escapeHTML(account.id)}">${escapeHTML(account.name)}</option>`).join("");
  });
}

let lastAccountsSnapshot = "";

async function refreshAccounts() {
  if (!tauriInvoke) {
    accountList.innerHTML = '<div class="account-empty">Quản lý tài khoản khả dụng trong ứng dụng KMUSE trên Windows.</div>';
    return [];
  }
  const accounts = await tauriInvoke("list_accounts");
  const snapshot = JSON.stringify(accounts);
  if (snapshot !== lastAccountsSnapshot) {
    lastAccountsSnapshot = snapshot;
    renderAccounts(accounts);
    syncMuseAccounts(accounts);
  }
  return accounts;
}

function openAccountDialog(name = "") {
  accountForm.reset();
  accountForm.elements.name.value = name;
  accountDialog.showModal();
  window.setTimeout(() => accountForm.elements.name.focus(), 0);
}

document.querySelector("#add-account").addEventListener("click", () => openAccountDialog());

accountForm.addEventListener("submit", (event) => {
  if (event.submitter?.value === "cancel") return;
  event.preventDefault();
  const name = accountForm.elements.name.value.trim();
  if (!name) return;

  if (!tauriInvoke) {
    accountDialog.close();
    showToast("Hãy mở KMUSE trên Windows để thêm tài khoản thật.");
    return;
  }
  tauriInvoke("add_account", { name }).then(async () => {
    accountDialog.close();
    await refreshAccounts();
    showToast(`Đã thêm “${name}”. Bấm Đăng nhập / Mở Muse để đăng nhập riêng.`);
  }).catch((error) => showToast(String(error)));
});

accountList.addEventListener("click", (event) => {
  const row = event.target.closest(".account-row");
  if (!row) return;
  const name = row.querySelector("strong").textContent;
  const id = row.dataset.accountId;
  if (event.target.closest(".open-account")) {
    if (!tauriInvoke) return showToast("Mở tài khoản khả dụng trong KMUSE trên Windows.");
    const button = event.target.closest(".open-account");
    button.disabled = true;
    tauriInvoke("open_account", { id }).then(async () => {
      await refreshAccounts();
      showToast(`Đã mở Muse trong hồ sơ riêng “${name}”. Hoàn tất đăng nhập ở cửa sổ trình duyệt.`);
    }).catch((error) => showToast(String(error))).finally(() => { button.disabled = false; });
  } else if (event.target.closest(".check-account")) {
    const button = event.target.closest(".check-account");
    button.disabled = true;
    button.textContent = "Đang kiểm tra…";
    tauriInvoke("check_account", { id }).then(async () => {
      await refreshAccounts();
      showToast(`Đã cập nhật trạng thái và Usage của “${name}”.`);
    }).catch((error) => showToast(String(error))).finally(() => { button.disabled = false; button.textContent = "Kiểm tra"; });
  } else if (event.target.closest(".remove-account")) {
    if (!tauriInvoke) return;
    tauriInvoke("remove_account", { id }).then(refreshAccounts).then(() => showToast(`Đã xóa mục “${name}”. Hồ sơ trình duyệt được giữ nguyên.`)).catch((error) => showToast(String(error)));
  }
});

warnInput.addEventListener("change", () => {
  const value = Math.min(100, Math.max(50, Math.round(Number(warnInput.value) || 95)));
  warnInput.value = String(value);
  try { window.localStorage.setItem("warnPercent", String(value)); } catch { /* storage unavailable */ }
  lastAccountsSnapshot = "";
  refreshAccounts().catch(() => {});
});

const refreshUsageButton =document.querySelector("#refresh-usage");
refreshUsageButton.addEventListener("click", async () => {
  if (!tauriInvoke) return;
  refreshUsageButton.disabled = true;
  try {
    await tauriInvoke("refresh_all_usage");
    for (;;) {
      const [running, done, total] = await tauriInvoke("usage_progress");
      refreshUsageButton.textContent = total ? `Đang đọc ${done}/${total}…` : "Đang đọc…";
      await refreshAccounts();
      if (!running) break;
      await new Promise((resolve) => setTimeout(resolve, 1500));
    }
    showToast("Đã cập nhật lượng token của các tài khoản.");
  } catch (error) {
    showToast(String(error));
  } finally {
    refreshUsageButton.disabled = false;
    refreshUsageButton.textContent = "Làm mới token";
  }
});

refreshAccounts().catch((error) => showToast(`Không tải được danh sách tài khoản: ${String(error)}`));
window.setInterval(() => {
  if (!accountList.querySelector("button:disabled")) refreshAccounts().catch(() => {});
}, 4000);

document.querySelectorAll(".switch:not(.api-switch) input").forEach((input, index) => {
  const key = `muse-tool.setting-${index}`;
  const saved = window.localStorage.getItem(key);
  if (saved !== null) input.checked = saved === "true";
  input.addEventListener("change", () => {
    window.localStorage.setItem(key, String(input.checked));
    showToast("Đã lưu cài đặt trên máy.");
  });
});

document.querySelector("#open-data").addEventListener("click", () => {
  showToast("Mở thư mục dữ liệu sẽ hoạt động khi nối Tauri.");
});

const apiEnabled = document.querySelector("#api-enabled");
const apiPort = document.querySelector("#api-port");
const apiState = document.querySelector("#api-state");

let apiBase = "http://127.0.0.1:17890";

function renderApi(status) {
  apiEnabled.checked = status.enabled;
  apiPort.value = status.port;
  apiState.textContent = status.error || (status.running ? `Đang chạy tại http://127.0.0.1:${status.port}` : "Đang tắt");
  apiState.className = `api-state ${status.error ? "is-bad" : status.running ? "is-good" : ""}`;
  const base = `http://127.0.0.1:${status.port}`;
  apiBase = base;
  document.querySelector("#api-docs-url").textContent = `${base}/docs`;
  document.querySelector("#api-example").textContent = [
    `# Gửi nhiều prompt khác nhau trong một lần`,
    `curl -X POST ${base}/jobs -H "Content-Type: application/json" -d "{\\"jobs\\":[{\\"kind\\":\\"image\\",\\"prompt\\":\\"a red apple\\"},{\\"kind\\":\\"video\\",\\"prompt\\":\\"a cat chasing a ball\\"}]}"`,
    `# Video từ ảnh đã tạo: thêm "reference_job": <id ảnh> (hoặc "reference_path" trong thư mục output)`,
    `# Chờ job xong rồi lấy kết quả (tối đa 600 giây)`,
    `curl "${base}/jobs/<id>?wait=120"`,
    `# Tải file kết quả    |  Xem tài khoản    |  Hủy job đang chờ`,
    `curl -o out.webp ${base}/jobs/<id>/file`,
    `curl ${base}/accounts`,
    `curl -X DELETE ${base}/jobs/<id>`,
  ].join("\n");
}

async function loadApi() {
  if (!tauriInvoke) { apiState.textContent = "Chỉ khả dụng trong ứng dụng KMUSE trên Windows."; return; }
  renderApi(await tauriInvoke("get_api_settings"));
}

async function saveApi() {
  try {
    renderApi(await tauriInvoke("set_api_settings", { enabled: apiEnabled.checked, port: Number(apiPort.value) }));
    showToast("Đã áp dụng cài đặt API.");
  } catch (error) {
    showToast(String(error));
  }
}

// Clipboard API needs a secure context; fall back to a hidden textarea so the buttons work either way.
async function copyText(text, done) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.classList.add("visually-hidden");
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    if (!ok) { showToast("Không sao chép được. Hãy chọn và copy thủ công."); return; }
  }
  showToast(done);
}

document.querySelector("#api-copy-url").addEventListener("click", () => copyText(apiBase, "Đã sao chép địa chỉ API."));
document.querySelector("#api-copy-example").addEventListener("click", () => copyText(document.querySelector("#api-example").textContent, "Đã sao chép ví dụ gọi API."));
document.querySelector("#api-copy-guide").addEventListener("click", async () => {
  if (!tauriInvoke) return;
  try {
    copyText(await tauriInvoke("get_api_guide"), "Đã sao chép hướng dẫn. Dán vào cuộc trò chuyện của AI khác.");
  } catch (error) {
    showToast(String(error));
  }
});

document.querySelector("#api-save").addEventListener("click", saveApi);
apiEnabled.addEventListener("change", saveApi);
loadApi().catch(() => {});

const runAuto = document.querySelector("#run-auto-open");
const runHidden = document.querySelector("#run-hidden");
const runIdle = document.querySelector("#run-idle");

function renderRun(settings) {
  runAuto.checked = settings.auto_open;
  runHidden.checked = settings.hidden;
  runIdle.value = settings.idle_minutes;
}

async function saveRun() {
  try {
    renderRun(await tauriInvoke("set_run_settings", { autoOpen: runAuto.checked, hidden: runHidden.checked, idleMinutes: Math.max(0, Number(runIdle.value) || 0) }));
    showToast("Đã lưu chế độ chạy.");
  } catch (error) {
    showToast(String(error));
  }
}

[runAuto, runHidden].forEach((input) => input.addEventListener("change", saveRun));
runIdle.addEventListener("change", saveRun);
if (tauriInvoke) tauriInvoke("get_run_settings").then(renderRun).catch(() => {});

// ---- app updates (signed releases on GitHub) ----
const updateState = document.querySelector("#update-state");
const updateCheck = document.querySelector("#update-check");
const updateInstall = document.querySelector("#update-install");
const updateAuto = document.querySelector("#update-auto");
let appVersion = "";
try { updateAuto.checked = window.localStorage.getItem("autoUpdate") !== "off"; } catch { /* storage unavailable */ }

const updateDialog = document.querySelector("#update-dialog");
const updateDialogText = document.querySelector("#update-dialog-text");
const updateDialogNotes = document.querySelector("#update-dialog-notes");
const updateNow = document.querySelector("#update-now");
const updateLater = document.querySelector("#update-later");
let pendingUpdate = null;
let installing = false;

function openUpdateDialog(found) {
  pendingUpdate = found;
  document.querySelector("#update-dialog-title").textContent = `Có phiên bản mới ${found.version}`;
  updateDialogText.textContent = `Bạn đang dùng phiên bản ${appVersion}. Bấm “Cập nhật ngay” để tự tải, cài đặt và mở lại KMUSE.`;
  const notes = (found.notes || "").trim();
  updateDialogNotes.hidden = !notes;
  updateDialogNotes.textContent = notes;
  updateNow.disabled = false;
  updateLater.disabled = false;
  if (!updateDialog.open) updateDialog.showModal();
}

async function installPendingUpdate() {
  if (installing || !pendingUpdate) return;
  installing = true;
  updateNow.disabled = true;
  updateLater.disabled = true;
  updateDialogText.textContent = "Đang tải và cài bản mới… KMUSE sẽ tự đóng và mở lại sau vài giây. Vui lòng không tắt máy.";
  updateState.textContent = "Đang cập nhật…";
  try {
    await tauriInvoke("install_update");
  } catch (error) {
    // Nothing was installed (for example jobs are running): let the user read why and try again later.
    installing = false;
    updateDialogText.textContent = String(error);
    updateState.textContent = String(error);
    updateNow.disabled = false;
    updateLater.disabled = false;
  }
}

updateNow.addEventListener("click", installPendingUpdate);
updateLater.addEventListener("click", () => updateDialog.close());
// Escape must not dismiss the dialog halfway through an install.
updateDialog.addEventListener("cancel", (event) => { if (installing) event.preventDefault(); });

async function checkForUpdate({ quiet }) {
  if (!tauriInvoke) { updateState.textContent = "Chỉ khả dụng trong ứng dụng KMUSE trên Windows."; return; }
  updateCheck.disabled = true;
  updateState.textContent = `Phiên bản ${appVersion} · đang kiểm tra…`;
  try {
    const found = await tauriInvoke("check_update");
    updateInstall.hidden = !found;
    if (found) {
      pendingUpdate = found;
      updateState.textContent = `Phiên bản ${appVersion} · có bản mới ${found.version}`;
      // At startup the question is asked right away; from Settings the button below asks it.
      if (quiet) openUpdateDialog(found);
    } else {
      updateState.textContent = `Phiên bản ${appVersion} · đang dùng bản mới nhất`;
      if (!quiet) showToast("Đang dùng bản mới nhất.");
    }
  } catch (error) {
    updateState.textContent = `Phiên bản ${appVersion} · ${quiet ? "chưa kiểm tra được bản mới (không có mạng?)" : String(error)}`;
    if (!quiet) showToast(String(error));
  } finally {
    updateCheck.disabled = false;
  }
}

updateCheck.addEventListener("click", () => checkForUpdate({ quiet: false }));
updateAuto.addEventListener("change", () => { try { window.localStorage.setItem("autoUpdate", updateAuto.checked ? "on" : "off"); } catch { /* storage unavailable */ } });
updateInstall.addEventListener("click", () => { if (pendingUpdate) openUpdateDialog(pendingUpdate); });

if (tauriInvoke) {
  tauriInvoke("get_app_version").then((version) => {
    appVersion = version;
    document.querySelector("#app-version").textContent = `v${version}`;
    updateState.textContent = `Phiên bản ${version}`;
    if (updateAuto.checked) window.setTimeout(() => checkForUpdate({ quiet: true }), 4000);
  }).catch(() => {});
}

// ---- clean-up: browser cache and generation history ----
const cleanInfo = document.querySelector("#clean-info");
const confirmDialog = document.querySelector("#confirm-dialog");
const cleanButtons = ["#clean-cache", "#clean-history", "#clean-all"].map((id) => document.querySelector(id));

function formatBytes(bytes) {
  if (bytes >= 1073741824) return `${(bytes / 1073741824).toFixed(1)} GB`;
  if (bytes >= 1048576) return `${Math.round(bytes / 1048576)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function askConfirm(title, text, okLabel) {
  return new Promise((resolve) => {
    document.querySelector("#confirm-title").textContent = title;
    document.querySelector("#confirm-text").textContent = text;
    const ok = document.querySelector("#confirm-ok");
    const cancel = document.querySelector("#confirm-cancel");
    ok.textContent = okLabel;
    const finish = (answer) => {
      ok.removeEventListener("click", onOk);
      cancel.removeEventListener("click", onCancel);
      confirmDialog.removeEventListener("close", onClose);
      if (confirmDialog.open) confirmDialog.close();
      resolve(answer);
    };
    const onOk = () => finish(true);
    const onCancel = () => finish(false);
    const onClose = () => finish(false);
    ok.addEventListener("click", onOk);
    cancel.addEventListener("click", onCancel);
    confirmDialog.addEventListener("close", onClose);
    confirmDialog.showModal();
  });
}

async function loadStorageInfo() {
  if (!tauriInvoke) { cleanInfo.textContent = "Chỉ khả dụng trong ứng dụng KMUSE trên Windows."; return null; }
  try {
    const info = await tauriInvoke("get_storage_info");
    cleanInfo.textContent = `Lịch sử: ${info.finished_jobs} việc · File đã tạo: ${info.result_files} (${formatBytes(info.result_bytes)}) · Cache trình duyệt: ${formatBytes(info.cache_bytes)}`
      + (info.cache_busy ? ` · ${info.cache_busy} cửa sổ đang mở` : "");
    return info;
  } catch (error) {
    cleanInfo.textContent = String(error);
    return null;
  }
}

async function runClean(command, args, title, text, okLabel, done) {
  if (!(await askConfirm(title, text, okLabel))) return;
  cleanButtons.forEach((b) => { b.disabled = true; });
  try {
    done(await tauriInvoke(command, args));
  } catch (error) {
    showToast(String(error));
  } finally {
    cleanButtons.forEach((b) => { b.disabled = false; });
    lastAccountsSnapshot = "";
    await loadStorageInfo();
  }
}

document.querySelector("#clean-cache").addEventListener("click", () => runClean(
  "clear_cache", {}, "Xóa cache trình duyệt?",
  "Xóa dữ liệu tạm của Chrome trong các hồ sơ tài khoản. Không mất đăng nhập. Hồ sơ nào đang mở cửa sổ sẽ được bỏ qua; lần mở sau Chrome có thể tải chậm hơn một chút.",
  "Xóa cache",
  (r) => showToast(`Đã giải phóng ${formatBytes(r.bytes_freed)} cache${r.skipped ? ` (bỏ qua ${r.skipped} tài khoản đang mở)` : ""}.`),
));

document.querySelector("#clean-history").addEventListener("click", () => runClean(
  "clear_history", { deleteFiles: false }, "Xóa lịch sử tạo?",
  "Danh sách các việc đã xong hoặc lỗi và ảnh thu nhỏ sẽ bị xóa. File ảnh/video vẫn giữ nguyên trong thư mục output. Việc đang chờ hoặc đang chạy không bị ảnh hưởng.",
  "Xóa lịch sử",
  (r) => showToast(`Đã xóa ${r.jobs_removed} mục lịch sử.`),
));

document.querySelector("#clean-all").addEventListener("click", () => runClean(
  "clear_history", { deleteFiles: true }, "Xóa cả file ảnh và video?",
  "Toàn bộ lịch sử VÀ các file ảnh/video đã tạo trong thư mục output sẽ bị xóa vĩnh viễn, không khôi phục được. Hãy sao lưu những file cần giữ trước. File do bạn tự bỏ vào output và file của việc đang chờ hoặc đang chạy sẽ được giữ lại.",
  "Xóa vĩnh viễn",
  (r) => showToast(`Đã xóa ${r.jobs_removed} mục lịch sử và ${r.files_removed} file (${formatBytes(r.bytes_freed)}).`),
));

document.querySelector('[data-tab="settings"]').addEventListener("click", () => { loadStorageInfo(); });
loadStorageInfo();
