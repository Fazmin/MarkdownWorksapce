export type ThemeTokens = {
  id: string;
  name: string;
  mode: "light" | "dark";
  paper: string;
  ink: string;
  muted: string;
  accent: string;
  rule: string;
  codeBg: string;
  fontBody: string;
  fontHeading: string;
  baseSize: number;
  lineHeight: number;
  measure: number;
  paragraphGap: number;
};

export const themes: ThemeTokens[] = [
  {
    id: "classic",
    name: "Classic",
    mode: "light",
    paper: "#fbfaf6",
    ink: "#272621",
    muted: "#77736a",
    accent: "#b64a2e",
    rule: "#d8d3c8",
    codeBg: "#f0ede5",
    fontBody: "'Source Serif 4', Georgia, serif",
    fontHeading: "'DM Sans', sans-serif",
    baseSize: 18,
    lineHeight: 1.72,
    measure: 690,
    paragraphGap: 1.15
  },
  {
    id: "newsprint",
    name: "Newsprint",
    mode: "light",
    paper: "#f0eee6",
    ink: "#161615",
    muted: "#5f5e58",
    accent: "#bb1d24",
    rule: "#20201e",
    codeBg: "#e3e0d6",
    fontBody: "'Libre Baskerville', Georgia, serif",
    fontHeading: "'Barlow Condensed', sans-serif",
    baseSize: 17,
    lineHeight: 1.62,
    measure: 720,
    paragraphGap: 1
  },
  {
    id: "sepia",
    name: "Sepia",
    mode: "light",
    paper: "#f3e7cf",
    ink: "#3b2d21",
    muted: "#806d5a",
    accent: "#9a4f2a",
    rule: "#cdbb9d",
    codeBg: "#e8d7b8",
    fontBody: "'Crimson Pro', Georgia, serif",
    fontHeading: "'Crimson Pro', Georgia, serif",
    baseSize: 19,
    lineHeight: 1.76,
    measure: 660,
    paragraphGap: 1.2
  },
  {
    id: "midnight",
    name: "Midnight",
    mode: "dark",
    paper: "#121619",
    ink: "#e9e8e1",
    muted: "#929ba0",
    accent: "#efb35d",
    rule: "#343b3f",
    codeBg: "#1c2327",
    fontBody: "'Source Serif 4', Georgia, serif",
    fontHeading: "'DM Sans', sans-serif",
    baseSize: 18,
    lineHeight: 1.72,
    measure: 690,
    paragraphGap: 1.15
  },
  {
    id: "solar",
    name: "Solar",
    mode: "dark",
    paper: "#002b36",
    ink: "#eee8d5",
    muted: "#93a1a1",
    accent: "#2aa198",
    rule: "#33545c",
    codeBg: "#073642",
    fontBody: "'IBM Plex Sans', sans-serif",
    fontHeading: "'IBM Plex Sans', sans-serif",
    baseSize: 17,
    lineHeight: 1.68,
    measure: 700,
    paragraphGap: 1.05
  },
  {
    id: "minimal",
    name: "Minimal",
    mode: "light",
    paper: "#ffffff",
    ink: "#181818",
    muted: "#737373",
    accent: "#1757d7",
    rule: "#e2e2e2",
    codeBg: "#f5f5f5",
    fontBody: "'IBM Plex Sans', sans-serif",
    fontHeading: "'IBM Plex Sans', sans-serif",
    baseSize: 17,
    lineHeight: 1.65,
    measure: 680,
    paragraphGap: 1
  }
];

export const typefaces = {
  serif: "'Source Serif 4', Georgia, serif",
  sans: "'IBM Plex Sans', sans-serif",
  mono: "'IBM Plex Mono', Consolas, monospace"
};
