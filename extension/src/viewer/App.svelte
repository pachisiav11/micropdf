<script lang="ts">
  import { onMount, tick } from "svelte";
  import { openLocal, sendPdf } from "../bridge";
  import { fileName, localPath, pdfSource, startPage } from "../rules";
  import { loadSettings } from "../settings";
  import {
    DEFAULT_COLORS,
    type Markup,
    SWATCHES,
    TOOLS,
    type Tool,
    css,
    draft,
    isDraw,
    isMarkup,
    needsText,
    spansOf,
  } from "./annotate";
  import Draw from "./Draw.svelte";
  import { type Command, mapKey, vimState } from "./keys";
  import {
    LINE,
    PX_PER_POINT,
    type PageMode,
    type Rect,
    type Zoom,
    computeLayout,
    currentPage,
    frameToPage,
    hit,
    scrollTo,
    toView,
    wheelZoom,
    zoomStep,
  } from "./layout";
  import Page from "./Page.svelte";
  import {
    type AnnotInfo,
    Dropped,
    type Edited,
    type FieldInfo,
    type Hit,
    type Opened,
    type OutlineNode,
    type PageSize,
    Pdf,
    type Point,
    type Rgb,
    type Target,
  } from "./pdf";
  import type { ReadingMode } from "./recolor";
  import Sidebar from "./Sidebar.svelte";

  interface Prefs {
    mode: PageMode;
    reading: ReadingMode;
    vim: boolean;
    sidebar: boolean;
    tab: "pages" | "outline";
    /** The colours the reader chose per comment tool. */
    colors: Partial<Record<Tool, Rgb>>;
  }

  /** Where the reader was: a page, how far down it, and the middle across the column. */
  interface Spot {
    page: number;
    frac: number;
    xFrac: number;
  }

  const PREFS = "micropdf.viewer";
  const HISTORY_LIMIT = 100;

  function loadPrefs(): Partial<Prefs> {
    try {
      return JSON.parse(localStorage.getItem(PREFS) ?? "{}") as Partial<Prefs>;
    } catch {
      return {};
    }
  }

  const pdf = new Pdf();
  const prefs = loadPrefs();

  let name = $state("micropdf");
  let pages: PageSize[] = $state.raw([]);
  /** Shown instead of the pages while loading or after a failure. */
  let message = $state("Loading…");
  let locked = $state(false);
  let password = $state("");
  let outline: OutlineNode[] = $state.raw([]);

  let mode: PageMode = $state(prefs.mode ?? "continuous");
  let reading: ReadingMode = $state(prefs.reading ?? "normal");
  let vim = $state(prefs.vim ?? false);
  let sidebar = $state(prefs.sidebar ?? false);
  let tab: "pages" | "outline" = $state(prefs.tab ?? "pages");
  let zoom: Zoom = $state("fit-width");
  let rotation = $state(0);
  /** The page shown in single mode. */
  let single = $state(0);
  let presenting = $state(false);
  let beforePresenting: { mode: PageMode; zoom: Zoom; sidebar: boolean } | null = null;

  let commenting = $state(false);
  let tool: Tool = $state("select");
  let colors: Partial<Record<Tool, Rgb>> = $state(prefs.colors ?? {});
  /** Per page, raised by each edit to it, so it draws again. */
  let versions: number[] = $state.raw([]);
  let picked: (AnnotInfo & { page: number }) | null = $state(null);
  let canUndo = $state(false);
  let canRedo = $state(false);
  let dirty = $state(false);
  let saveTo: FileSystemFileHandle | null = null;
  /** The PDF's URL: on the web, or file:// on this computer. */
  let source = "";
  let printFrame: HTMLIFrameElement | null = null;
  let author = $state("");
  let notice = $state("");
  let noticeTimer = 0;
  let asking: { title: string; text: string; resolve: (text: string | null) => void } | null = $state(null);
  let dialog: HTMLDialogElement | undefined = $state();
  let column: HTMLElement | undefined = $state();

  let view: HTMLElement | undefined = $state();
  // The content box, unlike clientWidth's border box, shrinks when a scrollbar appears.
  let box: DOMRectReadOnly | undefined = $state();
  // Until the first observation, which can come after a small PDF has opened, ask the element.
  const viewWidth = $derived(box?.width ?? view?.clientWidth ?? 0);
  const viewHeight = $derived(box?.height ?? view?.clientHeight ?? 0);
  let scrollTop = $state(0);
  let scrollLeft = $state(0);
  let dpr = $state(devicePixelRatio);

  const layout = $derived(
    computeLayout({ sizes: pages, rotation, mode, zoom, viewWidth, viewHeight, current: single }),
  );
  const current = $derived(
    mode === "single" ? Math.min(single, Math.max(pages.length - 1, 0)) : currentPage(layout, scrollTop, viewHeight),
  );

  let finding = $state(false);
  let needle = $state("");
  /** Search hits per page; undefined for pages not searched yet. */
  let found: (Hit[] | undefined)[] = $state.raw([]);
  let searched = $state(0);
  let hitAt: { page: number; index: number } | null = $state(null);
  let searchRun = 0;
  let searchTimer = 0;
  const total = $derived(found.reduce((n, h) => n + (h?.length ?? 0), 0));
  const ordinal = $derived.by(() => {
    if (!hitAt) return 0;
    let n = hitAt.index + 1;
    for (let p = 0; p < hitAt.page; p++) n += found[p]?.length ?? 0;
    return n;
  });

  let findInput: HTMLInputElement | undefined = $state();
  let pageInput: HTMLInputElement | undefined = $state();
  let menu: HTMLElement | undefined = $state();
  let menuButton: HTMLElement | undefined = $state();

  const keys = vimState();
  const back: Spot[] = [];
  let ahead: Spot[] = [];

  $effect(() => {
    if (presenting) return;
    try {
      localStorage.setItem(PREFS, JSON.stringify({ mode, reading, vim, sidebar, tab, colors } satisfies Prefs));
    } catch {
      // Storage is off; the settings last for this page.
    }
  });

  // Browser zoom and moving to another screen change the device pixel ratio.
  $effect(() => {
    const query = matchMedia(`(resolution: ${dpr}dppx)`);
    const update = () => (dpr = devicePixelRatio);
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  });

  async function show(opened: Opened, start: number | null): Promise<void> {
    locked = opened.needsPassword;
    pages = opened.pages;
    versions = pages.map(() => 0);
    message = locked || pages.length ? "" : "The PDF has no pages.";
    if (opened.title.trim()) document.title = opened.title.trim();
    if (locked || !pages.length) return;
    pdf.outline().then(
      (o) => (outline = o),
      () => (outline = []),
    );
    if (start !== null) await goPage(start);
  }

  /** Reads the PDF; fetch cannot read file:// URLs, which XMLHttpRequest can with file access. */
  async function load(url: string): Promise<ArrayBuffer> {
    if (!/^file:/i.test(url)) {
      const response = await fetch(url, { credentials: "include" });
      if (!response.ok) throw new Error(`the server answered ${response.status}`);
      return response.arrayBuffer();
    }
    return new Promise((resolve, reject) => {
      const request = new XMLHttpRequest();
      request.open("GET", url);
      request.responseType = "arraybuffer";
      request.onload = () => resolve(request.response as ArrayBuffer);
      request.onerror = () => reject(new Error("the browser did not let micropdf read the file"));
      request.send();
    });
  }

  onMount(async () => {
    const src = pdfSource(location.href);
    if (!src) {
      message = "Open a link to a PDF to view it here.";
      return;
    }
    source = src;
    name = fileName(src);
    document.title = name;
    const settings = await loadSettings();
    author = settings.author;
    try {
      await show(await pdf.open(await load(src)), startPage(src));
    } catch (e) {
      message = `Could not open the PDF: ${e instanceof Error ? e.message : e}`;
      return;
    }
    if (settings.open === "app" && pages.length) void handOff(true);
  });

  // The extension's toolbar button asks the viewer in its tab to hand its PDF on.
  $effect(() => {
    const messages = typeof chrome === "undefined" ? undefined : chrome.runtime?.onMessage;
    if (!messages) return;
    const listener = (m: { type?: string }) => void (m.type === "open-in-app" && handOff());
    messages.addListener(listener);
    return () => messages.removeListener(listener);
  });

  async function unlock(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    const opened = await pdf.unlock(password);
    if (opened) await show(opened, startPage(pdfSource(location.href) ?? ""));
    else message = "That password is not right.";
  }

  function syncScroll(): void {
    if (!view) return;
    scrollTop = view.scrollTop;
    scrollLeft = view.scrollLeft;
  }

  function scrollBy(dx: number, dy: number): void {
    view?.scrollBy({ left: dx, top: dy });
    syncScroll();
  }

  function spot(): Spot | null {
    if (!view || !pages.length) return null;
    const page = mode === "single" ? current : currentPage(layout, view.scrollTop, 0);
    const f = layout.frames[page];
    if (!f) return null;
    return {
      page,
      frac: (view.scrollTop - f.y) / f.height,
      xFrac: (view.scrollLeft + viewWidth / 2) / layout.width,
    };
  }

  async function goSpot(s: Spot): Promise<void> {
    if (mode === "single") single = s.page;
    await tick();
    const f = layout.frames[s.page];
    if (!f || !view) return;
    view.scrollTop = f.y + s.frac * f.height;
    view.scrollLeft = s.xFrac * layout.width - viewWidth / 2;
    syncScroll();
  }

  /** Applies a change to the layout, keeping the spot at the top of the view in place, or
   * the point under `anchor` (view pixels) when given. */
  async function relayout(change: () => void, anchor?: [number, number]): Promise<void> {
    if (!view || !pages.length) return change();
    if (!anchor) {
      const s = spot();
      change();
      if (s) await goSpot(s);
      return;
    }
    const [x, y] = [view.scrollLeft + anchor[0], view.scrollTop + anchor[1]];
    const page = hit(layout, x, y)?.[0] ?? currentPage(layout, y, 0);
    const f = layout.frames[page];
    change();
    await tick();
    const g = layout.frames[page];
    if (!f || !g) return;
    view.scrollLeft = g.x + ((x - f.x) / f.width) * g.width - anchor[0];
    view.scrollTop = g.y + ((y - f.y) / f.height) * g.height - anchor[1];
    syncScroll();
  }

  async function goPage(page: number, top?: number): Promise<void> {
    if (!pages.length) return;
    page = Math.min(Math.max(page, 0), pages.length - 1);
    if (mode === "single") single = page;
    await tick();
    const y = scrollTo(layout, page, top);
    if (y !== null && view) {
      view.scrollTop = y;
      syncScroll();
    }
  }

  function remember(): void {
    const s = spot();
    if (!s) return;
    back.push(s);
    if (back.length > HISTORY_LIMIT) back.shift();
    ahead = [];
  }

  async function travel(from: Spot[], to: Spot[]): Promise<void> {
    const s = from.pop();
    if (!s) return;
    const here = spot();
    if (here) to.push(here);
    await goSpot(s);
  }

  /** Follows a link or bookmark. */
  function go(target: Target): void {
    if (target.uri) {
      window.open(target.uri, "_blank", "noopener,noreferrer");
    } else if (target.page !== undefined) {
      remember();
      void goPage(target.page, target.top);
    }
  }

  /** The first page of the row `n` rows away from the current page's. */
  function rowAway(n: number): number {
    if (mode === "single") return current + n;
    const row = layout.rows.findIndex((r) => r.includes(current));
    const to = Math.min(Math.max(row + n, 0), layout.rows.length - 1);
    return layout.rows[to][0];
  }

  async function pageScroll(forward: boolean): Promise<void> {
    if (!view) return;
    const atBottom = view.scrollTop + view.clientHeight >= view.scrollHeight - 1;
    if (mode === "single" && forward && atBottom) {
      await goPage(current + 1);
    } else if (mode === "single" && !forward && view.scrollTop <= 0 && current > 0) {
      single = current - 1;
      await tick();
      view.scrollTop = view.scrollHeight;
      syncScroll();
    } else {
      const step = Math.max(viewHeight - 40, viewHeight / 2);
      scrollBy(0, forward ? step : -step);
    }
  }

  function setMode(next: PageMode): void {
    void relayout(() => {
      if (next === "single") single = current;
      mode = next;
    });
  }

  function setZoom(next: Zoom, anchor?: [number, number]): void {
    void relayout(() => (zoom = next), anchor);
  }

  function rotate(by: number): void {
    void relayout(() => (rotation = (rotation + by + 360) % 360));
  }

  async function startPresenting(): Promise<void> {
    beforePresenting = { mode, zoom, sidebar };
    await relayout(() => {
      single = current;
      mode = "single";
      zoom = "fit-page";
      sidebar = false;
      presenting = true;
    });
    await document.documentElement.requestFullscreen().catch(() => {});
  }

  function stopPresenting(): void {
    const before = beforePresenting;
    beforePresenting = null;
    if (!presenting || !before) return;
    if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
    void relayout(() => {
      presenting = false;
      mode = before.mode;
      zoom = before.zoom;
      sidebar = before.sidebar;
    });
  }

  async function openFind(): Promise<void> {
    finding = true;
    await tick();
    findInput?.focus();
    findInput?.select();
  }

  function closeFind(): void {
    finding = false;
    searchRun++;
    found = [];
    hitAt = null;
  }

  function searchSoon(): void {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void search(needle), 200);
  }

  /** Searches every page, starting at the current one, so the first hits come soonest. */
  async function search(text: string): Promise<void> {
    const run = ++searchRun;
    found = [];
    searched = 0;
    hitAt = null;
    const query = text.trim();
    if (!query || !pages.length) return;
    const count = pages.length;
    const start = current;
    const results: (Hit[] | undefined)[] = new Array(count);
    for (let k = 0; k < count; k++) {
      const page = (start + k) % count;
      try {
        results[page] = await pdf.search(page, query, () => run === searchRun);
      } catch (e) {
        if (e instanceof Dropped) return;
        results[page] = [];
      }
      if (run !== searchRun) return;
      searched = k + 1;
      const hits = results[page]!.length;
      if (hits || k % 16 === 15 || k === count - 1) found = results.slice();
      if (!hitAt && hits) {
        hitAt = { page, index: 0 };
        await reveal();
      }
    }
  }

  function step(forward: boolean): void {
    if (!total) return;
    const count = pages.length;
    let { page, index } = hitAt ?? { page: current, index: forward ? -1 : (found[current]?.length ?? 0) };
    index += forward ? 1 : -1;
    while (index < 0 || index >= (found[page]?.length ?? 0)) {
      page = (page + (forward ? 1 : -1) + count) % count;
      index = forward ? 0 : (found[page]?.length ?? 0) - 1;
    }
    hitAt = { page, index };
    void reveal();
  }

  async function reveal(): Promise<void> {
    const at = hitAt;
    const rect = at && found[at.page]?.[at.index]?.[0];
    if (!at || !rect || !view) return;
    if (mode === "single") single = at.page;
    await tick();
    const v = toView(layout, at.page, rect);
    if (!v) return;
    if (v.y < view.scrollTop || v.y + v.height > view.scrollTop + viewHeight) {
      view.scrollTop = v.y - viewHeight / 3;
    }
    if (v.x < view.scrollLeft || v.x + v.width > view.scrollLeft + viewWidth) {
      view.scrollLeft = v.x - viewWidth / 2;
    }
    syncScroll();
  }

  function findKey(e: KeyboardEvent): void {
    if (e.key === "Enter") {
      e.preventDefault();
      clearTimeout(searchTimer);
      if (!found.length && needle.trim()) void search(needle);
      else step(!e.shiftKey);
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeFind();
    }
  }

  function pageKey(e: KeyboardEvent): void {
    if (e.key === "Enter") {
      const n = Number.parseInt(pageInput?.value ?? "", 10);
      if (Number.isFinite(n)) {
        remember();
        void goPage(n - 1);
      }
      pageInput?.blur();
    } else if (e.key === "Escape") {
      if (pageInput) pageInput.value = String(current + 1);
      pageInput?.blur();
    }
  }

  function say(text: string): void {
    notice = text;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = ""), 4000);
  }

  const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const colorOf = (t: Exclude<Tool, "select">) => colors[t] ?? DEFAULT_COLORS[t];

  /** Waits for an edit, then draws again the pages it changed. */
  async function apply(change: Promise<Edited>, what: string): Promise<boolean> {
    try {
      const done = await change;
      versions = versions.map((v, i) => (!done.pages || done.pages.includes(i) ? v + 1 : v));
      canUndo = done.canUndo;
      canRedo = done.canRedo;
      dirty = true;
      return true;
    } catch (e) {
      say(`Could not ${what}: ${reason(e)}`);
      return false;
    }
  }

  /** Asks for a comment's text; null when the reader cancels. */
  async function ask(title: string, text = ""): Promise<string | null> {
    const answer = new Promise<string | null>((resolve) => (asking = { title, text, resolve }));
    await tick();
    dialog?.showModal();
    return answer;
  }

  function answer(ok: boolean): void {
    const a = asking;
    asking = null;
    if (dialog?.open) dialog.close();
    a?.resolve(ok ? a.text : null);
  }

  async function drawn(page: number, points: Point[]): Promise<void> {
    if (!isDraw(tool)) return;
    const t = tool;
    if (!draft(t, points, colorOf(t), "")) {
      say(t === "text" ? "Drag to draw the text box" : "Drag to draw the shape");
      return;
    }
    const text = needsText(t) ? await ask(t === "note" ? "Add a note" : "Add a text box") : "";
    if (text === null || (needsText(t) && !text.trim())) return;
    await apply(pdf.addAnnot(page, draft(t, points, colorOf(t), text)!, author), "add the comment");
  }

  /** The selected part of each line of the text layer, in page space, by page. */
  function selectedLines(): Map<number, Rect[]> {
    const lines = new Map<number, Rect[]>();
    const selection = getSelection();
    if (!column || !selection?.rangeCount || selection.isCollapsed) return lines;
    const range = selection.getRangeAt(0);
    const origin = column.getBoundingClientRect();
    for (const span of column.querySelectorAll<HTMLElement>(".text span")) {
      const text = span.firstChild;
      if (!text || !range.intersectsNode(span)) continue;
      const part = document.createRange();
      part.selectNodeContents(text);
      if (span.contains(range.startContainer)) part.setStart(range.startContainer, range.startOffset);
      if (span.contains(range.endContainer)) part.setEnd(range.endContainer, range.endOffset);
      const r = part.getBoundingClientRect();
      const page = Number(span.closest<HTMLElement>("[data-page]")?.dataset.page);
      const f = layout.frames[page];
      if (!r.width || !f) continue;
      const [ax, ay] = frameToPage(layout, page, r.left - origin.left - f.x, r.top - origin.top - f.y);
      const [bx, by] = frameToPage(layout, page, r.right - origin.left - f.x, r.bottom - origin.top - f.y);
      const rect = { x0: Math.min(ax, bx), y0: Math.min(ay, by), x1: Math.max(ax, bx), y1: Math.max(ay, by) };
      lines.set(page, [...(lines.get(page) ?? []), rect]);
    }
    return lines;
  }

  /** Marks the selected text; false when none is selected. */
  async function markup(kind: Markup): Promise<boolean> {
    const lines = selectedLines();
    if (!lines.size) return false;
    getSelection()?.removeAllRanges();
    for (const [page, rects] of lines) {
      const annot = { kind, spans: spansOf(rects), color: colorOf(kind) };
      await apply(pdf.addAnnot(page, annot, author), "mark the text");
    }
    return true;
  }

  async function choose(t: Tool): Promise<void> {
    // With text selected, a markup button marks it, as in Acrobat.
    if (isMarkup(t) && (await markup(t))) return;
    tool = t;
    picked = null;
  }

  function setColor(c: Rgb): void {
    if (tool === "select" && picked && !picked.locked) {
      void apply(pdf.editAnnot(picked.page, picked.id, { color: c }), "change the colour");
    } else if (tool !== "select") {
      colors = { ...colors, [tool]: c };
    }
  }

  /** Picks the comment under a click with the Select tool. */
  async function pick(e: MouseEvent): Promise<void> {
    if (tool !== "select" || !column || !getSelection()?.isCollapsed) return;
    if (e.target instanceof Element && e.target.closest("a, button, input, select, textarea, .card")) return;
    const r = column.getBoundingClientRect();
    const at = hit(layout, e.clientX - r.left, e.clientY - r.top);
    const info = at ? await pdf.annotAt(at[0], [at[1], at[2]]).catch(() => null) : null;
    picked = at && info ? { ...info, page: at[0] } : null;
  }

  async function editPicked(): Promise<void> {
    const p = picked;
    if (!p || p.locked) return;
    getSelection()?.removeAllRanges();
    const text = await ask("Edit the comment", p.contents);
    if (text === null) return;
    if (await apply(pdf.editAnnot(p.page, p.id, { contents: text }), "change the comment")) {
      picked = { ...p, contents: text };
    }
  }

  async function deletePicked(): Promise<void> {
    const p = picked;
    if (!p || p.locked) return;
    picked = null;
    await apply(pdf.deleteAnnot(p.page, p.id), "delete the comment");
  }

  // Not held back by canUndo and canRedo: they lag behind edits still in the worker's queue.
  function history(redo: boolean): void {
    picked = null;
    void apply(redo ? pdf.redo() : pdf.undo(), redo ? "redo" : "undo");
  }

  function fill(page: number, field: FieldInfo, value?: string): void {
    const change = value === undefined ? pdf.toggleField(page, field.id) : pdf.setField(page, field.id, value);
    void apply(change, "fill in the field");
  }

  /** Writes the PDF with every change: a local PDF in place, through micropdf-bridge; a web one
   * to a file the reader picks once, then to the same file. */
  async function save(saveAs = false): Promise<void> {
    if (!pages.length) return;
    const local = saveAs ? null : localPath(source);
    const w = window as { showSaveFilePicker?: (o: object) => Promise<FileSystemFileHandle> };
    const suggestedName = /\.pdf$/i.test(name) ? name : `${name}.pdf`;
    const types = [{ description: "PDF document", accept: { "application/pdf": [".pdf"] } }];
    try {
      // The file first: a cancelled dialog leaves the document and its history alone.
      if (!local && (saveAs || !saveTo) && w.showSaveFilePicker) {
        saveTo = await w.showSaveFilePicker({ suggestedName, types });
      }
      const saved = await pdf.save();
      canUndo = saved.canUndo;
      canRedo = saved.canRedo;
      if (local) {
        await sendPdf(saved.bytes, { target: local });
      } else if (saveTo) {
        const out = await saveTo.createWritable();
        await out.write(saved.bytes);
        await out.close();
      } else {
        const a = document.createElement("a");
        a.href = URL.createObjectURL(new Blob([saved.bytes], { type: "application/pdf" }));
        a.download = suggestedName;
        a.click();
        setTimeout(() => URL.revokeObjectURL(a.href), 60_000);
      }
      dirty = false;
      say(`Saved ${local ?? saveTo?.name ?? suggestedName}`);
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") return;
      say(`Could not save${local ? " in place" : ""}: ${reason(e)}${local ? ". Save as (Ctrl+Shift+S) still works." : ""}`);
    }
  }

  /** Opens the PDF in micropdf for Windows: a local one as it is unless it has changes here, else
   * the bytes, changes included. `leave` takes the tab back afterwards. */
  async function handOff(leave = false): Promise<void> {
    if (!pages.length) return;
    const local = localPath(source);
    try {
      if (local && !dirty) {
        await openLocal(local);
      } else {
        const saved = await pdf.save();
        canUndo = saved.canUndo;
        canRedo = saved.canRedo;
        await sendPdf(saved.bytes, { name });
      }
    } catch (e) {
      say(`Could not open in micropdf: ${reason(e)}`);
      return;
    }
    if (!leave) say("Opened in micropdf");
    else if (window.history.length > 1) window.history.back();
    else void chrome.tabs.getCurrent().then((tab) => void (tab?.id && chrome.tabs.remove(tab.id)));
  }

  /** Prints the PDF itself, through the browser's own viewer in a frame, not this page. */
  async function print(): Promise<void> {
    if (!pages.length) return;
    try {
      const bytes = await pdf.copy();
      if (printFrame) {
        URL.revokeObjectURL(printFrame.src);
        printFrame.remove();
      }
      const frame = document.createElement("iframe");
      frame.className = "print";
      frame.title = "Print";
      frame.src = URL.createObjectURL(new Blob([bytes], { type: "application/pdf" }));
      frame.onload = () => frame.contentWindow?.print();
      document.body.append(frame);
      printFrame = frame;
    } catch (e) {
      say(`Could not print: ${reason(e)}`);
    }
  }

  function run(cmd: Command): void {
    switch (cmd.id) {
      case "scroll": {
        const unit = cmd.unit === "line" ? LINE : viewHeight / 2;
        scrollBy(cmd.x * unit, cmd.y * unit);
        break;
      }
      case "next-page":
        void goPage(rowAway(cmd.n));
        break;
      case "prev-page":
        void goPage(rowAway(-cmd.n));
        break;
      case "go-page":
        remember();
        void goPage(cmd.page);
        break;
      case "first-page":
        remember();
        void goPage(0);
        break;
      case "last-page":
        remember();
        void goPage(pages.length - 1);
        break;
      case "page-down":
        void pageScroll(true);
        break;
      case "page-up":
        void pageScroll(false);
        break;
      case "left":
      case "right": {
        const forward = cmd.id === "right";
        if (layout.width > viewWidth + 1) scrollBy(forward ? LINE : -LINE, 0);
        else void goPage(rowAway(forward ? 1 : -1));
        break;
      }
      case "back":
        void travel(back, ahead);
        break;
      case "forward":
        void travel(ahead, back);
        break;
      case "zoom-in":
        setZoom(zoomStep(layout.scale, 1));
        break;
      case "zoom-out":
        setZoom(zoomStep(layout.scale, -1));
        break;
      case "zoom-100":
        setZoom(PX_PER_POINT);
        break;
      case "fit-width":
      case "fit-page":
        setZoom(cmd.id);
        break;
      case "rotate-cw":
        rotate(90);
        break;
      case "rotate-ccw":
        rotate(-90);
        break;
      case "find":
        void openFind();
        break;
      case "find-next":
        step(true);
        break;
      case "find-prev":
        step(false);
        break;
      case "goto":
        pageInput?.focus();
        pageInput?.select();
        break;
      case "sidebar":
        sidebar = !sidebar;
        break;
      case "present":
        if (presenting) stopPresenting();
        else void startPresenting();
        break;
      case "copy":
        document.execCommand("copy");
        break;
      case "undo":
      case "redo":
        history(cmd.id === "redo");
        break;
      case "save":
      case "save-as":
        void save(cmd.id === "save-as");
        break;
      case "print":
        void print();
        break;
      case "delete":
        void deletePicked();
        break;
      case "escape":
        if (presenting) stopPresenting();
        else if (finding) closeFind();
        else if (picked) picked = null;
        else if (tool !== "select") tool = "select";
        else getSelection()?.removeAllRanges();
        break;
    }
  }

  function keydown(e: KeyboardEvent): void {
    if (!pages.length || e.defaultPrevented) return;
    const target = e.target instanceof HTMLElement ? e.target : null;
    const typing = target?.closest("input, textarea, select, [contenteditable]") != null;
    if (typing && (!(e.ctrlKey || e.metaKey) || e.key === "Escape")) return;
    // Space and Enter press a focused button.
    if (target?.closest("button, a") && (e.key === " " || e.key === "Enter")) return;
    const cmd = mapKey(
      { key: e.key, ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey, alt: e.altKey },
      { vim, presenting },
      keys,
    );
    // Fields and dialogs keep their own undo.
    if (!cmd || (typing && (cmd.id === "undo" || cmd.id === "redo"))) return;
    // Escape also closes menus and leaves full screen; the browser does that.
    if (cmd.id !== "escape") e.preventDefault();
    run(cmd);
  }

  // Ctrl+wheel zooms the document, not the browser tab; that needs a listener that is not passive.
  $effect(() => {
    if (!view) return;
    const el = view;
    const wheel = (e: WheelEvent) => {
      if (!e.ctrlKey || !pages.length) return;
      e.preventDefault();
      const r = el.getBoundingClientRect();
      setZoom(wheelZoom(layout.scale, e.deltaY), [e.clientX - r.left, e.clientY - r.top]);
    };
    el.addEventListener("wheel", wheel, { passive: false });
    return () => el.removeEventListener("wheel", wheel);
  });

  function placeMenu(e: ToggleEvent): void {
    if (e.newState !== "open" || !menu || !menuButton) return;
    const r = menuButton.getBoundingClientRect();
    menu.style.top = `${r.bottom + 4}px`;
    menu.style.right = `${Math.max(document.documentElement.clientWidth - r.right, 8)}px`;
  }

  /** The part of page `i` in view, in page space; null when none is. */
  function clipOf(i: number): Rect | null {
    const f = layout.frames[i];
    if (!f) return null;
    const x0 = Math.max(scrollLeft, f.x);
    const y0 = Math.max(scrollTop, f.y);
    const x1 = Math.min(scrollLeft + viewWidth, f.x + f.width);
    const y1 = Math.min(scrollTop + viewHeight, f.y + f.height);
    if (x1 <= x0 || y1 <= y0) return null;
    const [ax, ay] = frameToPage(layout, i, x0 - f.x, y0 - f.y);
    const [bx, by] = frameToPage(layout, i, x1 - f.x, y1 - f.y);
    return { x0: Math.min(ax, bx), y0: Math.min(ay, by), x1: Math.max(ax, bx), y1: Math.max(ay, by) };
  }

  const MODES: [PageMode, string][] = [
    ["single", "Single page"],
    ["continuous", "Continuous"],
    ["two-up", "Two pages"],
    ["book", "Book (cover alone)"],
  ];
  const READING: [ReadingMode, string][] = [
    ["normal", "Normal"],
    ["dark", "Dark"],
    ["sepia", "Sepia"],
    ["invert", "Inverted"],
  ];
  const findStatus = $derived(
    !needle.trim()
      ? ""
      : total
        ? `${hitAt ? `${ordinal} of ` : ""}${total}${searched < pages.length ? "+" : ""}`
        : searched < pages.length
          ? "Searching…"
          : "No matches",
  );
</script>

<!-- The hash names the PDF, which is read once; a new one means a new document. -->
<svelte:window
  onkeydown={keydown}
  onhashchange={() => location.reload()}
  onbeforeunload={(e) => dirty && e.preventDefault()}
/>
<svelte:document onfullscreenchange={() => !document.fullscreenElement && stopPresenting()} />

<div class="viewer" class:presenting class:commenting={commenting && pages.length > 0 && !presenting}>
  {#if !presenting}
    <header class="toolbar">
      <button
        class="icon"
        class:active={sidebar}
        aria-label="Sidebar"
        title="Sidebar (F4)"
        disabled={!pages.length}
        onclick={() => (sidebar = !sidebar)}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true"
          ><rect x="1.5" y="2.5" width="13" height="11" rx="1.5" /><path d="M6 2.5v11" /></svg
        >
      </button>
      <span class="name" title={name}>{name}</span>
      {#if pages.length}
        <div class="group">
          <input
            class="page-input"
            bind:this={pageInput}
            aria-label="Page number"
            title="Go to page (Ctrl+G)"
            inputmode="numeric"
            value={current + 1}
            onkeydown={pageKey}
            onfocus={() => pageInput?.select()}
          />
          <span class="muted">/ {pages.length}</span>
        </div>
        <div class="group">
          <button class="icon" aria-label="Zoom out" title="Zoom out (Ctrl+−)" onclick={() => run({ id: "zoom-out" })}
            >−</button
          >
          <span class="percent muted">{Math.round((layout.scale / PX_PER_POINT) * 100)}%</span>
          <button class="icon" aria-label="Zoom in" title="Zoom in (Ctrl+=)" onclick={() => run({ id: "zoom-in" })}
            >+</button
          >
        </div>
        <button class="text" bind:this={menuButton} popovertarget="view-menu">View</button>
        <button
          class="text"
          class:active={commenting}
          aria-pressed={commenting}
          onclick={() => ((commenting = !commenting), commenting || (tool = "select"))}>Comment</button
        >
        <button class="text" title="Save (Ctrl+S); Save as (Ctrl+Shift+S)" onclick={() => save()}
          >{dirty ? "Save •" : "Save"}</button
        >
        <button class="text" title="Open in micropdf for Windows" onclick={() => handOff()}>Open in micropdf</button>
        <button class="icon" class:active={finding} aria-label="Find" title="Find (Ctrl+F)" onclick={openFind}>
          <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" /></svg>
        </button>
      {/if}
    </header>
    {#if commenting && pages.length}
      <!-- Pressing a tool keeps the text selection, for the markup tools. -->
      <div
        class="toolbar tools"
        role="toolbar"
        tabindex="-1"
        aria-label="Comment tools"
        onmousedown={(e) => e.preventDefault()}
      >
        {#each TOOLS as [t, label] (t)}
          <button class="text" class:active={tool === t} aria-pressed={tool === t} onclick={() => choose(t)}
            >{label}</button
          >
        {/each}
        <span class="sep"></span>
        {#each SWATCHES as [label, rgb] (label)}
          <button
            class="swatch"
            class:active={tool !== "select" && css(colorOf(tool)) === css(rgb)}
            aria-label={label}
            title={label}
            style:background={css(rgb)}
            disabled={tool === "select" && (!picked || picked.locked)}
            onclick={() => setColor(rgb)}
          ></button>
        {/each}
        <span class="sep"></span>
        <button class="icon" aria-label="Undo" title="Undo (Ctrl+Z)" disabled={!canUndo} onclick={() => history(false)}
          >↶</button
        >
        <button class="icon" aria-label="Redo" title="Redo (Ctrl+Y)" disabled={!canRedo} onclick={() => history(true)}
          >↷</button
        >
      </div>
    {/if}
  {/if}

  <div class="body">
    {#if sidebar && pages.length && !presenting}
      <Sidebar
        {pdf}
        {pages}
        {rotation}
        {dpr}
        mode={reading}
        {current}
        {versions}
        {outline}
        bind:tab
        ongo={go}
      />
    {/if}
    <main
      bind:this={view}
      bind:contentRect={box}
      onscroll={syncScroll}
    >
      {#if locked}
        <form class="message" onsubmit={unlock}>
          <label for="password">{message || "This PDF has a password."}</label>
          <input id="password" type="password" bind:value={password} autocomplete="off" />
          <button class="text" type="submit">Open</button>
        </form>
      {:else if message}
        <p class="message">{message}</p>
      {:else}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div
          class="column"
          bind:this={column}
          style:width="{layout.width}px"
          style:height="{layout.height}px"
          onclick={pick}
          ondblclick={editPicked}
          onmouseup={() => isMarkup(tool) && void markup(tool)}
        >
          {#each pages as size, i (i)}
            {@const frame = layout.frames[i]}
            {#if frame}
              <Page
                {pdf}
                index={i}
                {size}
                {frame}
                scale={layout.scale}
                {rotation}
                {dpr}
                mode={reading}
                clip={clipOf(i)}
                hits={found[i]}
                currentHit={hitAt?.page === i ? hitAt.index : -1}
                version={versions[i] ?? 0}
                onlink={go}
                onfill={(field, value) => fill(i, field, value)}
              />
            {/if}
          {/each}
          {#if isDraw(tool)}
            <Draw {layout} {tool} color={css(colorOf(tool))} ondraw={drawn} />
          {/if}
          {#if picked}
            {@const v = toView(layout, picked.page, picked.rect)}
            {#if v}
              <div
                class="picked"
                style:left="{v.x - 3}px"
                style:top="{v.y - 3}px"
                style:width="{v.width + 6}px"
                style:height="{v.height + 6}px"
              ></div>
              <div class="card" role="dialog" aria-label="Comment" style:left="{v.x}px" style:top="{v.y + v.height + 8}px">
                <div class="muted">{picked.author || "Comment"} · {picked.type}</div>
                {#if picked.contents}<p>{picked.contents}</p>{/if}
                {#if picked.locked}
                  <div class="muted">Locked</div>
                {:else}
                  <div class="row">
                    <button class="text" onclick={editPicked}>Edit</button>
                    <button class="text" onclick={deletePicked}>Delete</button>
                  </div>
                {/if}
              </div>
            {/if}
          {/if}
        </div>
      {/if}
    </main>
    {#if notice}
      <div class="notice" role="status">{notice}</div>
    {/if}
    {#if finding}
      <div class="find" role="search">
        <input
          bind:this={findInput}
          bind:value={needle}
          aria-label="Find in document"
          placeholder="Find in document"
          oninput={searchSoon}
          onkeydown={findKey}
        />
        <span class="muted status" aria-live="polite">{findStatus}</span>
        <button class="icon" aria-label="Previous match" title="Previous (Shift+Enter)" onclick={() => step(false)}
          >↑</button
        >
        <button class="icon" aria-label="Next match" title="Next (Enter)" onclick={() => step(true)}>↓</button>
        <button class="icon" aria-label="Close find" title="Close (Esc)" onclick={closeFind}>×</button>
      </div>
    {/if}
  </div>
</div>

<dialog bind:this={dialog} aria-label={asking?.title} onclose={() => answer(false)}>
  {#if asking}
    <form
      onsubmit={(e) => {
        e.preventDefault();
        answer(true);
      }}
    >
      <h2>{asking.title}</h2>
      <!-- svelte-ignore a11y_autofocus -->
      <textarea
        bind:value={asking.text}
        rows="5"
        autofocus
        aria-label="Comment text"
        onkeydown={(e) => e.key === "Enter" && (e.ctrlKey || e.metaKey) && answer(true)}
      ></textarea>
      <div class="row">
        <button class="text" type="button" onclick={() => answer(false)}>Cancel</button>
        <button class="text primary" type="submit">OK</button>
      </div>
    </form>
  {/if}
</dialog>

<div id="view-menu" class="menu" popover bind:this={menu} ontoggle={placeMenu}>
  <section>
    <h2>Zoom</h2>
    <button class:active={zoom === "fit-width"} onclick={() => setZoom("fit-width")}
      >Fit width <kbd>Ctrl+2</kbd></button
    >
    <button class:active={zoom === "fit-page"} onclick={() => setZoom("fit-page")}
      >Fit page <kbd>Ctrl+0</kbd></button
    >
    <button class:active={zoom === PX_PER_POINT} onclick={() => setZoom(PX_PER_POINT)}
      >Actual size <kbd>Ctrl+1</kbd></button
    >
  </section>
  <section>
    <h2>Pages</h2>
    {#each MODES as [m, label] (m)}
      <button class:active={mode === m} onclick={() => setMode(m)}>{label}</button>
    {/each}
  </section>
  <section>
    <h2>Rotate view</h2>
    <div class="row">
      <button onclick={() => rotate(-90)}>⟲ Left</button>
      <button onclick={() => rotate(90)}>⟳ Right</button>
    </div>
  </section>
  <section>
    <h2>Reading mode</h2>
    {#each READING as [r, label] (r)}
      <button class:active={reading === r} onclick={() => (reading = r)}>{label}</button>
    {/each}
  </section>
  <section>
    <label class="check"><input type="checkbox" bind:checked={vim} /> Vim keys</label>
    <button onclick={() => (menu?.hidePopover(), void startPresenting())}>Present <kbd>Ctrl+L</kbd></button>
  </section>
</div>

<style>
  .viewer {
    display: grid;
    grid-template-rows: auto 1fr;
    grid-template-columns: minmax(0, 1fr);
    height: 100%;
  }

  .viewer.commenting {
    grid-template-rows: auto auto 1fr;
  }

  .viewer.presenting {
    grid-template-rows: 1fr;
    background: #000;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 12px;
    height: var(--toolbar-height);
    padding: 0 8px;
    background: var(--color-bg-secondary);
    border-bottom: 1px solid var(--color-border-muted);
  }

  .tools {
    gap: 2px;
    overflow-x: auto;
  }

  .tools .text {
    flex: none;
    padding: 0 8px;
  }

  .sep {
    flex: none;
    width: 1px;
    height: 20px;
    margin: 0 6px;
    background: var(--color-border-muted);
  }

  .swatch {
    flex: none;
    width: 20px;
    height: 20px;
    padding: 0;
    border: 1px solid var(--color-border);
    border-radius: 50%;
  }

  .swatch.active {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }

  .swatch:disabled {
    opacity: 0.35;
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-display);
  }

  .group {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .muted {
    color: var(--color-text-muted);
    font-variant-numeric: tabular-nums;
  }

  .percent {
    min-width: 4.5ch;
    text-align: center;
  }

  button {
    height: 28px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }

  button:disabled {
    color: var(--color-text-subtle);
    cursor: default;
  }

  button.active {
    border-color: var(--color-border);
    background: var(--color-bg-active);
  }

  .icon {
    display: grid;
    place-items: center;
    width: 28px;
    padding: 0;
  }

  .text {
    padding: 0 10px;
  }

  svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.3;
    stroke-linecap: round;
  }

  input {
    padding: 4px 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg-tertiary);
    color: var(--color-text);
    font: inherit;
  }

  .page-input {
    width: 5ch;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .body {
    position: relative;
    display: flex;
    min-width: 0;
    min-height: 0;
  }

  main {
    position: relative;
    flex: 1;
    min-width: 0;
    overflow: auto;
  }

  .presenting main {
    overflow: hidden;
  }

  .column {
    position: relative;
    min-width: 100%;
  }

  .picked {
    position: absolute;
    border: 2px solid var(--color-accent);
    border-radius: var(--radius-sm);
    pointer-events: none;
  }

  .card {
    position: absolute;
    z-index: 1;
    max-width: 280px;
    padding: 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-bg-secondary);
    box-shadow: 0 4px 16px var(--color-shadow-lg);
    white-space: pre-wrap;
  }

  .card p {
    margin: 6px 0;
  }

  .card .row,
  dialog .row {
    display: flex;
    justify-content: flex-end;
    gap: 4px;
  }

  .notice {
    position: absolute;
    bottom: 16px;
    left: 50%;
    transform: translateX(-50%);
    padding: 6px 12px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-bg-secondary);
    box-shadow: 0 4px 16px var(--color-shadow-lg);
  }

  dialog {
    width: min(420px, calc(100vw - 32px));
    padding: 16px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-bg-secondary);
    color: var(--color-text);
  }

  dialog::backdrop {
    background: rgb(0 0 0 / 30%);
  }

  dialog h2 {
    margin: 0 0 8px;
    font: inherit;
    font-weight: 600;
  }

  dialog textarea {
    box-sizing: border-box;
    width: 100%;
    margin-bottom: 8px;
    padding: 6px 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg-tertiary);
    color: var(--color-text);
    font: inherit;
    resize: vertical;
  }

  .primary {
    border-color: var(--color-accent);
    background: var(--color-accent);
    color: var(--color-on-accent);
  }

  .message {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    margin: 20vh auto 0;
    max-width: 40ch;
    color: var(--color-text-muted);
    text-align: center;
  }

  .message input {
    width: 100%;
  }

  .find {
    position: absolute;
    top: 8px;
    right: 24px;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-bg-secondary);
    box-shadow: 0 4px 16px var(--color-shadow-lg);
  }

  .find input {
    width: 22ch;
  }

  .status {
    min-width: 9ch;
    padding: 0 4px;
    text-align: right;
  }

  .menu {
    position: fixed;
    inset: auto;
    margin: 0;
    padding: 6px;
    min-width: 220px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-bg-secondary);
    color: var(--color-text);
    box-shadow: 0 8px 24px var(--color-shadow-lg);
  }

  .menu section {
    display: flex;
    flex-direction: column;
    padding: 4px 0;
  }

  .menu section + section {
    border-top: 1px solid var(--color-border-muted);
  }

  .menu h2 {
    margin: 4px 8px;
    color: var(--color-text-subtle);
    font: inherit;
    font-size: 12px;
  }

  .menu button {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 0 8px;
    text-align: left;
  }

  .menu .row {
    display: flex;
    gap: 4px;
  }

  .menu .row button {
    flex: 1;
  }

  kbd {
    color: var(--color-text-subtle);
    font: inherit;
    font-size: 12px;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 8px;
  }
</style>
