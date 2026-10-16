import { Component, Show, For } from 'solid-js';
import type { JSX } from 'solid-js';
import type { AnnotatedInstruction, ASTNode, CompilationStages } from '../types';
import { downloadText } from '../lib/download';
import { describeOpcode } from '../lib/opcodes';
import { MonacoDiff } from './MonacoDiff';

export type StageType =
  | 'tokens'
  | 'ast'
  | 'symbols'
  | 'ir-unopt'
  | 'ir-opt'
  | 'assembly'
  | 'binary'
  | 'decoded'
  | 'diff';

interface EducationalViewerProps {
  stages: CompilationStages | null | undefined;
  activeStage: StageType;
  onStageChange: (stage: StageType) => void;
}

const STAGE_LABELS: Record<StageType, string> = {
  'tokens': '1. Tokenization',
  'ast': '2. AST',
  'symbols': '3. Symbols',
  'ir-unopt': '4. IR (Unoptimized)',
  'ir-opt': '5. IR (Optimized)',
  'assembly': '6. Assembly',
  'binary': '7. Binary',
  'decoded': '8. Decoded',
  'diff': '9. Diff (O0 vs O2)',
};

const STAGE_KEYS = Object.keys(STAGE_LABELS) as StageType[];

// Node text (names, literal values, operators) is embedded in generated C block
// comments. Neutralize anything that would terminate or break the comment:
// `*/` must not close it early, and newlines must not split the line.
const sanitizeComment = (text: string): string => {
  const cleaned = text
    .replace(/\*\//g, '* /') // never terminate the C comment early
    .replace(/\s+/g, ' ')    // collapse newlines/tabs/runs onto one line
    .trim();
  return cleaned.length > 80 ? `${cleaned.slice(0, 77)}...` : cleaned;
};

// Describe a single AST node for the generated snippet's teaching comment.
const describeASTNode = (node: ASTNode): string => {
  switch (node.node_type) {
    case 'FunctionDecl':
      return `function: ${node.name || 'anonymous'}`;
    case 'VarDecl':
    case 'ParmVarDecl':
      return `variable: ${node.name || 'unknown'}`;
    case 'DeclRefExpr':
      return `reference to: ${node.name || 'unknown'}`;
    case 'BinaryOperator':
    case 'UnaryOperator':
      return `operator: ${node.opcode || '?'}`;
    default:
      break;
  }
  if (node.node_type.includes('Literal')) {
    return `literal value: ${node.value ?? 'N/A'}`;
  }
  if (node.name) return `${node.node_type}: ${node.name}`;
  if (node.value) return `${node.node_type}: ${node.value}`;
  if (node.opcode) return `${node.node_type}: ${node.opcode}`;
  return `${node.node_type} node`;
};

// Generate a valid-C if / else-if dispatch over the distinct node types present
// in the tree, one branch per type. The branch body is the teaching comment
// only; children are visited by the shared recursive loop below, so there are
// no undefined macros or fields.
const generateTraversalCode = (root: ASTNode | null): string => {
  if (!root) return '';

  const seen = new Map<string, ASTNode>();
  const collect = (node: ASTNode | null) => {
    if (!node || seen.has(node.node_type)) return;
    seen.set(node.node_type, node);
    node.children.forEach(collect);
  };
  collect(root);

  const branches = Array.from(seen.entries()).map(([type, node], index) => {
    const keyword = index === 0 ? 'if' : '} else if';
    return `    ${keyword} (strcmp(node->node_type, "${type}") == 0) {\n        /* ${sanitizeComment(describeASTNode(node))} */`;
  });

  return branches.length > 0 ? `${branches.join('\n')}\n    }` : '    /* no nodes */';
};

// Helper function to render AST nodes as a visual tree (optimized with memoization)
const renderASTNode = (node: ASTNode | null, depth: number): JSX.Element | null => {
  if (!node) return null;

  const hasChildren = node.children && node.children.length > 0;

  // Skip TypedefDecl nodes from system headers
  if (node.node_type === 'TypedefDecl' && (!node.name || node.name.startsWith('__'))) {
    return null;
  }

  // Performance: Limit depth for very deep trees
  if (depth > 15) {
    return (
      <div class="ast-depth-limit">
        Tree depth limit reached. {hasChildren ? `${node.children.length} more nodes...` : ''}
      </div>
    );
  }

  // Get display info
  const getNodeLabel = () => {
    if (node.opcode) return node.opcode;
    if (node.value) return node.value;
    if (node.name) return node.name;
    return '';
  };

  const label = getNodeLabel();

  return (
    <div class="ast-node-container">
      {/* Node box */}
      <div style="display: flex; flex-direction: column; align-items: center;">
        <div class="ast-node-box">
          <div class="ast-node-type">
            {node.node_type}
          </div>
          <Show when={label}>
            <div class="ast-node-label">
              {label}
            </div>
          </Show>
        </div>

        {/* Connector line down */}
        <Show when={hasChildren}>
          <div class="ast-connector-v"></div>
        </Show>
      </div>

      {/* Children in horizontal layout */}
      <Show when={hasChildren}>
        <div style="display: flex; gap: 2rem; position: relative;">
          {/* Horizontal connecting line */}
          <Show when={node.children.length > 1}>
            <div class="ast-connector-h"></div>
          </Show>

          {/* Child nodes - Performance: Only render first 50 children for very large nodes */}
          <For each={node.children.slice(0, 50)}>
            {(child) => (
              <div style="display: flex; flex-direction: column; align-items: center;">
                {/* Vertical line up to horizontal line */}
                <div class="ast-connector-v"></div>
                {renderASTNode(child, depth + 1)}
              </div>
            )}
          </For>
          <Show when={node.children.length > 50}>
            <div class="ast-more-nodes">
              ... and {node.children.length - 50} more children
            </div>
          </Show>
        </div>
      </Show>
    </div>
  );
};

const ArtifactButton: Component<{
  label: string;
  filename: string;
  content: () => string;
  mime?: string;
}> = (props) => (
  <button
    type="button"
    class="artifact-button"
    onClick={() => downloadText(props.filename, props.content(), props.mime)}
  >
    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      <path d="M8.5 1a.5.5 0 0 0-1 0v8.793L5.354 7.646a.5.5 0 1 0-.708.708l3 3a.5.5 0 0 0 .708 0l3-3a.5.5 0 0 0-.708-.708L8.5 9.793V1z"/>
      <path d="M3 13.5a.5.5 0 0 1 .5-.5h9a.5.5 0 0 1 0 1h-9a.5.5 0 0 1-.5-.5z"/>
    </svg>
    {props.label}
  </button>
);

export const EducationalViewer: Component<EducationalViewerProps> = (props) => {
  const stages = () => props.stages ?? null;

  // Function to download AST diagram as image
  const downloadASTImage = async () => {
    const container = document.getElementById('ast-diagram-container');
    if (!container) return;

    try {
      // Store original styles
      const originalOverflowX = container.style.overflowX;
      const originalOverflowY = container.style.overflowY;
      const originalMaxHeight = container.style.maxHeight;
      const originalHeight = container.style.height;
      const originalWidth = container.style.width;
      const originalMaxWidth = container.style.maxWidth;

      // Temporarily remove ALL scroll/size constraints to capture full content
      container.style.overflowX = 'visible';
      container.style.overflowY = 'visible';
      container.style.maxHeight = 'none';
      container.style.height = 'auto';
      container.style.width = 'auto';
      container.style.maxWidth = 'none';

      // Wait a moment for layout to recalculate
      await new Promise(resolve => setTimeout(resolve, 100));

      // Use html2canvas library
      const html2canvas = (await import('html2canvas')).default;

      const canvas = await html2canvas(container, {
        backgroundColor: '#f5f7fa',
        scale: 2, // Higher quality
        logging: false,
        useCORS: true,
        allowTaint: true,
        width: container.scrollWidth,
        height: container.scrollHeight,
        windowWidth: container.scrollWidth,
        windowHeight: container.scrollHeight
      });

      // Restore original styles
      container.style.overflowX = originalOverflowX;
      container.style.overflowY = originalOverflowY;
      container.style.maxHeight = originalMaxHeight;
      container.style.height = originalHeight;
      container.style.width = originalWidth;
      container.style.maxWidth = originalMaxWidth;

      // Convert to blob and download
      canvas.toBlob((blob) => {
        if (blob) {
          const url = URL.createObjectURL(blob);
          const link = document.createElement('a');
          link.href = url;
          link.download = `ast-diagram-${Date.now()}.png`;
          document.body.appendChild(link);
          link.click();
          document.body.removeChild(link);
          URL.revokeObjectURL(url);
        }
      });
    } catch (error) {
      console.error('Error generating AST image:', error);
      alert('Failed to generate image. Please try again.');
    }
  };

  const handleTabKeyDown = (event: KeyboardEvent, index: number) => {
    let nextIndex = index;
    if (event.key === 'ArrowRight') nextIndex = (index + 1) % STAGE_KEYS.length;
    else if (event.key === 'ArrowLeft') nextIndex = (index - 1 + STAGE_KEYS.length) % STAGE_KEYS.length;
    else if (event.key === 'Home') nextIndex = 0;
    else if (event.key === 'End') nextIndex = STAGE_KEYS.length - 1;
    else return;

    event.preventDefault();
    const nextKey = STAGE_KEYS[nextIndex];
    props.onStageChange(nextKey);
    queueMicrotask(() => document.getElementById(`stage-tab-${nextKey}`)?.focus());
  };

  const assemblyText = () =>
    stages()?.assembly_annotated
      ?.map((instr) => instr.full_instruction || `${instr.mnemonic}${instr.operands ? ' ' + instr.operands : ''}`)
      .join('\n') ?? '';

  const mnemonicOf = (instr: AnnotatedInstruction): string => {
    if (instr.mnemonic) return instr.mnemonic;
    const full = instr.full_instruction;
    if (typeof full === 'string') return full.trim().split(/\s+/)[0] ?? '';
    return '';
  };

  return (
    <div class="educational-viewer">
      <div class="stage-tabs" role="tablist" aria-label="Compilation stages">
        <For each={STAGE_KEYS}>
          {(key, index) => (
            <button
              type="button"
              role="tab"
              id={`stage-tab-${key}`}
              aria-selected={props.activeStage === key}
              aria-controls="stage-panel"
              tabindex={props.activeStage === key ? 0 : -1}
              class={`stage-tab ${props.activeStage === key ? 'active' : ''}`}
              onClick={() => props.onStageChange(key)}
              onKeyDown={(e) => handleTabKeyDown(e, index())}
            >
              {STAGE_LABELS[key]}
            </button>
          )}
        </For>
      </div>

      <div
        class="stage-content"
        role="tabpanel"
        id="stage-panel"
        aria-labelledby={`stage-tab-${props.activeStage}`}
        tabindex={0}
      >
        <Show when={props.stages} fallback={
          <div class="empty-state">
            <p>Compile your code to see compilation stages</p>
            <p class="hint">Use the "Compile" button above to start</p>
          </div>
        }>
          <Show when={props.activeStage === 'tokens'}>
            <div class="stage-view tokens-view">
              <div class="stage-head">
                <h3>Lexical Analysis (Tokenization)</h3>
                <ArtifactButton
                  label="Download tokens (JSON)"
                  filename="tokens.json"
                  mime="application/json"
                  content={() => JSON.stringify(stages()?.tokens ?? [], null, 2)}
                />
              </div>
              <p class="stage-description">
                The compiler breaks your source code into tokens - the smallest meaningful units like keywords, identifiers, operators, and literals.
              </p>
              <Show when={props.stages?.tokens && props.stages.tokens.length > 0}>
                <div class="token-list">
                  <For each={props.stages!.tokens}>
                    {(token) => (
                      <div class="token-item">
                        <div class="token-header">
                          <span class="token-type">{token.token_type}</span>
                          <span class="token-location">Line {token.line}:{token.column}</span>
                        </div>
                        <div class="token-value">{token.value}</div>
                        <div class="token-explanation">{token.explanation}</div>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={!props.stages?.tokens || props.stages.tokens.length === 0}>
                <p class="not-available">Token generation not implemented in backend yet. This stage would normally show keywords (int, return), identifiers (main, x, y), operators (=, +), and literals (10, 20) from your code.</p>
              </Show>
            </div>
          </Show>

          <Show when={props.activeStage === 'ast'}>
            <div class="stage-view ast-view">
              <div class="stage-head">
                <div>
                  <h3>Abstract Syntax Tree</h3>
                  <p class="stage-description">
                    The AST represents the grammatical structure of your code as a tree. Each node represents a construct in your source code.
                  </p>
                </div>
                <div class="stage-actions">
                  <Show when={props.stages?.ast}>
                    <ArtifactButton
                      label="Download AST (JSON)"
                      filename="ast.json"
                      mime="application/json"
                      content={() => JSON.stringify(stages()?.ast ?? null, null, 2)}
                    />
                    <button
                      type="button"
                      class="artifact-button primary"
                      onClick={() => downloadASTImage()}
                    >
                      <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                        <path d="M8.5 1a.5.5 0 0 0-1 0v8.793L5.354 7.646a.5.5 0 1 0-.708.708l3 3a.5.5 0 0 0 .708 0l3-3a.5.5 0 0 0-.708-.708L8.5 9.793V1z"/>
                        <path d="M3 13.5a.5.5 0 0 1 .5-.5h9a.5.5 0 0 1 0 1h-9a.5.5 0 0 1-.5-.5z"/>
                      </svg>
                      Download diagram (PNG)
                    </button>
                  </Show>
                </div>
              </div>
              <Show when={props.stages?.ast}>
                <div id="ast-diagram-container" class="ast-canvas">
                  {renderASTNode(props.stages!.ast!.root, 0)}
                </div>

                <div class="ast-traversal">
                  <h4>Tree Traversal Code</h4>
                  <p class="stage-description">
                    Here's how you would traverse and process this AST programmatically:
                  </p>
                  <pre class="code-block">
{`/* Recursively traverse and process AST nodes. */
#include <stddef.h>
#include <string.h>

typedef struct ASTNode {
    const char *node_type;   /* e.g. "FunctionDecl" */
    const char *name;        /* may be NULL */
    struct ASTNode **children;
    size_t child_count;
} ASTNode;

void process_ast_node(const ASTNode *node) {
    if (node == NULL) return;

`}
{generateTraversalCode(props.stages!.ast!.root)}
{`

    /* Recurse into every child. */
    for (size_t i = 0; i < node->child_count; i++) {
        process_ast_node(node->children[i]);
    }
}`}</pre>
                </div>
              </Show>
              <Show when={!props.stages?.ast}>
                <p class="not-available">AST parsing in progress...</p>
              </Show>
            </div>
          </Show>

          <Show when={props.activeStage === 'symbols'}>
            <div class="stage-view symbols-view">
              <div class="stage-head">
                <h3>Semantic Analysis (Symbol Table)</h3>
                <ArtifactButton
                  label="Download symbols (JSON)"
                  filename="symbols.json"
                  mime="application/json"
                  content={() => JSON.stringify(stages()?.symbols ?? {}, null, 2)}
                />
              </div>
              <p class="stage-description">
                The compiler analyzes the meaning of your code, tracking all variables, functions, and types in symbol tables.
              </p>
              <Show when={props.stages?.symbols}>
                <div class="symbol-table-display">
                  <For each={props.stages!.symbols.scopes}>
                    {(scope) => (
                      <div class="symbol-scope">
                        <h4>
                          Scope: {scope.name || 'global'}
                        </h4>
                        <Show when={scope.symbols && scope.symbols.length > 0}>
                          <For each={scope.symbols}>
                            {(symbol) => (
                              <div class="symbol-item">
                                <div class="symbol-line">
                                  <span class="symbol-name">{symbol.name}</span>
                                  <span class="symbol-kind">
                                    {symbol.symbol_type}
                                  </span>
                                  <span class="code-text text-muted">
                                    {symbol.data_type}
                                  </span>
                                  <span class="symbol-location">
                                    Line {symbol.line_declared}
                                  </span>
                                </div>
                                <div class="symbol-explanation">
                                  {symbol.explanation}
                                </div>
                              </div>
                            )}
                          </For>
                        </Show>
                        <Show when={!scope.symbols || scope.symbols.length === 0}>
                          <p class="symbol-empty">No symbols in this scope</p>
                        </Show>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={!props.stages?.symbols}>
                <p class="not-available">No symbol table information available</p>
              </Show>
            </div>
          </Show>

          <Show when={props.activeStage === 'ir-unopt'}>
            <div class="stage-view ir-view">
              <div class="stage-head">
                <h3>LLVM Intermediate Representation (Unoptimized)</h3>
                <ArtifactButton
                  label="Download IR (LLVM)"
                  filename="ir-unoptimized.ll"
                  content={() => stages()?.ir_unoptimized ?? ''}
                />
              </div>
              <p class="stage-description">
                Your code translated to LLVM IR - a low-level representation that's still machine-independent. This is before any optimizations.
              </p>
              <Show when={props.stages?.ir_unoptimized}>
                <div class="ir-display">
                  <For each={props.stages!.ir_unoptimized.split('\n')}>
                    {(line) => {
                      const trimmed = line.trim();
                      let explanation = '';

                      if (trimmed.includes('alloca i32')) {
                        explanation = ' // Allocate space on stack for an integer variable';
                      } else if (trimmed.match(/store i32 \d+/)) {
                        const match = trimmed.match(/store i32 (\d+)/);
                        explanation = match ? ` // Store value ${match[1]} into variable` : '';
                      } else if (trimmed.includes('load i32')) {
                        explanation = ' // Load value from variable';
                      } else if (trimmed.includes('add nsw i32')) {
                        explanation = ' // Add two integers (no signed wrap)';
                      } else if (trimmed.includes('call i32') && trimmed.includes('printf')) {
                        explanation = ' // Call printf function to output result';
                      } else if (trimmed.includes('ret i32')) {
                        explanation = ' // Return from main function';
                      } else if (trimmed.includes('define') && trimmed.includes('@main')) {
                        explanation = ' // Start of main function';
                      }

                      return (
                        <div>
                          <span>{line}</span>
                          <span class="ir-inline-comment">{explanation}</span>
                        </div>
                      );
                    }}
                  </For>
                </div>
              </Show>
              <Show when={!props.stages?.ir_unoptimized}>
                <p class="not-available">No unoptimized LLVM IR available</p>
              </Show>
            </div>
          </Show>

          <Show when={props.activeStage === 'ir-opt'}>
            <div class="stage-view ir-view">
              <div class="stage-head">
                <h3>LLVM IR (Optimized)</h3>
                <ArtifactButton
                  label="Download IR (LLVM)"
                  filename="ir-optimized.ll"
                  content={() => stages()?.ir_optimized ?? ''}
                />
              </div>
              <p class="stage-description">
                After optimization passes, LLVM improves your code for speed and size. Compare with the unoptimized version.
              </p>
              <Show when={props.stages?.optimizations_applied && props.stages.optimizations_applied.length > 0}>
                <div class="optimizations-detected">
                  <h4>Optimizations Applied:</h4>
                  <ul>
                    <For each={props.stages!.optimizations_applied}>
                      {(opt) => <li>{opt}</li>}
                    </For>
                  </ul>
                </div>
              </Show>
              <Show when={props.stages?.ir_optimized}>
                <div class="ir-display">
                  <For each={props.stages!.ir_optimized.split('\n')}>
                    {(line) => {
                      const trimmed = line.trim();
                      let explanation = '';

                      if (trimmed.includes('tail call') && trimmed.includes('printf')) {
                        const match = trimmed.match(/i32 noundef (\d+)/);
                        if (match) {
                          explanation = ` // Optimized: Compiler calculated result (${match[1]}) at compile time!`;
                        } else {
                          explanation = ' // Optimized tail call to printf';
                        }
                      } else if (trimmed.includes('ret i32')) {
                        explanation = ' // Return 0 (success)';
                      } else if (trimmed.includes('define') && trimmed.includes('@main')) {
                        explanation = ' // Main function (optimized)';
                      } else if (trimmed.startsWith('; Function Attrs:')) {
                        explanation = ' // Function optimization attributes';
                      }

                      return (
                        <div>
                          <span>{line}</span>
                          <span class="ir-inline-comment">{explanation}</span>
                        </div>
                      );
                    }}
                  </For>
                </div>
              </Show>
              <Show when={!props.stages?.ir_optimized}>
                <p class="not-available">No optimized LLVM IR available</p>
              </Show>
            </div>
          </Show>

          <Show when={props.activeStage === 'assembly'}>
            <div class="stage-view assembly-view">
              <div class="stage-head">
                <h3>Assembly Code Generation</h3>
                <ArtifactButton
                  label="Download assembly (ASM)"
                  filename="program.s"
                  content={assemblyText}
                />
              </div>
              <p class="stage-description">
                LLVM IR translated to x86-64 assembly - human-readable machine instructions specific to your CPU architecture. Hover an instruction to see what it does.
              </p>
              <Show when={props.stages?.assembly_annotated && props.stages.assembly_annotated.length > 0}>
                <div class="assembly-list">
                  <For each={props.stages!.assembly_annotated}>
                    {(instr) => {
                      const mnemonic = mnemonicOf(instr);
                      const description = describeOpcode(mnemonic);
                      return (
                        <div class="assembly-instruction">
                          <div class="assembly-head">
                            <span class="instr-line">Instruction {instr.line_number}</span>
                            <code class="instr-code" title={description}>
                              {mnemonic}
                              <Show when={instr.operands}>
                                <span class="instr-operands"> {instr.operands}</span>
                              </Show>
                            </code>
                          </div>
                          <div class="instr-explanation">
                            {instr.explanation_simple}
                          </div>
                        </div>
                      );
                    }}
                  </For>
                </div>
              </Show>
              <Show when={!props.stages?.assembly_annotated || props.stages.assembly_annotated.length === 0}>
                <p class="not-available">No assembly instructions generated</p>
              </Show>
            </div>
          </Show>

          <Show when={props.activeStage === 'binary'}>
            <div class="stage-view binary-view">
              <div class="stage-head">
                <h3>Binary Encoding</h3>
                <ArtifactButton
                  label="Download binary (JSON bytes)"
                  filename="binary.json"
                  mime="application/json"
                  content={() => JSON.stringify(stages()?.binary ?? [], null, 2)}
                />
              </div>
              <p class="stage-description">
                Assembly instructions encoded as raw bytes - the actual machine code that executes on your CPU.
              </p>
              <Show when={props.stages?.binary && props.stages.binary.length > 0}>
                <div class="binary-stats">
                  <h4>Binary Statistics</h4>
                  <div class="binary-stats-grid">
                    <div class="binary-stat">
                      <div class="binary-stat-value accent-1">{props.stages!.binary.length}</div>
                      <div class="binary-stat-label">Total Bytes</div>
                    </div>
                    <div class="binary-stat">
                      <div class="binary-stat-value accent-2">{(props.stages!.binary.length / 1024).toFixed(2)} KB</div>
                      <div class="binary-stat-label">Binary Size</div>
                    </div>
                    <div class="binary-stat">
                      <div class="binary-stat-value accent-3">{props.stages!.binary_decoded?.length || 0}</div>
                      <div class="binary-stat-label">Instructions</div>
                    </div>
                  </div>

                  <h4>Hexadecimal View</h4>
                  <div class="code-panel">
                    <For each={Array.from({length: Math.ceil(props.stages!.binary.length / 16)})}>
                      {(_row, rowIndex) => {
                        const offset = rowIndex() * 16;
                        const rowBytes = props.stages!.binary.slice(offset, offset + 16);
                        return (
                          <div class="byte-row">
                            <span class="byte-offset">
                              {offset.toString(16).padStart(8, '0')}:
                            </span>
                            <span class="byte-list">
                              <For each={rowBytes}>
                                {(byte) => (
                                  <span
                                    class="byte-cell"
                                    title={`Byte: 0x${byte.toString(16).padStart(2, '0')} (decimal: ${byte}, binary: ${byte.toString(2).padStart(8, '0')})`}
                                  >
                                    {byte.toString(16).padStart(2, '0')}
                                  </span>
                                )}
                              </For>
                            </span>
                            <span class="byte-ascii">
                              {String.fromCharCode(...rowBytes.map(b => (b >= 32 && b <= 126) ? b : 46))}
                            </span>
                          </div>
                        );
                      }}
                    </For>
                  </div>
                </div>
              </Show>
              <Show when={!props.stages?.binary || props.stages.binary.length === 0}>
                <p class="not-available">No binary data available</p>
              </Show>
            </div>
          </Show>

          <Show when={props.activeStage === 'decoded'}>
            <div class="stage-view decoded-view">
              <div class="stage-head">
                <h3>Binary Instruction Decoding</h3>
                <ArtifactButton
                  label="Download decoded (JSON)"
                  filename="decoded.json"
                  mime="application/json"
                  content={() => JSON.stringify(stages()?.binary_decoded ?? [], null, 2)}
                />
              </div>
              <p class="stage-description">
                Each instruction broken down into its encoding components: prefixes, opcode, ModR/M, SIB, displacement, and immediate values.
              </p>
              <Show when={props.stages?.binary_decoded && props.stages.binary_decoded.length > 0}>
                <div class="decoded-list">
                  <For each={props.stages!.binary_decoded}>
                    {(decoded, index) => (
                      <div class="decoded-instruction">
                        <div class="decoded-head">
                          <div class="decoded-title">
                            Instruction {index() + 1}
                          </div>
                          <div class="code-text decoded-offset-text">
                            Offset: 0x{decoded.offset.toString(16).padStart(8, '0')}
                          </div>
                        </div>

                        <div class="decoded-grid">
                          <div class="decoded-label">Raw Bytes:</div>
                          <div class="code-text decoded-bytes">
                            <For each={decoded.bytes}>
                              {(byte) => <span class="code-inline">{byte.toString(16).padStart(2, '0')}</span>}
                            </For>
                          </div>
                        </div>

                        <Show when={decoded.encoding.prefixes && decoded.encoding.prefixes.length > 0}>
                          <div class="decoded-grid">
                            <div class="decoded-label">Prefixes:</div>
                            <div>
                              <For each={decoded.encoding.prefixes}>
                                {(prefix) => (
                                  <div class="decoded-prefix">
                                    <span class="code-inline">
                                      0x{prefix.byte.toString(16).padStart(2, '0')}
                                    </span>
                                    <span class="decoded-prefix-meaning" title={prefix.purpose}>{prefix.name}</span>
                                  </div>
                                )}
                              </For>
                            </div>
                          </div>
                        </Show>

                        <div class="decoded-grid">
                          <div class="decoded-label">Opcode:</div>
                          <div class="code-text decoded-opcodes">
                            <For each={decoded.encoding.opcode}>
                              {(byte) => <span class="opcode-cell">{byte.toString(16).padStart(2, '0')}</span>}
                            </For>
                          </div>
                        </div>

                        <Show when={decoded.encoding.modrm}>
                          <div class="decoded-grid">
                            <div class="decoded-label">ModR/M:</div>
                            <div>
                              <span class="code-inline">
                                0x{decoded.encoding.modrm!.byte.toString(16).padStart(2, '0')}
                              </span>
                              <span class="decoded-note">{decoded.encoding.modrm!.explanation}</span>
                            </div>
                          </div>
                        </Show>

                        <Show when={decoded.encoding.sib}>
                          <div class="decoded-grid">
                            <div class="decoded-label">SIB:</div>
                            <div>
                              <span class="code-inline">
                                0x{decoded.encoding.sib!.byte.toString(16).padStart(2, '0')}
                              </span>
                              <span class="decoded-note">{decoded.encoding.sib!.explanation}</span>
                            </div>
                          </div>
                        </Show>

                        <Show when={decoded.encoding.displacement !== null && decoded.encoding.displacement !== undefined}>
                          <div class="decoded-grid">
                            <div class="decoded-label">Displacement:</div>
                            <div class="code-text decoded-value">
                              {decoded.encoding.displacement} (0x{Math.abs(decoded.encoding.displacement!).toString(16)})
                            </div>
                          </div>
                        </Show>

                        <Show when={decoded.encoding.immediate !== null && decoded.encoding.immediate !== undefined}>
                          <div class="decoded-grid">
                            <div class="decoded-label">Immediate:</div>
                            <div class="code-text decoded-value">
                              {decoded.encoding.immediate} (0x{Math.abs(Number(decoded.encoding.immediate)).toString(16)})
                            </div>
                          </div>
                        </Show>

                        <div class="decoded-footer">
                          {decoded.explanation}
                        </div>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={!props.stages?.binary_decoded || props.stages.binary_decoded.length === 0}>
                <p class="not-available">No decoded instruction data available</p>
              </Show>
            </div>
          </Show>

          <Show when={props.activeStage === 'diff'}>
            <div class="stage-view diff-view">
              <h3>Optimizer Diff (O0 vs O2)</h3>
              <p class="stage-description">
                Unoptimized LLVM IR on the left, optimized LLVM IR on the right. Lines only one side produces are highlighted.
              </p>
              <Show
                when={props.stages?.ir_unoptimized && props.stages?.ir_optimized}
                fallback={<p class="not-available">Both IR snapshots are needed to render a diff.</p>}
              >
                <MonacoDiff
                  original={props.stages!.ir_unoptimized}
                  modified={props.stages!.ir_optimized}
                  language="plaintext"
                />
              </Show>
            </div>
          </Show>
        </Show>
      </div>
    </div>
  );
};
