import loader from '@monaco-editor/loader';
import type { Monaco } from '@monaco-editor/loader';

// Monaco is fetched once from the CDN (no npm bundle) and shared between the
// source editor and the diff view. The npm `monaco-editor` package is used for
// types only, so it never lands in the main bundle.
let configured = false;
let themeDefined = false;

export async function getMonaco(): Promise<Monaco> {
  if (!configured) {
    loader.config({
      paths: {
        vs: 'https://cdn.jsdelivr.net/npm/monaco-editor@0.45.0/min/vs',
      },
    });
    configured = true;
  }

  const monaco = await loader.init();

  if (!themeDefined) {
    monaco.editor.defineTheme('alpine-theme', {
      base: 'vs-dark',
      inherit: true,
      rules: [
        { token: 'keyword', foreground: '95B2F8' },
        { token: 'comment', foreground: '506695' },
        { token: 'string', foreground: 'B5C0F3' },
        { token: 'number', foreground: '5271C0' },
        { token: 'type', foreground: '95B2F8' },
      ],
      colors: {
        'editor.background': '#28254F',
        'editor.foreground': '#ffffff',
        'editorCursor.foreground': '#95B2F8',
        'editor.lineHighlightBackground': '#283B60',
        'editorLineNumber.foreground': '#506695',
        'editorIndentGuide.background': '#506695',
      },
    });
    themeDefined = true;
  }

  return monaco;
}
