export type PageNumberPosition = "left" | "center" | "right";
export type PageNumberFormat = "number" | "page" | "page-of";
export type PageMarginPreset = "narrow" | "normal" | "wide";

export type ExportOptions = {
  toc: boolean;
  pageNumbers: boolean;
  pageNumberPosition: PageNumberPosition;
  pageNumberFormat: PageNumberFormat;
  margins: PageMarginPreset;
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

const positions: PageNumberPosition[] = ["left", "center", "right"];

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

/**
 * The `@page` rule used when printing: margins from the chosen preset, with header, footer, and
 * page-number text drawn in the page-margin boxes (Chromium 131+, which includes WebView2).
 */
export function printPageCss(options: ExportOptions, title: string) {
  const margins = marginPresets[options.margins] ?? marginPresets.normal;
  const { header, footer } = runningSlots(options);
  const boxStyle = 'font: 500 8.5pt "DM Sans", system-ui, sans-serif; color: #6b6b6b; letter-spacing: .02em;';
  const boxes = [
    ...header.map((template, index) => [`top-${positions[index]}`, template]),
    ...footer.map((template, index) => [`bottom-${positions[index]}`, template])
  ]
    .filter(([, template]) => template)
    .map(([name, template]) => `@${name} { content: ${cssContent(template, title)}; ${boxStyle} }`)
    .join("\n  ");
  return `@page {\n  margin: ${margins.top}mm ${margins.right}mm ${margins.bottom}mm ${margins.left}mm;\n  ${boxes}\n}`;
}
