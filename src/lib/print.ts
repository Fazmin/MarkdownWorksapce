export type PageNumberPosition = "left" | "center" | "right";
export type PageNumberFormat = "number" | "page" | "page-of";
export type PageMarginPreset = "narrow" | "normal" | "wide";

export type ExportOptions = {
  toc: boolean;
  pageNumbers: boolean;
  pageNumberPosition: PageNumberPosition;
  pageNumberFormat: PageNumberFormat;
  margins: PageMarginPreset;
  /** Printed body text size in points, independent of the on-screen reader size. */
  textSize: number;
  /** Start every top-level section (H1 and H2) on a new page. */
  sectionBreaks: boolean;
  headerLeft: string;
  headerCenter: string;
  headerRight: string;
  footerLeft: string;
  footerCenter: string;
  footerRight: string;
};

export const defaultExportOptions: ExportOptions = {
  toc: true,
  pageNumbers: true,
  pageNumberPosition: "center",
  pageNumberFormat: "number",
  margins: "normal",
  textSize: 11,
  sectionBreaks: false,
  headerLeft: "",
  headerCenter: "{title}",
  headerRight: "",
  footerLeft: "",
  footerCenter: "",
  footerRight: ""
};

/** Page margins in millimetres, mirrored by the Rust exporters. */
export const marginPresets: Record<PageMarginPreset, { label: string; top: number; right: number; bottom: number; left: number }> = {
  narrow: { label: "Narrow", top: 12.7, right: 12.7, bottom: 12.7, left: 12.7 },
  normal: { label: "Normal", top: 25.4, right: 25.4, bottom: 25.4, left: 25.4 },
  wide: { label: "Wide", top: 25.4, right: 50.8, bottom: 25.4, left: 50.8 }
};

export const pageNumberFormats: Record<PageNumberFormat, { label: string; template: string }> = {
  number: { label: "1", template: "{page}" },
  page: { label: "Page 1", template: "Page {page}" },
  "page-of": { label: "Page 1 of 9", template: "Page {page} of {pages}" }
};

export const textSizeRange = { min: 8, max: 16, step: 0.5, base: 11 };

const positions: PageNumberPosition[] = ["left", "center", "right"];

export function clampTextSize(value: number) {
  const size = Number.isFinite(value) ? value : textSizeRange.base;
  return Math.min(textSizeRange.max, Math.max(textSizeRange.min, size));
}

/** Fills in options added after a project was saved and drops the old hidden footer default. */
export function normalizeExportOptions(value: Partial<ExportOptions> | null | undefined): ExportOptions {
  const options = { ...defaultExportOptions, ...(value ?? {}) };
  const legacy = !value || !("pageNumberPosition" in value);
  if (legacy) {
    // Older versions always placed "{page}" in the (hidden) right footer slot and drove the
    // centre slot from the page-number checkbox; the toggle now owns page numbers.
    if (options.footerRight.trim() === "{page}") options.footerRight = "";
    if (options.footerCenter.trim() === "{page}") options.footerCenter = "";
  }
  if (!positions.includes(options.pageNumberPosition)) options.pageNumberPosition = "center";
  if (!(options.pageNumberFormat in pageNumberFormats)) options.pageNumberFormat = "number";
  if (!(options.margins in marginPresets)) options.margins = "normal";
  options.textSize = clampTextSize(Number(options.textSize));
  options.sectionBreaks = Boolean(options.sectionBreaks);
  return options;
}

/** Header and footer templates per slot, with page numbers merged into their footer slot. */
export function runningSlots(options: ExportOptions) {
  const header = [options.headerLeft, options.headerCenter, options.headerRight].map((value) => value.trim());
  const footer = [options.footerLeft, options.footerCenter, options.footerRight].map((value) => value.trim());
  if (options.pageNumbers) {
    const slot = positions.indexOf(options.pageNumberPosition);
    const template = pageNumberFormats[options.pageNumberFormat].template;
    footer[slot] = footer[slot] ? `${footer[slot]} · ${template}` : template;
  }
  return { header, footer };
}

function localDate() {
  const now = new Date();
  return [now.getFullYear(), now.getMonth() + 1, now.getDate()].map((part) => String(part).padStart(2, "0")).join("-");
}

function cssString(value: string) {
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/</g, "\\3c ").replace(/[\r\n]+/g, " ")}"`;
}

/** Converts a header/footer template into a CSS `content` value using the page counters. */
export function cssContent(template: string, title: string) {
  const text = template.replaceAll("{title}", title).replaceAll("{date}", localDate());
  const parts = text
    .split(/(\{pages?\})/)
    .filter(Boolean)
    .map((part) => part === "{page}" ? "counter(page)" : part === "{pages}" ? "counter(pages)" : cssString(part));
  return parts.length ? parts.join(" ") : '""';
}

function escapeHtml(value: string) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

/** Estimated width of a word in em: capitals and symbols run wider than lowercase letters. */
function wordWidthEm(word: string) {
  let width = 0;
  for (const character of word) width += /[A-Z@_%&#WMwm]/.test(character) ? .74 : .54;
  return width;
}

/**
 * Stops table columns collapsing mid-word. Long text in cells may break anywhere so a table can
 * never push past the margins, but the browser then squeezes short columns down to single
 * letters. Each cell gets a minimum width for its longest word, limited so the minimums of all
 * columns together still fit the printed line.
 */
function fitTablesToPage(root: DocumentFragment, options: ExportOptions) {
  const margins = marginPresets[options.margins] ?? marginPresets.normal;
  // A4 is the narrowest common paper; 2.835 converts millimetres to points.
  const lineWidthEm = (210 - margins.left - margins.right) * 2.835 / clampTextSize(options.textSize);
  // Table text is .88em with .7em of padding either side; bold header text is a little wider.
  const tableEm = .88;
  const cellPaddingEm = 1.4;
  const widestWord = (cell: HTMLTableCellElement) => {
    const scale = cell.tagName === "TH" ? 1.1 : 1;
    return scale * Math.max(0, ...(cell.textContent ?? "").split(/\s+/).map(wordWidthEm));
  };
  root.querySelectorAll("table").forEach((table) => {
    const rows = Array.from(table.rows);
    const columnWidths: number[] = [];
    for (const row of rows) {
      Array.from(row.cells).forEach((cell, index) => {
        columnWidths[index] = Math.max(columnWidths[index] ?? 0, widestWord(cell));
      });
    }
    const limit = columnWidthLimit(columnWidths, lineWidthEm / tableEm - columnWidths.length * cellPaddingEm);
    for (const row of rows) {
      for (const cell of Array.from(row.cells)) {
        cell.style.minWidth = `${(Math.min(widestWord(cell), limit) + cellPaddingEm).toFixed(2)}em`;
      }
    }
  });
}

/**
 * The largest column width that lets every column keep its words whole within the available
 * width: columns with shorter words take only what they need, and only the columns with the
 * longest words are limited.
 */
function columnWidthLimit(columnWidths: number[], available: number) {
  const sorted = [...columnWidths].sort((a, b) => a - b);
  let remaining = available;
  for (let index = 0; index < sorted.length; index++) {
    const columnsLeft = sorted.length - index;
    if (sorted[index] * columnsLeft > remaining) return Math.max(1.6, remaining / columnsLeft);
    remaining -= sorted[index];
  }
  return Infinity;
}

/**
 * Builds the printable document: one article per file, preceded by a contents page of clickable
 * links to each file and its H1/H2 sections when requested and there is something to list.
 */
export function buildPrintDocument(sources: { name: string; html: string }[], options: ExportOptions) {
  const includeContents = options.toc;
  const contents: string[] = [];
  const articles = sources.map((source, sourceIndex) => {
    const template = document.createElement("template");
    template.innerHTML = source.html;
    fitTablesToPage(template.content, options);
    const sectionId = `print-file-${sourceIndex}`;
    if (sources.length > 1) {
      contents.push(`<a class="toc-file" href="#${sectionId}">${escapeHtml(source.name)}</a>`);
    }
    template.content.querySelectorAll("h1, h2").forEach((heading, headingIndex) => {
      heading.id ||= `${sectionId}-${headingIndex}`;
      const text = heading.textContent?.trim();
      if (text) {
        contents.push(`<a class="toc-${heading.tagName.toLowerCase()}" href="#${heading.id}">${escapeHtml(text)}</a>`);
      }
    });
    const label = `${String(sourceIndex + 1).padStart(2, "0")} · ${escapeHtml(source.name)}`;
    return `<article id="${sectionId}" class="markdown-body print-source"><div class="print-source-label">${label}</div>${template.innerHTML}</article>`;
  });
  const toc = includeContents && contents.length > 1
    ? `<nav class="markdown-body print-source print-toc"><h1>Contents</h1>${contents.join("")}</nav>`
    : "";
  return toc + articles.join("");
}

/**
 * The stylesheet for the print layout: margins from the chosen preset, the printed text size,
 * and header, footer, and page-number text in the page-margin boxes (Chromium 131+, which
 * includes WebView2). Pages have no background and dark ink, so a dark theme's accent colour is
 * darkened to stay readable on paper.
 */
export function printPageCss(options: ExportOptions, title: string, theme: { mode: string; accent: string }) {
  const margins = marginPresets[options.margins] ?? marginPresets.normal;
  const { header, footer } = runningSlots(options);
  const accent = theme.mode === "dark" ? `color-mix(in srgb, ${theme.accent} 58%, #000)` : theme.accent;
  const boxStyle = 'font: 500 8.5pt "DM Sans", system-ui, sans-serif; color: #6b6b6b; letter-spacing: .02em;';
  const boxes = [
    ...header.map((template, index) => [`top-${positions[index]}`, template]),
    ...footer.map((template, index) => [`bottom-${positions[index]}`, template])
  ]
    .filter(([, template]) => template)
    .map(([name, template]) => `@${name} { content: ${cssContent(template, title)}; ${boxStyle} }`)
    .join("\n  ");
  const rules = [
    `@page {\n  margin: ${margins.top}mm ${margins.right}mm ${margins.bottom}mm ${margins.left}mm;\n  ${boxes}\n}`,
    `@media print { body.printing .print-document { --reader-size: ${clampTextSize(options.textSize)}pt !important; --accent: ${accent} !important; } }`
  ];
  if (options.sectionBreaks) {
    rules.push(`@media print {
  body.printing .print-source:not(.print-toc) > :is(h1, h2) { break-before: page; }
  body.printing .print-source:not(.print-toc) > .print-source-label + :is(h1, h2),
  body.printing .print-source:not(.print-toc) > :is(h1, h2) + h2 { break-before: auto; }
}`);
  }
  return rules.join("\n");
}
