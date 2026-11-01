import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";
import { chromium } from "playwright";

const DEFAULT_URL = "https://muse.ai/";
const ALLOWED_HOST = "muse.ai";

function printHelp() {
  console.log(`
Muse cookie session launcher (local only)

Usage:
  npm start -- --cookies <cookies.json>
  npm start -- --cookies <cookies.json> --profile <folder> --url https://muse.ai/

Options:
  --cookies <file>  JSON exported from your own browser (required on first run)
  --profile <dir>   Persistent browser profile (default: .muse-profile)
  --url <url>       Muse page to open (default: https://muse.ai/)
  --help            Show this help

Only cookies scoped to muse.ai are accepted. Cookie values are never printed.
`);
}

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (token === "--help" || token === "-h") args.help = true;
    else if (token === "--cookies") args.cookies = argv[++index];
    else if (token === "--profile") args.profile = argv[++index];
    else if (token === "--url") args.url = argv[++index];
    else throw new Error(`Unknown argument: ${token}`);
  }
  return args;
}

function isAllowedDomain(domain) {
  const normalized = String(domain ?? "")
    .trim()
    .toLowerCase()
    .replace(/^\./, "");
  return normalized === ALLOWED_HOST || normalized.endsWith(`.${ALLOWED_HOST}`);
}

function normalizeSameSite(value) {
  const normalized = String(value ?? "").toLowerCase();
  if (normalized === "strict") return "Strict";
  if (normalized === "lax") return "Lax";
  if (normalized === "none" || normalized === "no_restriction") return "None";
  return undefined;
}

export function normalizeCookies(payload) {
  const source = Array.isArray(payload) ? payload : payload?.cookies;
  if (!Array.isArray(source)) {
    throw new Error("Cookie file must be a JSON array or an object containing a cookies array.");
  }

  const accepted = [];
  let rejected = 0;

  for (const item of source) {
    if (!item || typeof item.name !== "string" || typeof item.value !== "string") {
      rejected += 1;
      continue;
    }

    const domain = item.domain || (() => {
      try {
        return item.url ? new URL(item.url).hostname : "";
      } catch {
        return "";
      }
    })();

    if (!isAllowedDomain(domain)) {
      rejected += 1;
      continue;
    }

    const cookie = {
      name: item.name,
      value: item.value,
      domain: String(domain).toLowerCase(),
      path: item.path || "/",
      httpOnly: Boolean(item.httpOnly),
      secure: item.secure !== false,
    };

    const sameSite = normalizeSameSite(item.sameSite);
    if (sameSite) cookie.sameSite = sameSite;

    const expiry = Number(item.expires ?? item.expirationDate);
    if (Number.isFinite(expiry) && expiry > 0) cookie.expires = Math.floor(expiry);

    accepted.push(cookie);
  }

  if (accepted.length === 0) {
    throw new Error("No valid muse.ai cookies were found. Other domains are intentionally rejected.");
  }

  return { accepted, rejected };
}

function validateTargetUrl(value) {
  const target = new URL(value || DEFAULT_URL);
  if (target.protocol !== "https:" || !isAllowedDomain(target.hostname)) {
    throw new Error("--url must be an HTTPS URL on muse.ai or one of its subdomains.");
  }
  return target.toString();
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    printHelp();
    return;
  }

  const profileDir = path.resolve(args.profile || ".muse-profile");
  const targetUrl = validateTargetUrl(args.url);
  const context = await chromium.launchPersistentContext(profileDir, {
    headless: false,
    viewport: null,
  });

  try {
    if (args.cookies) {
      const cookieFile = path.resolve(args.cookies);
      const raw = await fs.readFile(cookieFile, "utf8");
      const payload = JSON.parse(raw);
      const { accepted, rejected } = normalizeCookies(payload);
      await context.addCookies(accepted);
      console.log(`Imported ${accepted.length} muse.ai cookie(s); rejected ${rejected}.`);
    } else {
      console.log("No cookie file supplied; opening the saved local profile.");
    }

    const pages = context.pages();
    const page = pages[0] || (await context.newPage());
    await page.goto(targetUrl, { waitUntil: "domcontentloaded" });
    console.log("Browser opened. Close the browser window to stop the tool.");
    await new Promise((resolve) => context.once("close", resolve));
  } catch (error) {
    await context.close().catch(() => {});
    throw error;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(`Error: ${error.message}`);
    process.exitCode = 1;
  });
}
