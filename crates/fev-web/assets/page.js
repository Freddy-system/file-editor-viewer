const dark = () =>
  document.documentElement.dataset.theme
    ? document.documentElement.dataset.theme === "dark"
    : matchMedia("(prefers-color-scheme: dark)").matches;

const failed = (el, source, error) => {
  el.classList.add("failed");
  el.innerHTML = "";
  const note = document.createElement("p");
  note.className = "diagram-error";
  note.textContent = String(error && error.message ? error.message : error);
  const pre = document.createElement("pre");
  pre.textContent = source;
  el.append(note, pre);
};

async function renderMermaid(root) {
  const blocks = [...root.querySelectorAll(".diagram[data-kind=mermaid]:not([data-done])")];
  if (!blocks.length) return;
  const { default: mermaid } = await import(FEV_CDN.mermaid);
  mermaid.initialize({ startOnLoad: false, theme: dark() ? "dark" : "default", securityLevel: "strict" });
  let n = 0;
  for (const el of blocks) {
    const source = el.textContent;
    el.dataset.done = "1";
    try {
      const { svg } = await mermaid.render(`fev-mermaid-${Date.now()}-${n++}`, source);
      el.innerHTML = svg;
    } catch (error) {
      failed(el, source, error);
    }
  }
}

async function renderGraphviz(root) {
  const blocks = [...root.querySelectorAll(".diagram[data-kind=graphviz]:not([data-done])")];
  if (!blocks.length) return;
  const { instance } = await import(FEV_CDN.viz);
  const viz = await instance();
  for (const el of blocks) {
    const source = el.textContent;
    el.dataset.done = "1";
    try {
      el.replaceChildren(viz.renderSVGElement(source));
    } catch (error) {
      failed(el, source, error);
    }
  }
}

function renderMath(root) {
  if (!window.katex) return;
  for (const el of root.querySelectorAll(".math:not([data-done])")) {
    const source = el.textContent;
    el.dataset.done = "1";
    try {
      katex.render(source, el, { displayMode: el.classList.contains("math-display"), throwOnError: false });
    } catch (error) {
      el.textContent = source;
    }
  }
}

function renderCode(root) {
  if (!window.hljs) return;
  for (const el of root.querySelectorAll("pre code[class*=language-]:not([data-done])")) {
    el.dataset.done = "1";
    hljs.highlightElement(el);
  }
}

function render(root = document) {
  renderMath(root);
  renderCode(root);
  renderMermaid(root).catch(console.error);
  renderGraphviz(root).catch(console.error);
}

// Live reload: swap the document in place so the scroll position survives.
async function watch(stampUrl) {
  let stamp = null;
  for (;;) {
    await new Promise((resolve) => setTimeout(resolve, 700));
    try {
      const current = await (await fetch(stampUrl, { cache: "no-store" })).text();
      if (stamp !== null && current !== stamp) {
        const html = await (await fetch(location.href, { cache: "no-store" })).text();
        const next = new DOMParser().parseFromString(html, "text/html");
        for (const selector of ["main", "nav.toc"]) {
          const from = next.querySelector(selector);
          const to = document.querySelector(selector);
          if (from && to) to.replaceWith(from);
        }
        document.title = next.title;
        render();
      }
      stamp = current;
    } catch (error) {
      // The server went away with the viewer; keep the last render on screen.
    }
  }
}

addEventListener("DOMContentLoaded", () => {
  render();
  const stampUrl = document.body.dataset.stamp;
  if (stampUrl) watch(stampUrl);
});
