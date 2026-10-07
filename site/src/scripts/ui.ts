// Site-wide behaviour: reveal-on-scroll and a zoomable lightbox for diagrams
// and images. Everything degrades to the static page without JS, and the
// reveal is skipped entirely under prefers-reduced-motion.

const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

// ---- reveal on scroll ------------------------------------------------------

const REVEAL = "main :is(h1, h2, h3, p, ul, ol, table, pre, blockquote, figure, img, .card, .term, .mermaid, .plots > a, #tiles, #pareto, #bars, #table, .toc)";

function setupReveal() {
  if (reduced || !("IntersectionObserver" in window)) return;
  const all = Array.from(document.querySelectorAll<HTMLElement>(REVEAL));
  // A block inside another block (a <p> in a .card) rides with its parent.
  const roots = all.filter((el) => !el.closest(".hero") && !all.some((o) => o !== el && o.contains(el)));
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      e.target.classList.add("shown");
      io.unobserve(e.target);
    }
  }, { rootMargin: "0px 0px -8% 0px", threshold: 0.05 });
  roots.forEach((el, i) => {
    el.classList.add("reveal");
    // Stagger siblings that enter together, a little, like lines filling a screen.
    const sib = roots.filter((o) => o.parentElement === el.parentElement);
    const k = sib.indexOf(el);
    el.style.transitionDelay = `${Math.min(k, 5) * 55}ms`;
    io.observe(el);
    void i;
  });
  // Anything already past the viewport bottom never animates in if the user
  // scrolls instantly to an anchor; show those on hashchange.
  const showAll = () => roots.forEach((el) => el.classList.add("shown"));
  window.addEventListener("hashchange", showAll);
  window.addEventListener("beforeprint", showAll);
}

// ---- lightbox ----------------------------------------------------------------

let dialog: HTMLDialogElement | null = null;
let stage: HTMLElement | null = null;
let inner: HTMLElement | null = null;
let scale = 1, tx = 0, ty = 0;

function apply() {
  if (inner) inner.style.transform = `translate(${tx}px, ${ty}px) scale(${scale})`;
  const z = dialog?.querySelector<HTMLElement>(".lb-zoom");
  if (z) z.textContent = `${Math.round(scale * 100)}%`;
}

function reset(fit = true) {
  scale = 1; tx = 0; ty = 0;
  if (fit && inner && stage) {
    const svg = inner.firstElementChild as SVGElement | HTMLElement | null;
    if (svg) {
      const r = svg.getBoundingClientRect();
      const w = stage.clientWidth - 32, h = stage.clientHeight - 32;
      if (r.width && r.height) scale = Math.min(w / r.width, h / r.height, 4);
    }
  }
  apply();
}

function ensureDialog() {
  if (dialog) return dialog;
  dialog = document.createElement("dialog");
  dialog.className = "lightbox";
  dialog.innerHTML = `
    <div class="lb-bar">
      <span class="dot r"></span><span class="dot y"></span><span class="dot g"></span>
      <span class="lb-title"></span>
      <span class="lb-hint">scroll to zoom · drag to pan · double-click to fit</span>
      <button type="button" class="lb-btn" data-act="out" aria-label="Zoom out">−</button>
      <span class="lb-zoom">100%</span>
      <button type="button" class="lb-btn" data-act="in" aria-label="Zoom in">+</button>
      <button type="button" class="lb-btn" data-act="fit" aria-label="Fit to window">fit</button>
      <button type="button" class="lb-btn" data-act="close" aria-label="Close">esc</button>
    </div>
    <div class="lb-stage"><div class="lb-inner"></div></div>`;
  document.body.append(dialog);
  stage = dialog.querySelector(".lb-stage");
  stage!.tabIndex = -1;
  inner = dialog.querySelector(".lb-inner");
  dialog.addEventListener("click", (ev) => {
    const b = (ev.target as HTMLElement).closest<HTMLElement>("[data-act]");
    if (b) {
      const act = b.dataset.act;
      if (act === "in") { scale = Math.min(scale * 1.25, 8); apply(); }
      else if (act === "out") { scale = Math.max(scale / 1.25, 0.1); apply(); }
      else if (act === "fit") reset();
      else if (act === "close") dialog!.close();
      return;
    }
    if (ev.target === stage && !moved) dialog!.close();
  });
  stage!.addEventListener("wheel", (ev) => {
    ev.preventDefault();
    const r = stage!.getBoundingClientRect();
    const px = ev.clientX - r.left - r.width / 2 - tx, py = ev.clientY - r.top - r.height / 2 - ty;
    const f = Math.exp(-ev.deltaY * 0.001);
    const ns = Math.min(Math.max(scale * f, 0.1), 8);
    tx -= px * (ns / scale - 1); ty -= py * (ns / scale - 1);
    scale = ns; apply();
  }, { passive: false });
  let drag: { x: number; y: number; tx: number; ty: number } | null = null;
  let moved = false;
  stage!.addEventListener("pointerdown", (ev) => {
    if ((ev.target as HTMLElement).closest("[data-act]")) return;
    drag = { x: ev.clientX, y: ev.clientY, tx, ty }; moved = false;
    stage!.setPointerCapture(ev.pointerId);
    stage!.classList.add("dragging");
  });
  stage!.addEventListener("pointermove", (ev) => {
    if (!drag) return;
    if (Math.abs(ev.clientX - drag.x) + Math.abs(ev.clientY - drag.y) > 4) moved = true;
    tx = drag.tx + ev.clientX - drag.x; ty = drag.ty + ev.clientY - drag.y; apply();
  });
  const end = () => { drag = null; stage!.classList.remove("dragging"); };
  stage!.addEventListener("pointerup", end);
  stage!.addEventListener("pointercancel", end);
  stage!.addEventListener("dblclick", () => reset());
  dialog.addEventListener("close", () => { if (inner) inner.innerHTML = ""; });
  return dialog;
}

export function openLightbox(content: Element, title: string) {
  const d = ensureDialog();
  inner!.innerHTML = "";
  const clone = content.cloneNode(true) as Element;
  if (clone instanceof SVGElement) {
    clone.removeAttribute("style");
    clone.style.maxWidth = "none";
    clone.style.height = "auto";
  }
  inner!.append(clone);
  d.querySelector(".lb-title")!.textContent = title;
  d.showModal();
  stage!.focus();
  requestAnimationFrame(() => reset());
}

function expandButton(target: Element, title: string) {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "expand";
  b.textContent = "⤢ expand";
  b.setAttribute("aria-label", `Expand ${title}`);
  b.addEventListener("click", (ev) => { ev.preventDefault(); ev.stopPropagation(); openLightbox(target, title); });
  return b;
}

function decorateMermaid(node: Element) {
  if (node.classList.contains("expandable")) return;
  const svg = node.querySelector("svg");
  if (!svg) return;
  node.classList.add("expandable");
  const title = node.closest("section, .doc, main")?.querySelector("h1, h2")?.textContent?.trim() ?? "diagram";
  node.append(expandButton(svg, `${title} diagram`));
  node.addEventListener("click", () => openLightbox(svg, `${title} diagram`));
}

function decorateImages() {
  for (const img of document.querySelectorAll<HTMLImageElement>("main img")) {
    const a = img.closest("a");
    const wrap = a ?? img;
    if (wrap.classList.contains("expandable")) continue;
    wrap.classList.add("expandable");
    const title = img.alt || "image";
    const open = (ev: Event) => { ev.preventDefault(); openLightbox(img, title); };
    if (a) a.addEventListener("click", open); else img.addEventListener("click", open);
  }
}

setupReveal();
decorateImages();
document.querySelectorAll(".mermaid").forEach(decorateMermaid);
document.addEventListener("mermaid:rendered", (ev) => decorateMermaid(ev.target as Element));
document.addEventListener("keydown", (ev) => {
  if (!dialog?.open) return;
  if (ev.key === "+" || ev.key === "=") { scale = Math.min(scale * 1.25, 8); apply(); }
  if (ev.key === "-") { scale = Math.max(scale / 1.25, 0.1); apply(); }
  if (ev.key === "0") reset();
});
