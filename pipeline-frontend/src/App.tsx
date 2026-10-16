import { createSignal, Component, Show, onMount, onCleanup, createEffect, For } from 'solid-js';
import { SourceEditor } from './components/SourceEditor';
import { EducationalViewer } from './components/EducationalViewer';
import type { StageType } from './components/EducationalViewer';
import { OutputView } from './components/OutputView';
import { buildShareUrl, readShareStateFromUrl } from './lib/share';
import { copyText } from './lib/download';
import { API_BASE_URL } from './lib/api';
import { exceedsLineLimit, exceedsByteLimit, LINE_LIMIT_MESSAGE, BYTE_LIMIT_MESSAGE } from './lib/lines';
import type { CompileResponse, CompilationStages } from './types';

// Code templates for the examples gallery
const TEMPLATES = {
  hello: {
    name: 'Hello World',
    code: `#include <stdio.h>

int main() {
    printf("Hello, World!\\n");
    return 0;
}`
  },
  arithmetic: {
    name: 'Simple Math',
    code: `#include <stdio.h>

int main() {
    int a = 10;
    int b = 20;
    int sum = a + b;
    printf("Sum: %d\\n", sum);
    return 0;
}`
  },
  loop: {
    name: 'For Loop',
    code: `#include <stdio.h>

int main() {
    for (int i = 0; i < 5; i++) {
        printf("Iteration %d\\n", i);
    }
    return 0;
}`
  },
  function: {
    name: 'Function Call',
    code: `#include <stdio.h>

int add(int x, int y) {
    return x + y;
}

int main() {
    int result = add(5, 10);
    printf("Result: %d\\n", result);
    return 0;
}`
  },
  array: {
    name: 'Array Operations',
    code: `#include <stdio.h>

int main() {
    int numbers[5] = {1, 2, 3, 4, 5};
    int sum = 0;

    for (int i = 0; i < 5; i++) {
        sum += numbers[i];
    }

    printf("Sum: %d\\n", sum);
    return 0;
}`
  },
  pointer: {
    name: 'Pointers',
    code: `#include <stdio.h>

int main() {
    int value = 42;
    int *ptr = &value;

    printf("Value: %d\\n", value);
    printf("Address: %p\\n", (void*)ptr);
    printf("Dereferenced: %d\\n", *ptr);

    return 0;
}`
  },
  recursion: {
    name: 'Recursion',
    code: `#include <stdio.h>

int factorial(int n) {
    if (n <= 1) {
        return 1;
    }
    return n * factorial(n - 1);
}

int main() {
    printf("5! = %d\\n", factorial(5));
    return 0;
}`
  },
  structs: {
    name: 'Structs',
    code: `#include <stdio.h>

struct Point {
    int x;
    int y;
};

int main() {
    struct Point p = {3, 4};
    int sum = p.x + p.y;
    printf("Sum: %d\\n", sum);
    return 0;
}`
  },
  bitops: {
    name: 'Bit Operations',
    code: `#include <stdio.h>

int main() {
    int a = 12;   /* 1100 */
    int b = 10;   /* 1010 */
    printf("AND: %d\\n", a & b);
    printf("OR:  %d\\n", a | b);
    printf("XOR: %d\\n", a ^ b);
    printf("SHL: %d\\n", a << 1);
    return 0;
}`
  }
};

const STAGE_KEYS: StageType[] = [
  'tokens', 'ast', 'symbols', 'ir-unopt', 'ir-opt', 'assembly', 'binary', 'decoded', 'diff',
];

const isStageType = (value: string): value is StageType =>
  (STAGE_KEYS as string[]).includes(value);

type Theme = 'dark' | 'light';
type ViewMode = 'stages' | 'output';

export const App: Component = () => {
  const [code, setCode] = createSignal('');
  const [compilationStages, setCompilationStages] = createSignal<CompilationStages | null>(null);
  const [isCompiling, setIsCompiling] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [showTemplates, setShowTemplates] = createSignal(false);
  const [showHelp, setShowHelp] = createSignal(false);
  const [saveStatus, setSaveStatus] = createSignal<string | null>(null);
  const [shareStatus, setShareStatus] = createSignal<string | null>(null);
  const [theme, setTheme] = createSignal<Theme>('dark');
  const [view, setView] = createSignal<ViewMode>('stages');
  const [activeStage, setActiveStage] = createSignal<StageType>('tokens');

  // Restore theme + shared/saved state on mount (permalink wins over localStorage)
  onMount(async () => {
    const savedTheme = localStorage.getItem('pipeline_theme');
    if (savedTheme === 'light' || savedTheme === 'dark') {
      setTheme(savedTheme);
    }

    const shared = await readShareStateFromUrl();
    if (shared) {
      setCode(shared.code);
      if (isStageType(shared.stage)) {
        setActiveStage(shared.stage);
      }
      return;
    }

    const saved = localStorage.getItem('pipeline_code');
    setCode(saved ?? TEMPLATES.hello.code);
  });

  // Auto-save code to localStorage
  createEffect(() => {
    const currentCode = code();
    if (currentCode) {
      localStorage.setItem('pipeline_code', currentCode);
    }
  });

  // Persist + apply theme
  createEffect(() => {
    const current = theme();
    document.documentElement.dataset.theme = current;
    localStorage.setItem('pipeline_theme', current);
  });

  const handleCodeChange = (newCode: string) => {
    setCode(newCode);
    setError(null);
  };

  const handleTemplateSelect = (templateKey: keyof typeof TEMPLATES) => {
    setCode(TEMPLATES[templateKey].code);
    setShowTemplates(false);
    setError(null);
    setCompilationStages(null);
  };

  const handleSave = () => {
    try {
      const data = {
        code: code(),
        timestamp: new Date().toISOString()
      };
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = url;
      link.download = `pipeline-code-${Date.now()}.json`;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      URL.revokeObjectURL(url);
      setSaveStatus('Saved successfully');
      setTimeout(() => setSaveStatus(null), 3000);
    } catch {
      setError('Failed to save file');
    }
  };

  const handleLoad = () => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = '.json';
    input.onchange = async (e) => {
      const file = (e.target as HTMLInputElement).files?.[0];
      if (file) {
        try {
          const text = await file.text();
          const data = JSON.parse(text);
          if (data.code) {
            setCode(data.code);
            setSaveStatus('Loaded successfully');
            setTimeout(() => setSaveStatus(null), 3000);
          } else {
            setError('Invalid file format');
          }
        } catch {
          setError('Failed to load file');
        }
      }
    };
    input.click();
  };

  const handleCompile = async () => {
    if (code().trim().length === 0) {
      setError('Please enter some C code');
      return;
    }

    if (exceedsLineLimit(code())) {
      setError(LINE_LIMIT_MESSAGE);
      return;
    }

    if (exceedsByteLimit(code())) {
      setError(BYTE_LIMIT_MESSAGE);
      return;
    }

    setIsCompiling(true);
    setError(null);

    try {
      const response = await fetch(`${API_BASE_URL}/compile`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({
          source_code: code(),
          compiler: 'clang',
          flags: [],
          timeout_secs: 10,
        }),
      });

      const data: CompileResponse = await response.json();

      if (data.success && data.stages) {
        setCompilationStages(data.stages);
        setView('stages');
      } else if (data.error) {
        setError(data.error);
      } else {
        setError('Compilation produced no stages');
      }
    } catch {
      setError('Failed to compile. Check if backend is running.');
    } finally {
      setIsCompiling(false);
    }
  };

  const handleShare = async () => {
    try {
      const url = await buildShareUrl({ code: code(), stage: activeStage() });
      const copied = await copyText(url);
      setShareStatus(copied ? 'Link copied to clipboard' : 'Link ready in the address bar');
      if (!copied) {
        window.location.hash = url.split('#')[1] ?? '';
      }
      setTimeout(() => setShareStatus(null), 3000);
    } catch {
      setError('Failed to build share link');
    }
  };

  const handleKeyDown = (event: KeyboardEvent) => {
    const target = event.target as HTMLElement | null;
    const typing = !!target && (
      target.tagName === 'INPUT'
      || target.tagName === 'TEXTAREA'
      || target.isContentEditable
    );
    const mod = event.ctrlKey || event.metaKey;

    if (mod && event.key === 'Enter') {
      event.preventDefault();
      void handleCompile();
    } else if (mod && (event.key === 's' || event.key === 'S')) {
      event.preventDefault();
      void handleShare();
    } else if (event.key === '?' && !typing) {
      event.preventDefault();
      setShowHelp(v => !v);
    } else if (event.key === 'Escape') {
      setShowTemplates(false);
      setShowHelp(false);
    }
  };

  onMount(() => {
    document.addEventListener('keydown', handleKeyDown);
    onCleanup(() => document.removeEventListener('keydown', handleKeyDown));
  });

  return (
    <div class="app">
      <header class="app-header">
        <div class="header-left">
          <h1>Pipeline</h1>
          <p class="subtitle">C Compiler Visualization Tool</p>
        </div>
        <div class="header-right">
          <Show when={saveStatus()}>
            <span class="status-toast">{saveStatus()}</span>
          </Show>
          <Show when={shareStatus()}>
            <span class="status-toast">{shareStatus()}</span>
          </Show>
          <button
            type="button"
            class="header-btn"
            onClick={() => setShowTemplates(true)}
            aria-label="Open examples gallery"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
              <path d="M2 3.5A.5.5 0 012.5 3h11a.5.5 0 010 1h-11a.5.5 0 01-.5-.5zM2 7.5A.5.5 0 012.5 7h11a.5.5 0 010 1h-11A.5.5 0 012 7.5zM2 11.5a.5.5 0 01.5-.5h11a.5.5 0 010 1h-11a.5.5 0 01-.5-.5z"/>
            </svg>
            Examples
          </button>
          <button type="button" class="header-btn" onClick={handleSave} aria-label="Save code to file">
            <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
              <path d="M.5 9.9a.5.5 0 01.5.5v2.5a1 1 0 001 1h12a1 1 0 001-1v-2.5a.5.5 0 011 0v2.5a2 2 0 01-2 2H2a2 2 0 01-2-2v-2.5a.5.5 0 01.5-.5z"/>
              <path d="M7.646 11.854a.5.5 0 00.708 0l3-3a.5.5 0 00-.708-.708L8.5 10.293V1.5a.5.5 0 00-1 0v8.793L5.354 8.146a.5.5 0 10-.708.708l3 3z"/>
            </svg>
            Save
          </button>
          <button type="button" class="header-btn" onClick={handleLoad} aria-label="Load code from file">
            <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
              <path d="M.5 9.9a.5.5 0 01.5.5v2.5a1 1 0 001 1h12a1 1 0 001-1v-2.5a.5.5 0 011 0v2.5a2 2 0 01-2 2H2a2 2 0 01-2-2v-2.5a.5.5 0 01.5-.5z"/>
              <path d="M7.646 1.146a.5.5 0 01.708 0l3 3a.5.5 0 01-.708.708L8.5 2.707V11.5a.5.5 0 01-1 0V2.707L5.354 4.854a.5.5 0 11-.708-.708l3-3z"/>
            </svg>
            Load
          </button>
          <button type="button" class="header-btn" onClick={handleShare} aria-label="Copy share link">
            <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
              <path d="M4.715 6.542L3.343 7.914a3 3 0 104.243 4.243l1.828-1.829A3 3 0 008.586 5.5l-1.828 1.829a.5.5 0 00.708.708l1.828-1.829a2 2 0 012.828 2.829l-1.828 1.828a2 2 0 01-2.828 0l-.708.708a3 3 0 004.243 0l1.828-1.828a3 3 0 10-4.243-4.243l-1.829 1.829a.5.5 0 10.708.708z"/>
              <path d="M6.5 9.5l3-3a.5.5 0 00-.708-.708l-3 3a.5.5 0 00.708.708z"/>
            </svg>
            Share
          </button>
          <button
            type="button"
            class="header-btn icon-btn"
            onClick={() => setTheme(theme() === 'dark' ? 'light' : 'dark')}
            aria-label={theme() === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}
            title="Toggle theme"
          >
            <Show when={theme() === 'dark'}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                <path d="M8 12a4 4 0 100-8 4 4 0 000 8zM8 0a.5.5 0 01.5.5v1a.5.5 0 01-1 0v-1A.5.5 0 018 0zm0 13a.5.5 0 01.5.5v1a.5.5 0 01-1 0v-1A.5.5 0 018 13zM16 8a.5.5 0 01-.5.5h-1a.5.5 0 010-1h1A.5.5 0 0116 8zM3 8a.5.5 0 01-.5.5h-1a.5.5 0 010-1h1A.5.5 0 013 8z"/>
              </svg>
            </Show>
            <Show when={theme() === 'light'}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                <path d="M6 .278a.768.768 0 01.08.858 7.208 7.208 0 00-.878 3.46c0 4.021 3.278 7.277 7.318 7.277.527 0 1.04-.055 1.533-.16a.787.787 0 01.81.316.733.733 0 01-.031.893A8.349 8.349 0 018.344 16C3.734 16 0 12.286 0 7.71 0 4.266 2.114 1.312 5.124.06A.752.752 0 016 .278z"/>
              </svg>
            </Show>
          </button>
          <button
            type="button"
            class="header-btn icon-btn"
            onClick={() => setShowHelp(true)}
            aria-label="Show keyboard shortcuts help"
            title="Keyboard shortcuts"
          >
            ?
          </button>
        </div>
      </header>

      <Show when={showTemplates()}>
        <div
          class="templates-overlay"
          onClick={() => setShowTemplates(false)}
          role="dialog"
          aria-modal="true"
          aria-label="Examples gallery"
        >
          <div class="templates-modal" onClick={(e) => e.stopPropagation()}>
            <div class="modal-head">
              <h2 class="modal-title">Examples</h2>
              <button
                type="button"
                class="modal-close"
                onClick={() => setShowTemplates(false)}
                aria-label="Close examples gallery"
              >
                ×
              </button>
            </div>
            <div class="template-grid">
              <For each={Object.entries(TEMPLATES)}>
                {([key, template]) => (
                  <button
                    type="button"
                    class="template-card"
                    onClick={() => handleTemplateSelect(key as keyof typeof TEMPLATES)}
                  >
                    <h3 class="template-name">{template.name}</h3>
                    <pre class="template-preview">{template.code.slice(0, 80)}...</pre>
                  </button>
                )}
              </For>
            </div>
          </div>
        </div>
      </Show>

      <Show when={showHelp()}>
        <div
          class="templates-overlay"
          onClick={() => setShowHelp(false)}
          role="dialog"
          aria-modal="true"
          aria-label="Keyboard shortcuts"
        >
          <div class="templates-modal help-modal" onClick={(e) => e.stopPropagation()}>
            <div class="modal-head">
              <h2 class="modal-title">Keyboard Shortcuts</h2>
              <button
                type="button"
                class="modal-close"
                onClick={() => setShowHelp(false)}
                aria-label="Close help"
              >
                ×
              </button>
            </div>
            <ul class="shortcut-list">
              <li><kbd>Ctrl</kbd>/<kbd>Cmd</kbd> + <kbd>Enter</kbd><span>Compile the current code</span></li>
              <li><kbd>Ctrl</kbd>/<kbd>Cmd</kbd> + <kbd>S</kbd><span>Copy a share link</span></li>
              <li><kbd>?</kbd><span>Toggle this help</span></li>
              <li><kbd>Esc</kbd><span>Close dialogs</span></li>
            </ul>
          </div>
        </div>
      </Show>

      <main class="app-main">
        <section class="editor-section">
          <SourceEditor
            initialValue={code()}
            onCodeChange={handleCodeChange}
            onCompile={handleCompile}
            isCompiling={isCompiling()}
          />
        </section>

        <section class="visualization-section">
          <div class="visualization-header">
            <h2>Compilation Pipeline</h2>
            <div class="view-mode-toggle" role="group" aria-label="View mode">
              <button
                type="button"
                class={`mode-button ${view() === 'stages' ? 'active' : ''}`}
                onClick={() => setView('stages')}
                aria-pressed={view() === 'stages'}
              >
                Pipeline stages
              </button>
              <button
                type="button"
                class={`mode-button ${view() === 'output' ? 'active' : ''}`}
                onClick={() => setView('output')}
                aria-pressed={view() === 'output'}
              >
                Output
              </button>
            </div>
          </div>

          <Show when={error()}>
            <div class="error-banner" role="alert">
              <div class="banner-row">
                <svg width="20" height="20" viewBox="0 0 16 16" fill="currentColor" class="banner-icon" aria-hidden="true">
                  <path d="M8 15A7 7 0 118 1a7 7 0 010 14zm0 1A8 8 0 108 0a8 8 0 000 16z"/>
                  <path d="M7.002 11a1 1 0 112 0 1 1 0 01-2 0zM7.1 4.995a.905.905 0 111.8 0l-.35 3.507a.552.552 0 01-1.1 0L7.1 4.995z"/>
                </svg>
                <div class="banner-body">
                  <div class="banner-title">Compilation Error</div>
                  <div class="error-message">{error()}</div>
                </div>
              </div>
            </div>
          </Show>

          <Show when={compilationStages() && !error()}>
            <div class="success-banner" role="status">
              <svg width="20" height="20" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                <path d="M16 8A8 8 0 110 8a8 8 0 0116 0zm-3.97-3.03a.75.75 0 00-1.08.022L7.477 9.417 5.384 7.323a.75.75 0 00-1.06 1.06L6.97 11.03a.75.75 0 001.079-.02l3.992-4.99a.75.75 0 00-.01-1.05z"/>
              </svg>
              <span>Compilation successful. Explore the stages below.</span>
            </div>
          </Show>

          <Show when={view() === 'stages'}>
            <EducationalViewer
              stages={compilationStages()}
              activeStage={activeStage()}
              onStageChange={setActiveStage}
            />
          </Show>

          <Show when={view() === 'output'}>
            <OutputView sourceCode={code()} />
          </Show>
        </section>
      </main>
    </div>
  );
};
