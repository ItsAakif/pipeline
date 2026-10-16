import { Component, Show, createSignal, createEffect, onCleanup } from 'solid-js';
import type { RunResponse } from '../types';
import { API_BASE_URL } from '../lib/api';
import {
  exceedsLineLimit,
  exceedsByteLimit,
  LINE_LIMIT_MESSAGE,
  BYTE_LIMIT_MESSAGE,
} from '../lib/lines';

// Coalesce rapid edits while the Output tab is open so we don't stampede the
// backend on every keystroke.
const RUN_DEBOUNCE_MS = 500;

// Used only if a 429 response carries neither a Retry-After header nor a
// retry_after_secs body field (mirrors the backend RUN_COOLDOWN_SECS default).
const FALLBACK_COOLDOWN_SECS = 180;

export const OutputView: Component<{ sourceCode: string }> = (props) => {
  const [result, setResult] = createSignal<RunResponse | null>(null);
  const [isRunning, setIsRunning] = createSignal(false);
  const [requestError, setRequestError] = createSignal<string | null>(null);
  const [cooldownSecs, setCooldownSecs] = createSignal(0);

  let lastRunSource: string | null = null;
  let debounceTimer: number | undefined;
  let cooldownTimer: number | undefined;

  const coolingDown = () => cooldownSecs() > 0;

  const stopCooldown = () => {
    if (cooldownTimer !== undefined) {
      window.clearInterval(cooldownTimer);
      cooldownTimer = undefined;
    }
    setCooldownSecs(0);
  };

  const startCooldown = (secs: number) => {
    if (cooldownTimer !== undefined) {
      window.clearInterval(cooldownTimer);
    }
    setCooldownSecs(secs);
    cooldownTimer = window.setInterval(() => {
      const next = cooldownSecs() - 1;
      if (next <= 0) {
        stopCooldown();
      } else {
        setCooldownSecs(next);
      }
    }, 1000);
  };

  const run = async (source: string) => {
    // Disabled while the per-IP cooldown is active.
    if (coolingDown()) return;

    if (source.trim().length === 0) {
      setResult(null);
      setRequestError(null);
      return;
    }
    if (exceedsLineLimit(source)) {
      setResult(null);
      setRequestError(LINE_LIMIT_MESSAGE);
      return;
    }
    if (exceedsByteLimit(source)) {
      setResult(null);
      setRequestError(BYTE_LIMIT_MESSAGE);
      return;
    }

    setIsRunning(true);
    setRequestError(null);

    try {
      const response = await fetch(`${API_BASE_URL}/run`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ source_code: source }),
      });

      // 429: per-IP cooldown. Show a countdown, not a generic error.
      if (response.status === 429) {
        let secs = Number.parseInt(response.headers.get('Retry-After') ?? '', 10);
        if (!Number.isFinite(secs) || secs <= 0) {
          try {
            const body = (await response.json()) as RunResponse;
            if (typeof body.retry_after_secs === 'number') {
              secs = body.retry_after_secs;
            }
          } catch {
            // malformed body; fall through to the default
          }
        }
        if (!Number.isFinite(secs) || secs <= 0) {
          secs = FALLBACK_COOLDOWN_SECS;
        }
        setResult(null);
        setRequestError(null);
        startCooldown(Math.ceil(secs));
        return;
      }

      const data: RunResponse = await response.json();
      setResult(data);
    } catch {
      setResult(null);
      setRequestError('Failed to run. Check if backend is running.');
    } finally {
      setIsRunning(false);
    }
  };

  // Run when the view opens (mount) and re-run when the code changes while the
  // view is open. This component is only mounted for the Output tab, so nothing
  // runs while the tab is closed, and `run` no-ops while cooling down.
  createEffect(() => {
    const source = props.sourceCode;
    if (lastRunSource === source) return;
    lastRunSource = source;
    if (debounceTimer) window.clearTimeout(debounceTimer);
    debounceTimer = window.setTimeout(() => void run(source), RUN_DEBOUNCE_MS);
  });

  onCleanup(() => {
    if (debounceTimer) window.clearTimeout(debounceTimer);
    if (cooldownTimer !== undefined) window.clearInterval(cooldownTimer);
  });

  return (
    <div class="output-view">
      <div class="output-head">
        <h3>Program Output</h3>
        <button
          type="button"
          class="artifact-button"
          onClick={() => void run(props.sourceCode)}
          disabled={isRunning() || coolingDown()}
        >
          {coolingDown()
            ? `Wait ${cooldownSecs()}s`
            : isRunning()
              ? 'Running...'
              : 'Run again'}
        </button>
      </div>

      <Show when={coolingDown()}>
        <div class="output-cooldown">
          Cooling down — try again in {cooldownSecs()}s
        </div>
      </Show>

      <Show when={isRunning()}>
        <div class="output-status" role="status">Running your program...</div>
      </Show>

      <Show when={requestError() && !coolingDown()}>
        <div class="error-banner" role="alert">
          <div class="banner-row">
            <svg width="20" height="20" viewBox="0 0 16 16" fill="currentColor" class="banner-icon" aria-hidden="true">
              <path d="M8 15A7 7 0 118 1a7 7 0 010 14zm0 1A8 8 0 108 0a8 8 0 000 16z"/>
              <path d="M7.002 11a1 1 0 112 0 1 1 0 01-2 0zM7.1 4.995a.905.905 0 111.8 0l-.35 3.507a.552.552 0 01-1.1 0L7.1 4.995z"/>
            </svg>
            <div class="banner-body">
              <div class="banner-title">Run Error</div>
              <div class="error-message">{requestError()}</div>
            </div>
          </div>
        </div>
      </Show>

      <Show when={!isRunning() && !requestError() && !coolingDown() && result()}>
        {(data) => (
          <>
            <Show when={data().compile_error}>
              <div class="error-banner" role="alert">
                <div class="banner-row">
                  <svg width="20" height="20" viewBox="0 0 16 16" fill="currentColor" class="banner-icon" aria-hidden="true">
                    <path d="M8 15A7 7 0 118 1a7 7 0 010 14zm0 1A8 8 0 108 0a8 8 0 000 16z"/>
                    <path d="M7.002 11a1 1 0 112 0 1 1 0 01-2 0zM7.1 4.995a.905.905 0 111.8 0l-.35 3.507a.552.552 0 01-1.1 0L7.1 4.995z"/>
                  </svg>
                  <div class="banner-body">
                    <div class="banner-title">Compile Error</div>
                    <div class="error-message">{data().compile_error}</div>
                  </div>
                </div>
              </div>
            </Show>

            <Show when={!data().compile_error}>
              <div class="output-meta">
                <span class="output-badge">Exit code: {data().exit_code}</span>
                <Show when={data().timed_out}>
                  <span class="output-badge flag">Timed out</span>
                </Show>
                <Show when={data().truncated}>
                  <span class="output-badge flag">Output truncated</span>
                </Show>
                <span class="output-time">{data().execution_time_ms} ms</span>
              </div>

              <section class="output-section">
                <h4>stdout</h4>
                <Show
                  when={data().stdout.length > 0}
                  fallback={<p class="output-empty">No output</p>}
                >
                  <pre class="code-block output-pre">{data().stdout}</pre>
                </Show>
              </section>

              <Show when={data().stderr.length > 0}>
                <section class="output-section">
                  <h4>stderr</h4>
                  <pre class="code-block output-pre output-stderr">{data().stderr}</pre>
                </section>
              </Show>
            </Show>
          </>
        )}
      </Show>

      <Show when={!isRunning() && !requestError() && !coolingDown() && !result()}>
        <div class="output-empty-state">
          <p>Run your code to see its output here.</p>
        </div>
      </Show>
    </div>
  );
};
