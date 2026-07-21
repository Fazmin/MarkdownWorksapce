import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { history, defaultKeymap, historyKeymap, indentWithTab } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { syntaxHighlighting, HighlightStyle } from "@codemirror/language";
import { tags } from "@lezer/highlight";

const markdownHighlight = HighlightStyle.define([
  { tag: tags.heading, color: "var(--ink)", fontWeight: "600" },
  { tag: tags.strong, fontWeight: "600" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.strikethrough, textDecoration: "line-through" },
  { tag: [tags.link, tags.url], color: "var(--accent)" },
  { tag: tags.monospace, color: "color-mix(in srgb, var(--accent) 72%, var(--ink))" },
  { tag: tags.quote, color: "var(--muted)", fontStyle: "italic" },
  { tag: [tags.meta, tags.processingInstruction], color: "var(--muted)" },
  { tag: tags.contentSeparator, color: "var(--accent)" }
]);

export type EditorHandle = {
  view: EditorView;
  setDoc: (text: string) => void;
  destroy: () => void;
};

export function createEditor(parent: HTMLElement, doc: string, onChange: (text: string) => void): EditorHandle {
  const extensions = [
    history(),
    keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
    markdown({ base: markdownLanguage }),
    syntaxHighlighting(markdownHighlight),
    EditorView.lineWrapping,
    EditorView.updateListener.of((update) => {
      if (update.docChanged) onChange(update.state.doc.toString());
    })
  ];
  const view = new EditorView({ parent, state: EditorState.create({ doc, extensions }) });
  return {
    view,
    setDoc: (text) => view.setState(EditorState.create({ doc: text, extensions })),
    destroy: () => view.destroy()
  };
}
