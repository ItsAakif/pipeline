import { onMount, onCleanup, createSignal, createEffect, Show, Component } from 'solid-js';
import type * as Monaco from 'monaco-editor';
import { getMonaco } from '../lib/monaco';
import { countLines, exceedsLineLimit, exceedsByteLimit, MAX_LINE_LIMIT, LINE_LIMIT_MESSAGE, BYTE_LIMIT_MESSAGE } from '../lib/lines';

interface SourceEditorProps {
  initialValue?: string;
  onCodeChange?: (code: string) => void;
  onCompile?: () => void;
  isCompiling?: boolean;
}

const DEFAULT_CODE = `#include <stdio.h>

int main() {
    int x = 10;
    int y = 20;
    int sum = x + y;

    printf("Sum: %d\\n", sum);

    return 0;
}`;

export const SourceEditor: Component<SourceEditorProps> = (props) => {
  const [editorRef, setEditorRef] = createSignal<Monaco.editor.IStandaloneCodeEditor>();
  const [code, setCode] = createSignal(props.initialValue || '');

  onMount(async () => {
    // Single Monaco load path: fetched once through the loader and shared with
    // the diff view. The npm package is imported for types only.
    const monaco = await getMonaco();

    const container = document.getElementById('monaco-container');
    if (!container) return;

    const initial = props.initialValue || DEFAULT_CODE;

    const editor = monaco.editor.create(container, {
      value: initial,
      language: 'c',
      theme: 'alpine-theme',
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      fontSize: 14,
      fontFamily: "'SF Mono', 'Consolas', monospace",
      lineNumbers: 'on',
      renderLineHighlight: 'line',
      tabSize: 2,
      automaticLayout: true,
    });

    setEditorRef(editor);
    setCode(initial);

    // Initialize parent with the starting value
    props.onCodeChange?.(initial);

    // Listen for changes
    editor.onDidChangeModelContent(() => {
      const newCode = editor.getValue();
      setCode(newCode);
      props.onCodeChange?.(newCode);
    });

    onCleanup(() => editor.dispose());
  });

  // Keep the editor in sync when the value changes from outside the editor
  // (e.g. restoring a permalink, loading a file, or picking an example).
  createEffect(() => {
    const editor = editorRef();
    const value = props.initialValue ?? '';
    if (editor && value !== editor.getValue()) {
      editor.setValue(value);
    }
  });

  // Warn before sending when either backend limit is exceeded.
  const limitWarning = (): string | null => {
    if (exceedsLineLimit(code())) return LINE_LIMIT_MESSAGE;
    if (exceedsByteLimit(code())) return BYTE_LIMIT_MESSAGE;
    return null;
  };

  return (
    <div class="source-editor-container">
      <div class="editor-header">
        <h2>C Source Code</h2>
        <Show when={limitWarning()}>
          <div class="warning">
            {limitWarning()}
          </div>
        </Show>
      </div>

      <div id="monaco-container" class="monaco-editor" />

      <div class="editor-footer">
        <button
          class="compile-button"
          onClick={() => props.onCompile?.()}
          disabled={props.isCompiling || exceedsLineLimit(code()) || exceedsByteLimit(code()) || code().trim().length === 0}
          aria-label="Compile source code"
        >
          {props.isCompiling ? 'Compiling...' : 'Compile'}
        </button>

        <div class="line-count">
          Lines: {countLines(code())} / {MAX_LINE_LIMIT}
        </div>
      </div>
    </div>
  );
};
