import { onMount, onCleanup, Component } from 'solid-js';
import type * as Monaco from 'monaco-editor';
import { getMonaco } from '../lib/monaco';

interface MonacoDiffProps {
  original: string;
  modified: string;
  language?: string;
}

// Read-only side-by-side diff backed by the same single Monaco loader instance
// used by the source editor.
export const MonacoDiff: Component<MonacoDiffProps> = (props) => {
  let containerRef: HTMLDivElement | undefined;
  let diffEditor: Monaco.editor.IStandaloneDiffEditor | undefined;
  let originalModel: Monaco.editor.ITextModel | undefined;
  let modifiedModel: Monaco.editor.ITextModel | undefined;

  onMount(async () => {
    const monaco = await getMonaco();
    if (!containerRef) return;

    diffEditor = monaco.editor.createDiffEditor(containerRef, {
      readOnly: true,
      renderSideBySide: true,
      automaticLayout: true,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      fontSize: 13,
      fontFamily: "'SF Mono', 'Consolas', monospace",
      theme: 'alpine-theme',
      renderOverviewRuler: false,
    });

    const language = props.language ?? 'plaintext';
    originalModel = monaco.editor.createModel(props.original, language);
    modifiedModel = monaco.editor.createModel(props.modified, language);
    diffEditor.setModel({ original: originalModel, modified: modifiedModel });

    onCleanup(() => {
      diffEditor?.dispose();
      originalModel?.dispose();
      modifiedModel?.dispose();
    });
  });

  return <div ref={containerRef} class="monaco-diff" aria-label="Optimized versus unoptimized difference" />;
};
