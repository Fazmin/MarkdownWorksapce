<script lang="ts">
  import { onMount, tick } from "svelte";
  import { marked } from "marked";
  import hljs from "highlight.js/lib/common";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open, save, confirm } from "@tauri-apps/plugin-dialog";
  import { themes, typefaces, type ThemeTokens } from "./lib/themes";
  import type { EditorHandle } from "./lib/editor";

  type StudioFile = { id: string; name: string; path?: string; content: string; savedContent: string };
  type SavedProject = { id: number; name: string; updated_at: string; file_count: number };
  type ExportOptions = {
    toc: boolean;
    pageNumbers: boolean;
    headerLeft: string;
    headerCenter: string;
    headerRight: string;
    footerLeft: string;
    footerCenter: string;
    footerRight: string;
  };
  type DocumentStats = {
    words: number;
    characters: number;
    pages: number;
    readingMinutes: number;
    headings: number;
    links: number;
    codeBlocks: number;
  };
  type MinimapBlock = {
    top: number;
    height: number;
    width: number;
    kind: "text" | "heading" | "code" | "list";
  };
  type ContentWidth = "theme" | "wide" | "extra-wide";

  const isTauri = "__TAURI_INTERNALS__" in window;
  const appWindow = isTauri ? getCurrentWindow() : null;
  const renderCache = new Map<string, { source: string; html: string; bytes: number }>();
  const statsCache = new Map<string, { source: string; stats: DocumentStats }>();
  const maxRenderCacheBytes = 12 * 1024 * 1024;
  let renderCacheBytes = 0;
  let renderSequence = 0;
  let highlightSequence = 0;
  let searchTimer = 0;
  let files: StudioFile[] = [];
  let activeId = "";
  let projectName = "Untitled collection";
  let activeTheme: ThemeTokens = window.matchMedia("(prefers-color-scheme: dark)").matches ? themes[3] : themes[0];
  let fontSize = 18;
  let typeface: keyof typeof typefaces = "serif";
  let contentWidth: ContentWidth = "theme";
  let renderedHtml = "";
  let search = "";
  let searchCount = 0;
  let searchIndex = 0;
  let searchLimited = false;
  let articleEl: HTMLElement;
  let readerEl: HTMLElement;
  let readerFrameEl: HTMLElement;
  let minimapEl: HTMLElement;
  let showMinimap = true;
  let showDocumentStats = true;
  let printAllFiles = true;
  let printHtml = "";
  let minimapBlocks: MinimapBlock[] = [];
  let minimapViewportTop = 0;
  let minimapViewportHeight = 40;
  let minimapScrollPercent = 0;
  let draggingId = "";
  let dragActive = false;
  let showPublish = false;
  let showLibrary = false;
  let showSettings = false;
  let showSidePanels = true;
  let busy = false;
  let toast = "";
  let selectionMenu = { visible: false, x: 0, y: 0, text: "" };
  let selectionTimer = 0;
  let speaking = false;
  let currentUtterance: SpeechSynthesisUtterance | null = null;
  let editMode = false;
  let editorHandle: EditorHandle | null = null;
  let editorPaneEl: HTMLElement;
  let editingFileId = "";
  let editTimer = 0;
  let pendingContent: string | null = null;
  let savedProjects: SavedProject[] = [];
  let projectId: number | null = null;
  let fileInput: HTMLInputElement;
  let folderInput: HTMLInputElement;
  let exportOptions: ExportOptions = {
    toc: true,
    pageNumbers: true,
    headerLeft: "",
    headerCenter: "{title}",
    headerRight: "",
    footerLeft: "",
    footerCenter: "",
    footerRight: "{page}"
  };

  $: activeFile = files.find((file) => file.id === activeId) ?? files[0];
  $: activeDirty = activeFile ? activeFile.content !== activeFile.savedContent : false;
  $: if (editMode && editorHandle && activeFile && activeFile.id !== editingFileId) {
    flushEdits();
    editingFileId = activeFile.id;
    editorHandle.setDoc(activeFile.content);
  }
  $: documentStats = getDocumentStats(activeFile);
  $: minimapBlocks = buildMinimapBlocks(activeFile?.content ?? "");
  $: if (activeFile) renderFile(activeFile);
  $: documentStyle = [
    `--paper:${activeTheme.paper}`,
    `--ink:${activeTheme.ink}`,
    `--muted:${activeTheme.muted}`,
    `--accent:${activeTheme.accent}`,
    `--rule:${activeTheme.rule}`,
    `--code-bg:${activeTheme.codeBg}`,
    `--reader-font:${typefaces[typeface]}`,
    `--heading-font:${activeTheme.fontHeading}`,
    `--reader-size:${fontSize}px`,
    `--reader-leading:${activeTheme.lineHeight}`,
    `--measure:${contentWidth === "wide" ? 960 : contentWidth === "extra-wide" ? 1240 : activeTheme.measure}px`,
    `--paragraph-gap:${activeTheme.paragraphGap}em`
  ].join(";");
  $: if (showMinimap && renderedHtml) {
    activeTheme;
    fontSize;
    typeface;
    contentWidth;
    tick().then(updateMinimapLayout);
  }

  marked.setOptions({ gfm: true, breaks: false });

  function calculateDocumentStats(markdown: string): DocumentStats {
    const headings = markdown.match(/^#{1,6}\s+\S.*$/gm)?.length ?? 0;
    const links = markdown.match(/(?<!!)\[[^\]]+\]\([^)]+\)/g)?.length ?? 0;
    const fenceLines = markdown.match(/^(?:```|~~~)/gm)?.length ?? 0;
    const codeBlocks = Math.ceil(fenceLines / 2);
    const plainText = markdown
      .replace(/^---\s*$[\s\S]*?^---\s*$/m, "")
      .replace(/^(?:```|~~~).*$/gm, "")
      .replace(/!\[([^\]]*)\]\([^)]+\)/g, "$1")
      .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
      .replace(/<[^>]+>/g, "")
      .replace(/^#{1,6}\s+/gm, "")
      .replace(/[*_~>`|]/g, "")
      .replace(/\s+/g, " ")
      .trim();
    const words = plainText.match(/[\p{L}\p{N}]+(?:['’.-][\p{L}\p{N}]+)*/gu)?.length ?? 0;
    return {
      words,
      characters: Array.from(plainText).length,
      pages: Math.max(1, Math.ceil(words / 450)),
      readingMinutes: Math.max(1, Math.ceil(words / 225)),
      headings,
      links,
      codeBlocks
    };
  }

  function getDocumentStats(file?: StudioFile): DocumentStats {
    if (!file) return calculateDocumentStats("");
    const cached = statsCache.get(file.id);
    if (cached?.source === file.content) return cached.stats;
    const stats = calculateDocumentStats(file.content);
    statsCache.set(file.id, { source: file.content, stats });
    if (statsCache.size > 512) statsCache.delete(statsCache.keys().next().value!);
    return stats;
  }

  function buildMinimapBlocks(markdown: string): MinimapBlock[] {
    if (!markdown) return [];
    const targetBlocks = 320;
    const chunkSize = Math.max(96, Math.ceil(markdown.length / targetBlocks));
    const blocks: MinimapBlock[] = [];
    for (let offset = 0; offset < markdown.length; offset += chunkSize) {
      const chunk = markdown.slice(offset, offset + chunkSize);
      const compact = chunk.replace(/\s+/g, " ").trim();
      if (!compact) continue;
      const firstLine = chunk.trimStart().split(/\r?\n/, 1)[0];
      const kind: MinimapBlock["kind"] =
        /^#{1,6}\s/.test(firstLine) ? "heading" :
        /^(?:```|~~~)/.test(firstLine) ? "code" :
        /^(?:[-*+]|\d+\.)\s/.test(firstLine) ? "list" : "text";
      const density = Math.min(1, compact.length / chunkSize);
      blocks.push({
        top: (offset / markdown.length) * 100,
        height: Math.max(0.12, (chunkSize / markdown.length) * 72),
        width: Math.min(100, 48 + density * 52),
        kind
      });
    }
    return blocks;
  }

  function formatStat(value: number) {
    return new Intl.NumberFormat(undefined, { notation: value >= 10_000 ? "compact" : "standard" }).format(value);
  }

  function escapeHtml(value: string) {
    return value
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;");
  }

  async function loadPendingOpenFiles() {
    try {
      const paths = await invoke<string[]>("take_opened_files");
      if (paths.length) await loadPaths(paths, { quickView: true });
    } catch (error) {
      notify(`Unable to open Markdown file: ${String(error)}`);
    }
  }

  onMount(() => {
    let unlistenDrop: (() => void) | undefined;
    let unlistenOpen: (() => void) | undefined;
    let unlistenClose: (() => void) | undefined;
    const resize = () => updateMinimapLayout();
    window.addEventListener("resize", resize);
    document.addEventListener("selectionchange", scheduleSelectionMenu);
    if (isTauri) {
      getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type === "enter" || event.payload.type === "over") {
            dragActive = true;
          } else if (event.payload.type === "leave") {
            dragActive = false;
          } else if (event.payload.type === "drop") {
            dragActive = false;
            loadPaths(event.payload.paths);
          }
        })
        .then((stopListening) => {
          unlistenDrop = stopListening;
        })
        .catch((error) => notify(`Drag and drop unavailable: ${String(error)}`));
      void (async () => {
        unlistenOpen = await listen("open-files-requested", loadPendingOpenFiles);
        await loadPendingOpenFiles();
      })();
      appWindow
        ?.onCloseRequested(async (event) => {
          try {
            flushEdits();
            if (!files.some((file) => file.content !== file.savedContent)) return;
            const leave = await confirm("You have unsaved changes. Close anyway?", { title: "Markdown Studio", kind: "warning" });
            if (!leave) event.preventDefault();
          } catch {
            // fail open: a broken dialog must never block closing the window
          }
        })
        .then((stopListening) => {
          unlistenClose = stopListening;
        })
        .catch(() => {});
    }
    return () => {
      unlistenDrop?.();
      unlistenOpen?.();
      unlistenClose?.();
      window.removeEventListener("resize", resize);
      document.removeEventListener("selectionchange", scheduleSelectionMenu);
      window.clearTimeout(selectionTimer);
      stopSpeaking();
      teardownEditor();
    };
  });

  function cacheRender(file: StudioFile, html: string) {
    const bytes = html.length * 2;
    if (bytes > maxRenderCacheBytes / 2) return;
    const previous = renderCache.get(file.id);
    if (previous) renderCacheBytes -= previous.bytes;
    renderCache.delete(file.id);
    renderCache.set(file.id, { source: file.content, html, bytes });
    renderCacheBytes += bytes;
    while (renderCacheBytes > maxRenderCacheBytes || renderCache.size > 16) {
      const oldestKey = renderCache.keys().next().value;
      if (!oldestKey) break;
      renderCacheBytes -= renderCache.get(oldestKey)!.bytes;
      renderCache.delete(oldestKey);
    }
  }

  async function renderFile(file: StudioFile) {
    const sequence = ++renderSequence;
    const cached = renderCache.get(file.id);
    if (cached?.source === file.content) {
      renderCache.delete(file.id);
      renderCache.set(file.id, cached);
      renderedHtml = cached.html;
      await finishRender(sequence);
      return;
    }
    let html: string;
    try {
      html = isTauri
        ? await invoke<string>("render_markdown", { markdown: file.content })
        : String(await marked.parse(file.content));
    } catch {
      html = String(await marked.parse(file.content));
    }
    if (sequence !== renderSequence) return;
    cacheRender(file, html);
    renderedHtml = html;
    await finishRender(sequence);
  }

  async function finishRender(sequence: number) {
    await tick();
    if (sequence !== renderSequence) return;
    scheduleHighlighting(sequence);
    scheduleSearch(0);
    updateMinimapLayout();
  }

  function scheduleHighlighting(sequence: number) {
    const highlightRun = ++highlightSequence;
    const blocks = Array.from(articleEl?.querySelectorAll<HTMLElement>("pre code") ?? [])
      .filter((block) => block.textContent!.length <= 50_000)
      .slice(0, 200);
    let index = 0;
    const work = () => {
      if (sequence !== renderSequence || highlightRun !== highlightSequence) return;
      const deadline = performance.now() + 8;
      while (index < blocks.length && performance.now() < deadline) {
        hljs.highlightElement(blocks[index++]);
      }
      if (index < blocks.length) window.setTimeout(work, 0);
    };
    work();
  }

  function updateMinimapLayout() {
    if (!showMinimap || !readerEl || !articleEl || !minimapEl) return;
    requestAnimationFrame(updateMinimapViewport);
  }

  function updateMinimapViewport() {
    if (!showMinimap || !readerEl || !minimapEl) return;
    const trackHeight = Math.max(1, minimapEl.clientHeight - 12);
    const visibleRatio = Math.min(1, readerEl.clientHeight / Math.max(readerEl.scrollHeight, 1));
    minimapViewportHeight = Math.max(22, trackHeight * visibleRatio);
    const scrollRange = Math.max(1, readerEl.scrollHeight - readerEl.clientHeight);
    const minimapRange = Math.max(0, trackHeight - minimapViewportHeight);
    minimapScrollPercent = Math.round((readerEl.scrollTop / scrollRange) * 100);
    minimapViewportTop = 6 + (minimapScrollPercent / 100) * minimapRange;
  }

  function navigateFromMinimap(event: PointerEvent) {
    if (!readerEl || !minimapEl) return;
    const bounds = minimapEl.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (event.clientY - bounds.top) / bounds.height));
    readerEl.scrollTo({
      top: ratio * readerEl.scrollHeight - readerEl.clientHeight / 2,
      behavior: "smooth"
    });
  }

  function scheduleSelectionMenu() {
    window.clearTimeout(selectionTimer);
    selectionTimer = window.setTimeout(updateSelectionMenu, 140);
  }

  function updateSelectionMenu() {
    const selection = window.getSelection();
    const text = selection && !selection.isCollapsed ? selection.toString().trim() : "";
    if (
      !selection || !text || !articleEl || !readerFrameEl ||
      !articleEl.contains(selection.anchorNode) ||
      !articleEl.contains(selection.focusNode)
    ) {
      if (selectionMenu.visible) selectionMenu = { ...selectionMenu, visible: false };
      return;
    }
    const rect = selection.getRangeAt(0).getBoundingClientRect();
    if (!rect.width && !rect.height) return;
    const frame = readerFrameEl.getBoundingClientRect();
    const x = Math.min(Math.max(rect.left + rect.width / 2 - frame.left, 58), Math.max(58, frame.width - 58));
    const above = rect.top - frame.top - 48;
    const y = Math.min(Math.max(above >= 8 ? above : rect.bottom - frame.top + 10, 8), Math.max(8, frame.height - 48));
    selectionMenu = { visible: true, x, y, text };
  }

  function hideSelectionMenu() {
    if (selectionMenu.visible) selectionMenu = { ...selectionMenu, visible: false };
  }

  function onReaderScroll() {
    updateMinimapViewport();
    if (selectionMenu.visible) updateSelectionMenu();
  }

  async function copySelection() {
    if (!selectionMenu.text) return;
    try {
      await navigator.clipboard.writeText(selectionMenu.text);
      notify("Copied to clipboard");
    } catch {
      notify(document.execCommand("copy") ? "Copied to clipboard" : "Unable to copy selection");
    }
  }

  function speakSelection() {
    if (!("speechSynthesis" in window)) return notify("Read aloud is not available on this system");
    if (!selectionMenu.text) return;
    speechSynthesis.cancel();
    const utterance = new SpeechSynthesisUtterance(selectionMenu.text);
    currentUtterance = utterance;
    utterance.onend = utterance.onerror = () => {
      if (currentUtterance === utterance) {
        speaking = false;
        currentUtterance = null;
      }
    };
    speaking = true;
    speechSynthesis.speak(utterance);
  }

  function stopSpeaking() {
    if (!("speechSynthesis" in window)) return;
    currentUtterance = null;
    speechSynthesis.cancel();
    speaking = false;
  }

  function onEditorChange(text: string) {
    pendingContent = text;
    window.clearTimeout(editTimer);
    editTimer = window.setTimeout(flushEdits, 200);
  }

  function flushEdits() {
    window.clearTimeout(editTimer);
    if (pendingContent === null) return;
    const text = pendingContent;
    const targetId = editingFileId;
    pendingContent = null;
    files = files.map((file) => file.id === targetId ? { ...file, content: text } : file);
  }

  async function toggleEditMode() {
    if (editMode) return teardownEditor();
    editMode = true;
    const { createEditor } = await import("./lib/editor");
    await tick();
    if (!editMode || editorHandle || !editorPaneEl) return;
    editingFileId = activeFile?.id ?? "";
    editorHandle = createEditor(editorPaneEl, activeFile?.content ?? "", onEditorChange);
    editorHandle.view.focus();
  }

  function teardownEditor() {
    flushEdits();
    pendingContent = null;
    editingFileId = "";
    editorHandle?.destroy();
    editorHandle = null;
    editMode = false;
  }

  async function saveActiveFile() {
    flushEdits();
    const file = files.find((candidate) => candidate.id === activeId) ?? files[0];
    if (!file) return;
    if (!isTauri) {
      const blob = new Blob([file.content], { type: "text/markdown" });
      const anchor = document.createElement("a");
      anchor.href = URL.createObjectURL(blob);
      anchor.download = `${file.name}.md`;
      anchor.click();
      files = files.map((candidate) => candidate.id === file.id ? { ...candidate, savedContent: candidate.content } : candidate);
      return notify("Downloaded a copy — the browser cannot write to the original file");
    }
    let path = file.path;
    if (!path) {
      const chosen = await save({
        defaultPath: `${file.name.replace(/[<>:"/\\|?*]/g, "-")}.md`,
        filters: [{ name: "Markdown", extensions: ["md"] }]
      });
      if (!chosen) return;
      path = chosen;
    }
    busy = true;
    try {
      await invoke("write_markdown_file", { path, content: file.content });
      const savedName = path.split(/[\\/]/).pop()!.replace(/\.(md|markdown)$/i, "");
      files = files.map((candidate) => candidate.id === file.id
        ? { ...candidate, path, name: candidate.path ? candidate.name : savedName, savedContent: candidate.content }
        : candidate);
      notify("Saved");
    } catch (error) {
      notify(String(error));
    } finally {
      busy = false;
    }
  }

  function ordered(incoming: StudioFile[]) {
    const number = (name: string) => Number(name.match(/(?:^|[\\/])(\d+)/)?.[1] ?? Number.MAX_SAFE_INTEGER);
    return incoming.sort((a, b) => number(a.name) - number(b.name) || a.name.localeCompare(b.name));
  }

  function clearSessionCaches() {
    stopSpeaking();
    hideSelectionMenu();
    teardownEditor();
    renderCache.clear();
    statsCache.clear();
    renderCacheBytes = 0;
    renderSequence++;
    highlightSequence++;
  }

  function goHome() {
    clearSessionCaches();
    files = [];
    activeId = "";
    projectId = null;
    renderedHtml = "";
    showSidePanels = true;
  }

  async function ingest(fileList: FileList | File[]) {
    const markdownFiles = Array.from(fileList).filter((file) => /\.md(?:own)?$/i.test(file.name));
    if (!markdownFiles.length) return notify("No Markdown files found");
    const loaded: StudioFile[] = [];
    for (let index = 0; index < markdownFiles.length; index += 8) {
      const batch = await Promise.all(markdownFiles.slice(index, index + 8).map(async (file) => {
        const content = await file.text();
        return {
          id: crypto.randomUUID(),
          name: file.name.replace(/\.md(?:own)?$/i, ""),
          content,
          savedContent: content
        };
      }));
      loaded.push(...batch);
    }
    clearSessionCaches();
    files = ordered(loaded);
    activeId = files[0].id;
    projectName = files.length === 1 ? files[0].name : "New collection";
    projectId = null;
    showSidePanels = true;
  }

  async function chooseFiles() {
    if (!isTauri) return fileInput.click();
    const paths = await open({ multiple: true, directory: false, filters: [{ name: "Markdown", extensions: ["md", "markdown"] }] });
    if (!paths) return;
    await loadPaths(Array.isArray(paths) ? paths : [paths]);
  }

  async function chooseFolder() {
    if (!isTauri) return folderInput.click();
    const path = await open({ directory: true, multiple: false });
    if (path) await loadPaths([path]);
  }

  async function loadPaths(paths: string[], options: { quickView?: boolean } = {}) {
    busy = true;
    try {
      const loaded = await invoke<Omit<StudioFile, "id" | "savedContent">[]>("read_markdown_paths", { paths });
      clearSessionCaches();
      files = ordered(loaded.map((file) => ({ ...file, id: crypto.randomUUID(), savedContent: file.content })));
      if (!files.length) return notify("No Markdown files found");
      activeId = files[0].id;
      projectName = files.length === 1 ? files[0].name : String(paths[0]).split(/[\\/]/).pop() || "New collection";
      projectId = null;
      showSidePanels = !options.quickView;
    } catch (error) {
      notify(String(error));
    } finally {
      busy = false;
    }
  }

  function onDrop(event: DragEvent) {
    event.preventDefault();
    dragActive = false;
    if (event.dataTransfer?.files.length) ingest(event.dataTransfer.files);
  }

  function reorder(targetId: string) {
    if (!draggingId || draggingId === targetId) return;
    const from = files.findIndex((file) => file.id === draggingId);
    const to = files.findIndex((file) => file.id === targetId);
    const next = [...files];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    files = next;
  }

  function removeFile(id: string) {
    const index = files.findIndex((file) => file.id === id);
    const cached = renderCache.get(id);
    if (cached) renderCacheBytes -= cached.bytes;
    renderCache.delete(id);
    statsCache.delete(id);
    files = files.filter((file) => file.id !== id);
    if (activeId === id) activeId = files[Math.min(index, files.length - 1)]?.id ?? "";
    if (!files.length) teardownEditor();
  }

  function scheduleSearch(delay = 160) {
    window.clearTimeout(searchTimer);
    searchTimer = window.setTimeout(applySearch, delay);
  }

  function applySearch() {
    if (!articleEl) return;
    const previousMarks = articleEl.querySelectorAll("mark.search-hit");
    previousMarks.forEach((mark) => mark.replaceWith(document.createTextNode(mark.textContent ?? "")));
    if (previousMarks.length) articleEl.normalize();
    searchCount = 0;
    searchIndex = 0;
    searchLimited = false;
    const query = search.trim();
    if (!query) return;
    const walker = document.createTreeWalker(articleEl, NodeFilter.SHOW_TEXT);
    const nodes: Text[] = [];
    const normalizedQuery = query.toLocaleLowerCase();
    const maxMatches = 500;
    let potentialMatches = 0;
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      if (node.parentElement?.closest("pre, code")) continue;
      const normalizedText = node.data.toLocaleLowerCase();
      let matchAt = normalizedText.indexOf(normalizedQuery);
      if (matchAt < 0) continue;
      nodes.push(node);
      while (matchAt >= 0) {
        potentialMatches++;
        if (potentialMatches >= maxMatches) {
          searchLimited = true;
          break;
        }
        matchAt = normalizedText.indexOf(normalizedQuery, matchAt + normalizedQuery.length);
      }
      if (searchLimited) break;
    }
    const regex = new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi");
    for (const node of nodes) {
      if (!regex.test(node.data)) continue;
      regex.lastIndex = 0;
      const fragment = document.createDocumentFragment();
      let cursor = 0;
      for (const match of node.data.matchAll(regex)) {
        if (searchCount >= maxMatches) {
          searchLimited = true;
          break;
        }
        fragment.append(node.data.slice(cursor, match.index));
        const mark = document.createElement("mark");
        mark.className = "search-hit";
        mark.textContent = match[0];
        fragment.append(mark);
        cursor = (match.index ?? 0) + match[0].length;
        searchCount++;
      }
      fragment.append(node.data.slice(cursor));
      node.replaceWith(fragment);
    }
    focusMatch(0);
  }

  function focusMatch(direction: number) {
    const matches = Array.from(articleEl?.querySelectorAll<HTMLElement>("mark.search-hit") ?? []);
    if (!matches.length) return;
    searchIndex = (searchIndex + direction + matches.length) % matches.length;
    matches.forEach((match, index) => match.classList.toggle("current", index === searchIndex));
    matches[searchIndex].scrollIntoView({ behavior: "smooth", block: "center" });
  }

  function toggleThemeMode() {
    activeTheme = activeTheme.mode === "dark" ? themes[0] : themes[3];
  }

  async function minimizeWindow() {
    try {
      await appWindow?.minimize();
    } catch (error) {
      notify(String(error));
    }
  }

  async function toggleMaximizeWindow() {
    try {
      await appWindow?.toggleMaximize();
    } catch (error) {
      notify(String(error));
    }
  }

  async function closeWindow() {
    try {
      await appWindow?.close();
    } catch (error) {
      notify(String(error));
    }
  }

  function projectPayload() {
    return {
      id: projectId,
      name: projectName,
      files: files.map(({ name, path, content }) => ({ name, path, content })),
      theme: activeTheme,
      fontSize,
      typeface,
      contentWidth,
      exportOptions
    };
  }

  async function saveProject() {
    flushEdits();
    if (!isTauri) return notify("Project saving is available in the desktop app");
    busy = true;
    try {
      projectId = await invoke<number>("save_project", { project: projectPayload() });
      notify("Project saved");
    } catch (error) {
      notify(String(error));
    } finally {
      busy = false;
    }
  }

  async function openLibrary() {
    showLibrary = true;
    if (!isTauri) return;
    try {
      savedProjects = await invoke<SavedProject[]>("list_projects");
    } catch (error) {
      notify(String(error));
    }
  }

  async function loadProject(id: number) {
    try {
      const project = await invoke<any>("load_project", { id });
      clearSessionCaches();
      projectId = project.id;
      projectName = project.name;
      files = project.files.map((file: any) => ({ ...file, id: crypto.randomUUID(), savedContent: file.content }));
      activeId = files[0]?.id ?? "";
      activeTheme = themes.find((theme) => theme.id === project.theme.id) ?? project.theme;
      fontSize = project.fontSize;
      typeface = project.typeface;
      contentWidth = ["theme", "wide", "extra-wide"].includes(project.contentWidth)
        ? project.contentWidth
        : "theme";
      exportOptions = project.exportOptions;
      showSidePanels = true;
      showLibrary = false;
    } catch (error) {
      notify(String(error));
    }
  }

  async function exportDocument(format: "pdf" | "docx" | "html" | "md") {
    flushEdits();
    if (!isTauri) {
      if (format !== "html" && format !== "md") return notify("PDF and DOCX export require the desktop app");
      const content = format === "md"
        ? files.map((file) => file.content.trim()).join("\n\n")
        : String(await marked.parse(files.map((file) => file.content).join("\n\n")));
      const blob = new Blob([content], { type: format === "md" ? "text/markdown" : "text/html" });
      const anchor = document.createElement("a");
      anchor.href = URL.createObjectURL(blob);
      anchor.download = `${projectName}.${format}`;
      anchor.click();
      return;
    }
    const path = await save({
      defaultPath: `${projectName.replace(/[<>:"/\\|?*]/g, "-")}.${format}`,
      filters: [{ name: format.toUpperCase(), extensions: [format] }]
    });
    if (!path) return;
    busy = true;
    try {
      await invoke("export_document", { request: { ...projectPayload(), format, outputPath: path } });
      showPublish = false;
      notify(`${format.toUpperCase()} exported`);
    } catch (error) {
      notify(String(error));
    } finally {
      busy = false;
    }
  }

  async function printDocument() {
    flushEdits();
    const currentFile = files.find((file) => file.id === activeId) ?? files[0];
    const selectedFiles = printAllFiles ? files : currentFile ? [currentFile] : [];
    if (!selectedFiles.length) return;
    busy = true;
    try {
      const sections: string[] = [];
      for (let start = 0; start < selectedFiles.length; start += 4) {
        const batch = await Promise.all(selectedFiles.slice(start, start + 4).map(async (file, batchIndex) => {
          const index = start + batchIndex;
          const html = isTauri
            ? await invoke<string>("render_markdown", { markdown: file.content })
            : String(await marked.parse(file.content));
          return `<article class="markdown-body print-source"><div class="print-source-label">${String(index + 1).padStart(2, "0")} · ${escapeHtml(file.name)}</div>${html}</article>`;
        }));
        sections.push(...batch);
      }
      printHtml = sections.join("");
      showPublish = false;
      await tick();
      const cleanup = () => {
        document.body.classList.remove("printing");
        printHtml = "";
      };
      window.addEventListener("afterprint", cleanup, { once: true });
      document.body.classList.add("printing");
      window.setTimeout(() => window.print(), 50);
    } catch (error) {
      notify(`Unable to print: ${String(error)}`);
    } finally {
      busy = false;
    }
  }

  function notify(message: string) {
    toast = message;
    window.setTimeout(() => { if (toast === message) toast = ""; }, 2800);
  }

  function handleKeydown(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
      event.preventDefault();
      document.querySelector<HTMLInputElement>(".titlebar-search input")?.focus();
    }
    if ((event.ctrlKey || event.metaKey) && event.key === "=") { event.preventDefault(); fontSize = Math.min(fontSize + 1, 28); }
    if ((event.ctrlKey || event.metaKey) && event.key === "-") { event.preventDefault(); fontSize = Math.max(fontSize - 1, 13); }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "p") { event.preventDefault(); printDocument(); }
    if ((event.ctrlKey || event.metaKey) && !event.shiftKey && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (editMode) saveActiveFile();
    }
    if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "t") toggleThemeMode();
    if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "e") { event.preventDefault(); showPublish = true; }
    if (event.key === "Escape") {
      if (speaking) stopSpeaking();
      hideSelectionMenu();
    }
  }
</script>

<svelte:window
  on:keydown={handleKeydown}
  on:beforeunload={(event) => { if (!isTauri && files.some((file) => file.content !== file.savedContent)) event.preventDefault(); }}
  on:dragover={(event) => { event.preventDefault(); dragActive = true; }}
  on:dragleave={(event) => { if (event.relatedTarget === null) dragActive = false; }}
  on:drop={onDrop}
/>

<input class="hidden-input" bind:this={fileInput} type="file" accept=".md,.markdown" multiple on:change={(e) => ingest(e.currentTarget.files ?? [])} />
<input class="hidden-input" bind:this={folderInput} type="file" accept=".md,.markdown" multiple webkitdirectory on:change={(e) => ingest(e.currentTarget.files ?? [])} />

<div class:drag-active={dragActive} class:dark-shell={activeTheme.mode === "dark"} class="app-shell">
  <header
    class:desktop-titlebar={isTauri}
    class="topbar"
    data-tauri-drag-region
  >
    <button class="brand" aria-label="Markdown Studio home" on:click={goHome}>
      <span class="brand-mark">M<span>↓</span></span>
      <span>Markdown Studio</span>
    </button>
    <button
      class="titlebar-drag"
      data-tauri-drag-region
      aria-label="Drag window; double-click to maximize"
      on:dblclick={toggleMaximizeWindow}
    >
      {#if files.length}<span>{projectName}</span>{/if}
    </button>
    {#if files.length}
      <div class="titlebar-search">
        <svg viewBox="0 0 24 24"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 5 5"/></svg>
        <input
          bind:value={search}
          on:input={() => scheduleSearch()}
          on:keydown={(event) => {
            if (event.key === "Enter") focusMatch(event.shiftKey ? -1 : 1);
            if (event.key === "Escape") { search = ""; scheduleSearch(0); event.currentTarget.blur(); }
          }}
          placeholder="Search document"
          aria-label="Search document"
        />
        {#if search}<small>{searchCount ? `${searchIndex + 1}/${searchCount}${searchLimited ? "+" : ""}` : "0/0"}</small>{/if}
        {#if searchCount}
          <button aria-label="Previous match" on:click={() => focusMatch(-1)}>↑</button>
          <button aria-label="Next match" on:click={() => focusMatch(1)}>↓</button>
        {/if}
      </div>
    {/if}
    <div class="top-actions">
      {#if files.length}
        <button
          class:active={showSidePanels}
          class="workspace-toggle"
          title={showSidePanels ? "Hide side panels" : "Show organizer and settings"}
          aria-label={showSidePanels ? "Hide side panels" : "Show organizer and settings"}
          aria-pressed={showSidePanels}
          on:click={() => showSidePanels = !showSidePanels}
        >
          <svg viewBox="0 0 20 20"><rect x="2.5" y="3" width="15" height="14" rx="2"/><path d="M6.5 3v14M13.5 3v14"/></svg>
          <span>{showSidePanels ? "Clean view" : "More"}</span>
        </button>
        <button
          class:active={editMode}
          class="workspace-toggle"
          title={editMode ? "Close the editor" : "Edit Markdown source"}
          aria-label={editMode ? "Close the editor" : "Edit Markdown source"}
          aria-pressed={editMode}
          on:click={toggleEditMode}
        >
          <svg viewBox="0 0 20 20"><path d="m13.2 3.6 3.2 3.2L7 16.2l-4 .8.8-4z"/></svg>
          <span>Edit</span>
        </button>
        {#if editMode}
          <button
            class="workspace-toggle save-button"
            title={activeFile?.path ?? "Choose where to save"}
            aria-label="Save file"
            on:click={saveActiveFile}
          >
            <svg viewBox="0 0 20 20"><path d="M4 3h10l3 3v11H4zM7 3v4h6V3M7 17v-5h6v5"/></svg>
            <span>Save</span>
            {#if activeDirty}<i class="save-dot"></i>{/if}
          </button>
        {/if}
        <button class="icon-button" title="Save project" aria-label="Save project" on:click={saveProject}>
          <svg viewBox="0 0 24 24"><path d="M5 4h12l2 2v14H5zM8 4v6h8V4M8 20v-6h8v6"/></svg>
        </button>
      {/if}
      <button class="quiet-button" on:click={openLibrary}>Library</button>
      <button
        class:dark={activeTheme.mode === "dark"}
        class="theme-toggle"
        title={`Switch to ${activeTheme.mode === "dark" ? "light" : "dark"} theme`}
        aria-label={`Switch to ${activeTheme.mode === "dark" ? "light" : "dark"} theme`}
        aria-pressed={activeTheme.mode === "dark"}
        on:click={toggleThemeMode}
      >
        <svg class="sun-icon" viewBox="0 0 20 20"><circle cx="10" cy="10" r="3"/><path d="M10 2v2m0 12v2M2 10h2m12 0h2M4.3 4.3l1.4 1.4m8.6 8.6 1.4 1.4m0-11.4-1.4 1.4m-8.6 8.6-1.4 1.4"/></svg>
        <span></span>
        <svg class="moon-icon" viewBox="0 0 20 20"><path d="M15.7 12.8A6 6 0 0 1 7.2 4.3 6.1 6.1 0 1 0 15.7 12.8z"/></svg>
      </button>
      {#if files.length}
        <button class="publish-button" on:click={() => showPublish = true}>
          Publish <span>↗</span>
        </button>
      {/if}
      {#if isTauri}
        <div class="window-divider"></div>
        <div class="window-controls">
          <button title="Minimize" aria-label="Minimize window" on:click={minimizeWindow}>
            <svg viewBox="0 0 12 12"><path d="M2 6.5h8"/></svg>
          </button>
          <button title="Maximize" aria-label="Maximize window" on:click={toggleMaximizeWindow}>
            <svg viewBox="0 0 12 12"><rect x="2.25" y="2.25" width="7.5" height="7.5"/></svg>
          </button>
          <button class="close-control" title="Close" aria-label="Close window" on:click={closeWindow}>
            <svg viewBox="0 0 12 12"><path d="m2.5 2.5 7 7m0-7-7 7"/></svg>
          </button>
        </div>
      {/if}
    </div>
  </header>

  {#if files.length === 0}
    <main class="empty-state">
      <div class="ambient-line line-one"></div>
      <div class="ambient-line line-two"></div>
      <section class="welcome">
        <div class="eyebrow">A quieter place for your words</div>
        <h1>Read. Arrange.<br /><em>Publish beautifully.</em></h1>
        <p>Turn Markdown into considered documents—without turning your desk into a publishing suite.</p>
        <div
          class="drop-zone"
          class:active={dragActive}
          role="button"
          tabindex="0"
          on:keydown={(event) => { if (event.key === "Enter" || event.key === " ") chooseFiles(); }}
          on:drop={onDrop}
          on:dragover={(e) => e.preventDefault()}
        >
          <div class="drop-icon">
            <svg viewBox="0 0 32 32"><path d="M16 21V5m0 0-6 6m6-6 6 6M6 20v7h20v-7"/></svg>
          </div>
          <strong>Drop your Markdown here</strong>
          <span>one file, a handful, or an entire folder</span>
          <div class="drop-actions">
            <button on:click={chooseFiles}>Choose files</button>
            <button on:click={chooseFolder}>Choose folder</button>
          </div>
        </div>
        <div class="feature-strip">
          <span><b>6</b> editorial themes</span>
          <span><b>3</b> export formats</span>
          <span><b>0</b> cloud uploads</span>
        </div>
      </section>
    </main>
  {:else}
    <main class:clean-view={!showSidePanels} class="workspace">
      <aside class="organizer" class:single={files.length === 1}>
        <div class="project-heading">
          <span>{files.length === 1 ? "Document" : "Collection"}</span>
          <input bind:value={projectName} aria-label="Project name" />
          <small>{files.length} {files.length === 1 ? "file" : "files"} · local</small>
        </div>
        <div class="file-list">
          {#each files as file, index (file.id)}
            <button
              class:active={file.id === activeId}
              class="file-card"
              draggable="true"
              on:dragstart={() => draggingId = file.id}
              on:dragover={(event) => { event.preventDefault(); reorder(file.id); }}
              on:click={() => activeId = file.id}
            >
              <span class="grip">⠿</span>
              <span class="file-number">{String(index + 1).padStart(2, "0")}</span>
              <span class="file-name"><span>{file.name}</span>{#if file.content !== file.savedContent}<i class="dirty-dot"></i>{/if}</span>
              <span
                class="remove"
                role="button"
                tabindex="0"
                on:keydown={(event) => { if (event.key === "Enter" || event.key === " ") removeFile(file.id); }}
                on:click|stopPropagation={() => removeFile(file.id)}
              >×</span>
            </button>
          {/each}
        </div>
        <button class="add-file" on:click={chooseFiles}>＋ Add Markdown</button>
        <div class="aside-footer">
          <span>Drag to reorder</span>
          <button on:click={() => showSettings = !showSettings}>Document settings <span>⌘</span></button>
        </div>
      </aside>

      <section bind:this={readerFrameEl} class:editing={editMode} class="reader-frame" style={documentStyle} data-mode={activeTheme.mode}>
        {#if editMode}
          <div class="editor-pane" bind:this={editorPaneEl}></div>
        {/if}
        <div id="document-reader" bind:this={readerEl} class="reader" on:scroll={onReaderScroll}>
          <article bind:this={articleEl} class="markdown-body">
            {@html renderedHtml}
          </article>
          <footer class="reader-end"><span>End of {activeFile?.name}</span></footer>
        </div>
        {#if selectionMenu.visible}
          <div
            class="selection-menu"
            role="toolbar"
            tabindex="-1"
            aria-label="Selection actions"
            style={`left:${selectionMenu.x}px;top:${selectionMenu.y}px`}
            on:pointerdown|preventDefault
          >
            <button title="Read out loud" aria-label="Read selection out loud" on:click={speakSelection}>
              <svg viewBox="0 0 24 24"><path d="M11 5 6 9H2v6h4l5 4z"/><path d="M15.5 8.5a5 5 0 0 1 0 7"/><path d="M18.6 5.4a9 9 0 0 1 0 13.2"/></svg>
            </button>
            <button title="Copy" aria-label="Copy selection" on:click={copySelection}>
              <svg viewBox="0 0 24 24"><rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
            </button>
          </div>
        {/if}
        {#if speaking}
          <div class="speech-pill" role="status">
            <span class="speech-wave" aria-hidden="true"><i></i><i></i><i></i></span>
            <span>Reading aloud</span>
            <button on:click={stopSpeaking}>Stop</button>
          </div>
        {/if}
        {#if showMinimap && !editMode}
          <div
            bind:this={minimapEl}
            class="document-minimap"
            role="scrollbar"
            aria-label="Document position"
            aria-controls="document-reader"
            aria-orientation="vertical"
            aria-valuemin="0"
            aria-valuemax="100"
            aria-valuenow={minimapScrollPercent}
            tabindex="0"
            on:pointerdown={navigateFromMinimap}
            on:keydown={(event) => {
              if (event.key === "ArrowDown") readerEl.scrollBy({ top: readerEl.clientHeight * .65, behavior: "smooth" });
              if (event.key === "ArrowUp") readerEl.scrollBy({ top: -readerEl.clientHeight * .65, behavior: "smooth" });
            }}
          >
            <div class="minimap-map" aria-hidden="true">
              {#each minimapBlocks as block}
                <i
                  class={`minimap-block ${block.kind}`}
                  style={`top:${block.top}%;height:${block.height}%;width:${block.width}%`}
                ></i>
              {/each}
            </div>
            <div class="minimap-viewport" style={`top:${minimapViewportTop}px;height:${minimapViewportHeight}px`}></div>
          </div>
        {/if}
        {#if showDocumentStats}
          <div class="document-stats" role="status" aria-label={`Statistics for ${activeFile?.name ?? "document"}`}>
            <span class="stats-heading"><b>Insights</b><small>{activeFile?.name}</small></span>
            <span title={`${documentStats.words.toLocaleString()} words`}><b>{formatStat(documentStats.words)}</b><small>Words</small></span>
            <span title={`${documentStats.characters.toLocaleString()} characters`}><b>{formatStat(documentStats.characters)}</b><small>Characters</small></span>
            <span title="Estimated at approximately 450 words per page"><b>{documentStats.pages}</b><small>Pages</small></span>
            <span title="Estimated at approximately 225 words per minute"><b>{documentStats.readingMinutes}m</b><small>Read time</small></span>
            <span><b>{documentStats.headings}</b><small>Headings</small></span>
            <span><b>{documentStats.links}</b><small>Links</small></span>
            <span><b>{documentStats.codeBlocks}</b><small>Code</small></span>
            <button title="Hide document insights" aria-label="Hide document insights" on:click={() => showDocumentStats = false}>×</button>
          </div>
        {/if}
      </section>

      <aside class="theme-rail" class:open={showSettings}>
        <div class="rail-title">
          <span>Settings</span>
          <button on:click={() => showSettings = false}>×</button>
        </div>
        <section class="settings-section typography-settings">
          <div class="settings-label"><span>Typography</span><small>Live preview</small></div>
          <label class="setting-name" for="font-size-input">Font size</label>
          <div class="font-size-control">
            <button aria-label="Decrease font size" on:click={() => fontSize = Math.max(13, fontSize - 1)}>−</button>
            <input id="font-size-input" type="number" min="13" max="28" bind:value={fontSize} />
            <span>px</span>
            <button aria-label="Increase font size" on:click={() => fontSize = Math.min(28, fontSize + 1)}>＋</button>
          </div>
          <input class="font-size-slider" aria-label="Font size" type="range" min="13" max="28" step="1" bind:value={fontSize} />
          <span class="setting-name">Typeface</span>
          <div class="typeface-options">
            <button class:active={typeface === "serif"} on:click={() => typeface = "serif"}><b style={`font-family:${typefaces.serif}`}>Aa</b><span>Serif</span></button>
            <button class:active={typeface === "sans"} on:click={() => typeface = "sans"}><b style={`font-family:${typefaces.sans}`}>Aa</b><span>Sans</span></button>
            <button class:active={typeface === "mono"} on:click={() => typeface = "mono"}><b style={`font-family:${typefaces.mono}`}>Aa</b><span>Mono</span></button>
          </div>
          <span class="setting-name">Content width</span>
          <div class="content-width-options" aria-label="Content width">
            <button class:active={contentWidth === "theme"} aria-pressed={contentWidth === "theme"} on:click={() => contentWidth = "theme"}>
              <i class="width-preview width-preview-theme"></i><span>Theme</span>
            </button>
            <button class:active={contentWidth === "wide"} aria-pressed={contentWidth === "wide"} on:click={() => contentWidth = "wide"}>
              <i class="width-preview width-preview-wide"></i><span>Wide</span>
            </button>
            <button class:active={contentWidth === "extra-wide"} aria-pressed={contentWidth === "extra-wide"} on:click={() => contentWidth = "extra-wide"}>
              <i class="width-preview width-preview-extra"></i><span>Extra wide</span>
            </button>
          </div>
        </section>
        <section class="settings-section navigation-settings">
          <div class="settings-label"><span>Navigation</span><small>Reader aid</small></div>
          <button
            class="setting-toggle"
            class:active={showMinimap}
            aria-pressed={showMinimap}
            on:click={() => showMinimap = !showMinimap}
          >
            <span><b>Document minimap</b><small>See your position in the full page</small></span>
            <i><span></span></i>
          </button>
        </section>
        <section class="settings-section insights-settings">
          <div class="settings-label"><span>Insights</span><small>Live analysis</small></div>
          <button
            class="setting-toggle"
            class:active={showDocumentStats}
            aria-pressed={showDocumentStats}
            on:click={() => showDocumentStats = !showDocumentStats}
          >
            <span><b>Document statistics</b><small>Words, pages, reading time, and structure</small></span>
            <i><span></span></i>
          </button>
        </section>
        <section class="settings-section theme-settings">
          <div class="settings-label"><span>Document theme</span><small>{themes.length} styles</small></div>
          <div class="theme-list">
            {#each themes as theme}
              <button class:active={theme.id === activeTheme.id} on:click={() => activeTheme = theme}>
                <span class="theme-swatch" style={`--swatch-paper:${theme.paper};--swatch-ink:${theme.ink};--swatch-accent:${theme.accent}`}>
                  <i></i><i></i><i></i>
                </span>
                <span>{theme.name}<small>{theme.mode}</small></span>
                {#if theme.id === activeTheme.id}<b>✓</b>{/if}
              </button>
            {/each}
          </div>
        </section>
        <div class="rail-note">Theme tokens travel with your export.</div>
      </aside>
    </main>
  {/if}

  {#if dragActive}
    <div class="drop-overlay">
      <div><span>↓</span><strong>Release to open</strong><small>Markdown stays on this device</small></div>
    </div>
  {/if}

  {#if showPublish}
    <div class="modal-backdrop" role="presentation" on:click={() => showPublish = false}>
      <div class="publish-sheet" role="dialog" tabindex="-1" aria-modal="true" aria-label="Publish document" on:keydown={(event) => { if (event.key === "Escape") showPublish = false; }} on:click|stopPropagation>
        <header>
          <div><small>Final details</small><h2>Publish your work</h2></div>
          <button on:click={() => showPublish = false}>×</button>
        </header>
        <div class="publish-body">
          <div class="format-list">
            <button on:click={printDocument}><b>PRINT</b><span>Open the system print dialog</span><i>→</i></button>
            <button on:click={() => exportDocument("pdf")}><b>PDF</b><span>Print-ready, themed pages</span><i>→</i></button>
            <button on:click={() => exportDocument("docx")}><b>DOCX</b><span>Editable in Word and Pages</span><i>→</i></button>
            <button on:click={() => exportDocument("html")}><b>HTML</b><span>One self-contained file</span><i>→</i></button>
            <button on:click={() => exportDocument("md")}><b>MD</b><span>Combine every source into one Markdown file</span><i>→</i></button>
          </div>
          <div class="export-settings">
            {#if files.length > 1}
              <label><input type="checkbox" bind:checked={printAllFiles} /><span>Print entire collection</span></label>
            {/if}
            <label><input type="checkbox" bind:checked={exportOptions.toc} /><span>Table of contents</span></label>
            <label><input type="checkbox" bind:checked={exportOptions.pageNumbers} /><span>Page numbers</span></label>
            <div class="field"><span>Header · center</span><input bind:value={exportOptions.headerCenter} placeholder={"{title}"} /></div>
            <div class="field"><span>Footer · center</span><input bind:value={exportOptions.footerCenter} placeholder={"{page}"} /></div>
            <p>Tokens: <code>{"{title}"}</code> <code>{"{page}"}</code> <code>{"{date}"}</code></p>
          </div>
        </div>
      </div>
    </div>
  {/if}

  {#if showLibrary}
    <div class="modal-backdrop" role="presentation" on:click={() => showLibrary = false}>
      <div class="library-sheet" role="dialog" tabindex="-1" aria-modal="true" aria-label="Project library" on:keydown={(event) => { if (event.key === "Escape") showLibrary = false; }} on:click|stopPropagation>
        <header><div><small>Saved locally</small><h2>Your library</h2></div><button on:click={() => showLibrary = false}>×</button></header>
        {#if savedProjects.length}
          <div class="saved-list">
            {#each savedProjects as project}
              <button on:click={() => loadProject(project.id)}>
                <span class="saved-mark">M↓</span>
                <span><b>{project.name}</b><small>{project.file_count} files · {new Date(project.updated_at).toLocaleDateString()}</small></span>
                <i>→</i>
              </button>
            {/each}
          </div>
        {:else}
          <div class="library-empty"><span>□</span><b>No saved projects yet</b><p>Open Markdown, make it yours, then save it here.</p></div>
        {/if}
      </div>
    </div>
  {/if}

  {#if printHtml}
    <div class="print-document" style={documentStyle}>
      {@html printHtml}
    </div>
  {/if}

  {#if busy}<div class="busy-indicator"><span></span>Working locally…</div>{/if}
  {#if toast}<div class="toast">{toast}</div>{/if}
</div>
