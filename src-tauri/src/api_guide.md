# Muse Tool local API — guide for AI agents

You can generate images and videos through Muse Tool, a desktop app running on THIS machine.
Base URL: __BASE__   (loopback only, no API key, plain HTTP, JSON in and out)
This guide: __BASE__/docs

The app owns several logged-in Muse accounts. Each account runs ONE job at a time, so N accounts
means up to N jobs in parallel. Jobs you submit wait in a queue and are handed to free accounts
automatically. Browser windows are opened, hidden and closed by the app itself: never open Muse yourself.

## Quick start

1. Check capacity:      GET  __BASE__/accounts
2. Submit work:         POST __BASE__/jobs         (many prompts in ONE request)
3. Wait for results:    GET  __BASE__/jobs/{id}?wait=120   (blocks until done, up to 600 s)
4. Download the file:   GET  __BASE__/jobs/{id}/file       (or read "file" = local path)

## POST /jobs

Body (JSON). Either one item or {"jobs": [item, item, ...]}:

    {
      "jobs": [
        {"kind": "image", "prompt": "a red fox in the snow"},
        {"kind": "video", "prompt": "waves rolling onto a beach at sunset"},
        {"kind": "image", "prompts": ["prompt A", "prompt B", "prompt C"], "ratio": "9:16"}
      ]
    }

Item fields:
- kind            "image" (default) or "video"
- prompt          one prompt (max 4000 characters)
- prompts         array of prompts: each becomes its own job with the same settings
- ratio           "16:9" (default), "9:16" or "1:1"
- quantity        1-50 copies of each prompt (default 1)
- account         optional: account name/email or id. Omit or "auto" = any free account (recommended)
- reference_job   optional: id of a FINISHED image job whose picture is used as the reference
- reference_path  optional: path of a PNG/JPG/WEBP inside the output folder (see below) used as the reference

Response 201: {"ids":[12,13,14], "jobs":[{"id":12,"kind":"image","prompt":"..."}, ...]}
Response 207: some items were created, "error" says why the rest were not.
Response 400: nothing was created, "error" explains.

Always send many prompts in a single request instead of many small requests.
The app adds its own wording to each prompt (style: no text, no watermark; videos are about 10 seconds),
so describe only the scene and the motion.

## Reference image -> video

To animate a picture, first create the image, wait until it is done, then submit a video that points at it:

    POST __BASE__/jobs  {"kind":"video","prompt":"the boat drifts, ripples spread","reference_job":12}

To use an image you already have, copy it into the output folder and use reference_path:
    output folder: __OUT__
    {"kind":"video","prompt":"...","reference_path":"__OUTJSON__\\my-picture.png"}
Files outside the output folder are refused. Max 20 MB.

## GET /jobs/{id}

Fields: id, kind, prompt, ratio, status, message, file, file_url, reference, account, started_ms, finished_ms.
status is one of:
- waiting   queued, no account yet
- running   an account is generating it
- done      finished: "file" is the local path, "file_url" downloads it (images .webp/.png, videos .mp4)
- error     failed after retries: read "message"

?wait=N holds the request open until the job is done/error or N seconds pass (max 600). Use it instead of a polling loop.
Typical times: image 30-60 s, video 60-120 s (up to 10 minutes in bad cases). When many jobs run, expect more.
Failed jobs are retried automatically (up to 3 attempts, on other accounts when not pinned). Do not resubmit before status is "error".

## Other endpoints

- GET    __BASE__/jobs?limit=50      newest jobs first (max 500)
- GET    __BASE__/jobs/{id}/file     the picture/video bytes (Content-Type set)
- DELETE __BASE__/jobs/{id}          cancel a job that is still "waiting" (running jobs cannot be cancelled)
- GET    __BASE__/accounts           [{id, name, status, connected, busy, usage, tokens_left}]
- POST   __BASE__/accounts/refresh   re-read every account's remaining usage in the background (takes a few minutes)
- GET    __BASE__/health             {"ok":true, ...}

"connected": true means the account can take jobs now. "busy": true means it is running one.
"usage" / "tokens_left" are Muse's own text (for example "Free plan · 52% used" and "44% used (558M tokens left)").
An account with 0 tokens left fails its jobs with an error starting "Hết lượt" (out of usage); unpinned jobs move to other accounts.

## Recommended workflow

    # 1. see how many accounts can work
    curl __BASE__/accounts
    # 2. submit all prompts at once (Windows cmd/PowerShell: put the JSON in a file and use -d @jobs.json)
    curl -X POST __BASE__/jobs -H "Content-Type: application/json" -d @jobs.json
    # 3. for each returned id, wait and collect
    curl "__BASE__/jobs/12?wait=300"
    curl -o fox.webp __BASE__/jobs/12/file

Python:

    import requests, time
    B = "__BASE__"
    r = requests.post(f"{B}/jobs", json={"jobs": [{"kind": "image", "prompt": p} for p in prompts]}).json()
    for jid in r["ids"]:
        job = requests.get(f"{B}/jobs/{jid}", params={"wait": 300}).json()
        if job["status"] == "done":
            open(f"{jid}.webp", "wb").write(requests.get(job["file_url"]).content)
        else:
            print(jid, job["status"], job.get("message"))

## Rules and limits

- Only this machine can call the API (127.0.0.1). Requests that carry a browser Origin header are refused.
- One job per account at a time; new jobs start about 2 seconds apart. Submitting 100 prompts is fine: they queue.
- The app may be closed by the user; jobs that were running then are marked "error" (message says interrupted) and must be resubmitted.
- Results are saved in the output folder (__OUT__) and also returned by /file.
- Do not call Muse directly and do not open new Muse chats: the app already does it safely.
