(async (kind) => {
  const sel = kind === 'video' ? '[aria-label^="Open animated video"]' : '[aria-label^="Open generated image"]';
  const nodes = [...document.querySelectorAll(sel)];
  const el = nodes[nodes.length - 1];
  if (!el) return { error: 'no-element' };
  const src = kind === 'video' ? el.getAttribute('data-hatch-video-src') : (el.querySelector('img') || {}).src;
  if (!src) return { error: 'no-src', html: el.outerHTML.slice(0, 300) };
  const blob = await (await fetch(src)).blob();
  const b64 = await new Promise((res) => { const r = new FileReader(); r.onload = () => res(String(r.result).split(',')[1]); r.readAsDataURL(blob); });
  let poster = null;
  const psrc = kind === 'video' ? el.getAttribute('data-hatch-video-poster') : null;
  if (psrc) { try { const pb = await (await fetch(psrc)).blob(); poster = await new Promise((res) => { const r = new FileReader(); r.onload = () => res(String(r.result)); r.readAsDataURL(pb); }); } catch (e) {} }
  return { b64, type: blob.type, label: el.getAttribute('aria-label'), poster };
})
