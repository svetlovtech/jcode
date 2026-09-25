    (function() {
      'use strict';

      // ============================================================
      // DATA LOADING (raw JSON in a script tag; "</" escaped as "<\/")
      // ============================================================

      const data = JSON.parse(document.getElementById('session-data').textContent);
      const { header, entries, stats, sessions } = data;

      // ============================================================
      // THEMES: light/dark/auto (default = browser preference)
      // ============================================================

      const THEME_KEY = 'jcode-export:theme';
      function applyTheme(mode) {
        // mode: 'auto' | 'light' | 'dark'
        document.documentElement.removeAttribute('data-theme-pref');
        if (mode === 'light' || mode === 'dark') {
          document.documentElement.setAttribute('data-theme-pref', mode);
        }
        try { localStorage.setItem(THEME_KEY, mode); } catch (e) { /* private mode */ }
        const btn = document.querySelector('[data-action="toggle-theme"]');
        if (btn) btn.textContent = mode === 'auto' ? 'Theme: auto' : mode === 'light' ? 'Theme: light' : 'Theme: dark';
      }
      function cycleTheme() {
        const cur = document.documentElement.getAttribute('data-theme-pref');
        applyTheme(cur === 'light' ? 'dark' : cur === 'dark' ? 'auto' : 'light');
      }
      try {
        const saved = localStorage.getItem(THEME_KEY);
        if (saved === 'light' || saved === 'dark') {
          document.documentElement.setAttribute('data-theme-pref', saved);
        }
      } catch (e) { /* ignore */ }

      // ============================================================
      // HELPERS
      // ============================================================

      function escapeHtml(text) {
        return String(text)
          .replace(/&/g, '&amp;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;')
          .replace(/"/g, '&quot;')
          .replace(/'/g, '&#39;');
      }

      function str(value) { return typeof value === 'string' ? value : null; }

      function truncate(s, maxLen) {
        s = String(s || '');
        maxLen = maxLen || 110;
        return s.length <= maxLen ? s : s.slice(0, maxLen) + '...';
      }

      function replaceTabs(text) { return String(text).replace(/\t/g, '    '); }
      function parseTs(ts) { return ts ? new Date(ts) : null; }

      function formatTimestamp(ts) {
        const d = parseTs(ts);
        return d && !isNaN(d.getTime()) ? d.toLocaleString() : '';
      }

      function formatTimeShort(ts) {
        const d = parseTs(ts);
        return d && !isNaN(d.getTime())
          ? d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })
          : '';
      }

      function formatTokens(n) {
        if (!n) return '0';
        if (n >= 1000000) return (n / 1000000).toFixed(1) + 'M';
        if (n >= 1000) return (n / 1000).toFixed(1) + 'k';
        return String(n);
      }

      function formatDuration(ms) {
        if (ms == null) return '';
        if (ms < 1000) return Math.round(ms) + 'ms';
        const s = ms / 1000;
        if (s < 60) return s.toFixed(s < 10 ? 1 : 0) + 's';
        const m = Math.floor(s / 60);
        return m + 'm ' + Math.round(s % 60) + 's';
      }

      function formatSpan(ms) {
        if (ms < 60000) return formatDuration(ms);
        const m = Math.floor(ms / 60000);
        const h = Math.floor(m / 60);
        return h > 0 ? h + 'h ' + (m % 60) + 'm' : m + 'm';
      }

      function getLanguageFromPath(path) {
        if (!path) return null;
        const base = String(path).split('/').pop().toLowerCase();
        const map = {
          rs: 'rust', js: 'javascript', mjs: 'javascript', ts: 'typescript', tsx: 'typescript',
          py: 'python', go: 'go', sh: 'bash', bash: 'bash', json: 'json', toml: 'toml',
          yaml: 'yaml', yml: 'yaml', md: 'markdown', sql: 'sql', css: 'css', html: 'html'
        };
        const ext = base.includes('.') ? base.split('.').pop() : '';
        return map[ext] || null;
      }

      function shortenPath(path) {
        const parts = String(path || '').split('/');
        return parts.length <= 3 ? path : '.../' + parts.slice(-2).join('/');
      }

      function sanitizeMarkdownUrl(value) {
        const href = String(value || '').trim().replace(/[\x00-\x1f\x7f]/g, '');
        if (!href) return href;
        const scheme = href.match(/^([A-Za-z][A-Za-z0-9+.-]*):/);
        return scheme && !/^(https?|mailto|tel|ftp)$/i.test(scheme[1]) ? null : href;
      }

      function safeMarkedParse(text) {
        try { return marked.parse(text); } catch (e) { return '<div>' + escapeHtml(text) + '</div>'; }
      }

      marked.use({
        breaks: true,
        gfm: true,
        tokenizer: { html() { return undefined; }, tag() { return undefined; } },
        renderer: {
          link(token) {
            const href = sanitizeMarkdownUrl(token.href);
            if (href === null) return this.parser.parseInline(token.tokens);
            let out = '<a href="' + escapeHtml(href) + '"';
            if (token.title) out += ' title="' + escapeHtml(token.title) + '"';
            return out + '>' + this.parser.parseInline(token.tokens) + '</a>';
          },
          // Highlight fenced code blocks with the built-in mini highlighter
          // (replaces the 119KB highlight.js bundle).
          code(token) {
            const lang = (token.lang || '').trim().split(/\s+/)[0];
            return '<pre><code class="hljs">' + miniHighlight(token.text || '', lang || null) + '</code></pre>';
          }
        }
      });

      // ============================================================
      // MINI HIGHLIGHTER (~2KB) - covers the languages jcode sessions use.
      // Not perfect; far smaller than a full highlighter bundle.
      // ============================================================

      const HL_LANGS = {
        rust: [
          [/(\/\/[^\n]*)/g, 'hljs-comment'],
          [/("(?:[^"\\]|\\.)*")/g, 'hljs-string'],
          [/\b(fn|let|mut|pub|struct|enum|impl|trait|use|mod|match|if|else|for|while|loop|return|self|Self|crate|super|where|async|await|move|dyn|const|static|type|ref|as|in|unsafe|box)\b/g, 'hljs-keyword'],
          [/\b(true|false|None|Some|Ok|Err)\b/g, 'hljs-literal'],
          [/\b(\d[\d_]*(?:\.[\d_]+)?(?:u8|u16|u32|u64|usize|i8|i16|i32|i64|isize|f32|f64)?)\b/g, 'hljs-number']
        ],
        javascript: [
          [/(\/\/[^\n]*)/g, 'hljs-comment'],
          [/("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`)/g, 'hljs-string'],
          [/\b(const|let|var|function|return|if|else|for|while|class|extends|new|this|typeof|instanceof|import|export|from|default|async|await|yield|try|catch|finally|throw|switch|case|break|continue|do|in|of|delete|void)\b/g, 'hljs-keyword'],
          [/\b(true|false|null|undefined|NaN)\b/g, 'hljs-literal'],
          [/\b(\d+(?:\.\d+)?)\b/g, 'hljs-number']
        ],
        python: [
          [/(#[^\n]*)/g, 'hljs-comment'],
          [/("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/g, 'hljs-string'],
          [/\b(def|class|return|if|elif|else|for|while|import|from|as|with|try|except|finally|raise|lambda|yield|async|await|pass|break|continue|global|nonlocal|assert|del|in|is|not|and|or)\b/g, 'hljs-keyword'],
          [/\b(True|False|None|self)\b/g, 'hljs-literal'],
          [/\b(\d+(?:\.\d+)?)\b/g, 'hljs-number']
        ],
        go: [
          [/(\/\/[^\n]*)/g, 'hljs-comment'],
          [/("(?:[^"\\]|\\.)*"|`(?:[^`\\]|\\.)*`)/g, 'hljs-string'],
          [/\b(func|package|import|var|const|type|struct|interface|map|chan|go|defer|if|else|for|range|switch|case|default|return|break|continue|fallthrough|select)\b/g, 'hljs-keyword'],
          [/\b(true|false|nil|iota)\b/g, 'hljs-literal'],
          [/\b(\d+(?:\.\d+)?)\b/g, 'hljs-number']
        ],
        bash: [
          [/(#[^\n]*)/g, 'hljs-comment'],
          [/("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/g, 'hljs-string'],
          [/\b(if|then|else|elif|fi|for|while|do|done|case|esac|function|return|export|local|source|set|unset|cd|echo|exit)\b/g, 'hljs-keyword'],
          [/\$\{?[A-Za-z_][A-Za-z0-9_]*\}?/g, 'hljs-attr']
        ],
        json: [
          [/("(?:[^"\\]|\\.)*")(\s*:)/g, 'hljs-attr'],
          [/(:\s*)("(?:[^"\\]|\\.)*")/g, null],
          [/\b(true|false|null)\b/g, 'hljs-literal'],
            [/-?\b\d+(?:\.\d+)?(?:e[+-]?\d+)?\b/gi, 'hljs-number']
        ]
      };
      HL_LANGS.typescript = HL_LANGS.javascript;
      HL_LANGS.tsx = HL_LANGS.javascript;

      function miniHighlight(code, lang) {
        const escaped = escapeHtml(code);
        const rules = HL_LANGS[lang];
        if (!rules) return escaped;
        // Tokenize with a combined regex per rule, escaping already-wrapped spans.
        let out = escaped;
        const stack = [];
        for (const [regex, cls] of rules) {
          if (!cls) continue;
          out = out.replace(regex, (match) => {
            const token = '\x00' + stack.length + '\x00';
            stack.push('<span class="' + cls + '">' + match + '</span>');
            return token;
          });
        }
        return out.replace(/\x00(\d+)\x00/g, (_, i) => stack[Number(i)]);
      }

      // ============================================================
      // TOOL RESULT INDEX
      // ============================================================

      const resultByCallId = new Map();
      for (const entry of entries) {
        if (entry.type === 'tool_result') resultByCallId.set(entry.tool_use_id, entry);
      }

      // ============================================================
      // OUTPUT + DIFF
      // ============================================================

      function formatExpandableOutput(text, maxLines, lang, startExpanded) {
        text = replaceTabs(text);
        const lines = text.split('\n');
        const remaining = lines.length - maxLines;
        const expandAttr = startExpanded ? ' expanded' : '';
        const highlight = (code) => miniHighlight(code, lang);

        if (remaining > 0) {
          return '<div class="tool-output expandable' + expandAttr + '" onclick="if(window.getSelection().toString())return;this.classList.toggle(\'expanded\')">' +
            '<div class="output-preview"><pre><code class="hljs">' + highlight(lines.slice(0, maxLines).join('\n')) + '</code></pre>' +
            '<div class="expand-hint">... (' + remaining + ' more lines - click)</div></div>' +
            '<div class="output-full"><pre><code class="hljs">' + highlight(text) + '</code></pre></div></div>';
        }
        return '<div class="tool-output"><pre><code class="hljs">' + highlight(text) + '</code></pre></div>';
      }

      function renderDiff(diff) {
        const lines = String(diff).split('\n');
        const added = lines.filter(l => l.startsWith('+')).length;
        const removed = lines.filter(l => l.startsWith('-')).length;
        let html = '<div class="tool-diff expandable" onclick="if(window.getSelection().toString())return;this.classList.toggle(\'expanded\')">';
        html += '<div class="diff-summary"><span class="diff-stat-added">+' + added + '</span> <span class="diff-stat-removed">-' + removed + '</span> (click to toggle)</div>';
        html += '<div class="diff-lines">';
        for (const line of lines) {
          const cls = line.startsWith('+') ? 'diff-added' : line.startsWith('-') ? 'diff-removed' : 'diff-context';
          html += '<div class="' + cls + '">' + escapeHtml(replaceTabs(line)) + '</div>';
        }
        return html + '</div></div>';
      }

      function pathFromArgs(args, keys) {
        for (const key of keys) {
          const value = str(args[key]);
          if (value) return value;
        }
        return null;
      }

      // ============================================================
      // TOOL CALL: compact row + expandable detail
      // ============================================================

      function oneLineSummary(name, args) {
        switch (name) {
          case 'bash': {
            const command = str(args.command);
            return command === null ? '[invalid arg]' : (command || '...');
          }
          case 'read': case 'write': case 'edit': {
            const filePath = pathFromArgs(args, ['file_path', 'path']);
            return filePath === null ? '[invalid path]' : (filePath || '');
          }
          case 'glob': case 'grep': {
            const pattern = str(args.pattern) || str(args.query) || '';
            const searchPath = pathFromArgs(args, ['path']);
            return pattern + (searchPath ? '  in ' + searchPath : '');
          }
          case 'agentgrep': return str(args.query) || str(args.file) || '';
          case 'webfetch': return str(args.url) || '';
          case 'subagent': return str(args.label) || str(args.prompt) || '';
          default: {
            const parts = [];
            for (const key of Object.keys(args)) {
              const value = args[key];
              if (value == null || key === 'intent') continue;
              const text = typeof value === 'string' ? value : JSON.stringify(value);
              if (!text) continue;
              parts.push(key + ': ' + truncate(text.replace(/\n/g, ' '), 48));
              if (parts.length >= 2) break;
            }
            return parts.join('  ');
          }
        }
      }

      function countResultLines(result) {
        return result && result.content ? result.content.split('\n').length || null : null;
      }

      function editDiffStat(args, result) {
        let source = null;
        if (result && result.content && /(^|\n)[+-]/.test(result.content)) {
          source = result.content;
        } else {
          const oldString = str(args.old_string);
          const newString = str(args.new_string);
          if (oldString !== null || newString !== null) {
            const parts = [];
            if (oldString) for (const line of oldString.split('\n')) parts.push('-' + line);
            if (newString) for (const line of newString.split('\n')) parts.push('+' + line);
            source = parts.join('\n');
          }
        }
        if (!source) return null;
        const lines = source.split('\n');
        const added = lines.filter(l => l.startsWith('+')).length;
        const removed = lines.filter(l => l.startsWith('-')).length;
        return added || removed ? '+' + added + ' -' + removed : null;
      }

      function renderToolCall(call) {
        const result = resultByCallId.get(call.id);
        const isError = result ? result.is_error : false;
        const args = call.arguments || {};
        const name = call.name;
        const duration = result && result.duration_ms != null ? result.duration_ms : null;

        const icon = !result ? '&#9679;' : (isError ? '&#10007;' : '&#10003;');
        const statusText = !result ? 'run' : (isError ? 'err' : 'ok');
        const summary = truncate(oneLineSummary(name, args), 96);
        const intentText = str(args.intent) || call.intent || '';
        const detail = escapeHtml(intentText && intentText !== summary ? intentText : summary);
        // Time + duration are the two numbers the user asked to see.
        const timeHtml = result && result.timestamp
          ? '<span class="tool-time">' + escapeHtml(formatTimeShort(result.timestamp)) + '</span>' : '';
        const durationHtml = duration != null
          ? '<span class="tool-duration">' + escapeHtml(formatDuration(duration)) + '</span>' : '';
        const resultLines = countResultLines(result);
        const linesBadge = resultLines != null && resultLines > 3
          ? '<span class="tool-lines">' + resultLines + ' lines</span>' : '';
        const diffStat = (name === 'edit' || name === 'write') ? editDiffStat(args, result) : null;
        const diffBadge = diffStat ? '<span class="tool-diffstat">' + escapeHtml(diffStat) + '</span>' : '';

        let body = '<div class="tool-detail">';
        body += '<div class="tool-args"><div class="detail-label">arguments</div>' +
          formatExpandableOutput(JSON.stringify(args, null, 2), 14, 'json', true) + '</div>';
        if (result && result.content && result.content.trim()) {
          const filePath = pathFromArgs(args, ['file_path', 'path']);
          const isDiff = name === 'edit' && /(^|\n)[+-]/.test(result.content);
          body += '<div class="tool-result"><div class="detail-label">result' +
            (isError ? ' (error)' : '') + '</div>' +
            (isDiff ? renderDiff(result.content)
                    : formatExpandableOutput(result.content, 16, isError ? null : getLanguageFromPath(filePath))) +
            '</div>';
        }
        body += '</div>';

        return '<div class="tool-execution ' + (result ? (isError ? 'error' : 'success') : 'pending') + '" id="tool-call-' + escapeHtml(call.id) + '">' +
          '<div class="tool-row" onclick="if(window.getSelection().toString())return;this.parentElement.classList.toggle(\'open\')">' +
          '<span class="tool-status tool-status-' + statusText + '">' + icon + '</span>' +
          '<span class="tool-name">' + escapeHtml(name) + '</span>' +
          '<span class="tool-detail-summary">' + detail + '</span>' +
          diffBadge + linesBadge + durationHtml + timeHtml +
          '</div>' + body + '</div>';
      }

      // ============================================================
      // ENTRY RENDERING
      // ============================================================

      let turnGroupOpen = false;
      function closeTurnGroupIfOpen() {
        if (turnGroupOpen) { turnGroupOpen = false; return '</div>'; }
        return '';
      }

      function renderEntry(entry) {
        const ts = formatTimestamp(entry.timestamp);
        const tsHtml = ts ? '<div class="message-timestamp">' + escapeHtml(ts) + '</div>' : '';
        const entryDomId = 'entry-' + escapeHtml(entry.id);

        if (entry.type === 'user') {
          const roleTag = entry.display_role
            ? '<div class="display-role-tag">[' + escapeHtml(entry.display_role) + ']</div>' : '';
          turnGroupOpen = true;
          return '<div class="turn-group" id="turn-' + escapeHtml(entry.id) + '">' +
            '<div class="turn-toggle" onclick="if(window.getSelection().toString())return;this.parentElement.classList.toggle(\'collapsed-turn\')" title="Collapse/expand this turn">&#9662;</div>' +
            '<div class="user-message" id="' + entryDomId + '" data-user-prompt>' + roleTag + tsHtml +
            '<div class="markdown-content md-pending">' + escapeHtml(entry.text) + '</div></div>';
        }

        if (entry.type === 'assistant') {
          let html = '<div class="assistant-message" id="' + entryDomId + '">' + tsHtml;
          for (const thinking of entry.thinking) {
            html += '<div class="thinking-block collapsed" onclick="if(window.getSelection().toString())return;this.classList.toggle(\'collapsed\')">' +
              '<div class="thinking-label">&#9654; thinking</div>' +
              '<div class="thinking-text">' + escapeHtml(thinking) + '</div></div>';
          }
          if (entry.text && entry.text.trim()) {
            html += '<div class="assistant-text markdown-content md-pending">' + escapeHtml(entry.text) + '</div>';
          }
          for (const call of entry.tool_calls) html += renderToolCall(call);
          html += '</div>' + closeTurnGroupIfOpen();
          return html;
        }

        if (entry.type === 'tool_result') return '';

        if (entry.type === 'compaction') {
          return '<div class="compaction" id="' + entryDomId + '" onclick="if(window.getSelection().toString())return;this.classList.toggle(\'expanded\')">' +
            '<div class="compaction-label">[compaction]</div>' +
            '<div class="compaction-collapsed">Compacted history summary (click to expand)</div>' +
            '<div class="compaction-content">' + escapeHtml(entry.summary) + '</div></div>';
        }

        return '';
      }

      // ============================================================
      // GANTT v2: one row per TURN (prompt -> end of its response), honest
      // duration bars, time ruler. Subagent sessions get labeled lanes.
      // ============================================================

      function buildTurns() {
        // Group entries into turns: each user entry starts a turn; everything
        // up to the next user entry belongs to it. Entries before the first
        // user prompt form a prelude turn.
        const turns = [];
        let current = null;
        for (const entry of entries) {
          const t = parseTs(entry.timestamp);
          const time = t && !isNaN(t.getTime()) ? t.getTime() : null;
          if (entry.type === 'user' || !current) {
            current = { start: time, end: time, user: entry, items: [], sessionId: entry.session_id || (sessions && sessions.length ? sessions[0].id : null) };
            turns.push(current);
            continue;
          }
          current.items.push({ entry, time });
          if (time != null) {
            if (current.start == null) current.start = time;
            current.end = time;
          }
        }
        // Turn end = last item time (+ its tool duration if known).
        for (const turn of turns) {
          let end = turn.end;
          for (const item of turn.items) {
            if (item.entry.type === 'tool_result' && item.entry.duration_ms != null && item.time != null) {
              end = Math.max(end, item.time + item.entry.duration_ms);
            }
          }
          turn.end = end;
        }
        return turns.filter(t => t.start != null);
      }

      function turnLabel(turn) {
        const text = turn.user.text.replace(/\s+/g, ' ').trim();
        const count = turn.items.reduce((n, i) => n + (i.entry.type === 'tool_result' ? 1 : 0), 0);
        const dur = turn.end != null && turn.start != null ? formatSpan(Math.max(turn.end - turn.start, 0)) : '';
        // Single line in the label column; CSS adds an ellipsis.
        return {
          text: text.length > 90 ? text.slice(0, 90) + '...' : (text || 'session start'),
          tools: count,
          duration: dur
        };
      }

      function renderGantt() {
        const container = document.getElementById('gantt-container');
        if (!container) return;
        const turns = buildTurns();
        if (turns.length === 0) { container.closest('.gantt').style.display = 'none'; return; }

        const start = turns[0].start;
        const end = Math.max(...turns.map(t => t.end || t.start));
        const span = Math.max(end - start, 1);
        // Ruler: ~5 ticks, unit rounded to a nice step, labels in that unit.
        const niceSteps = [
          [1000, (v) => Math.round(v / 1000) + 's'],
          [5000, (v) => Math.round(v / 5000) + 's'],
          [15000, (v) => Math.round(v / 15000) * 15 + 's'],
          [30000, (v) => Math.round(v / 30000) * 30 + 's'],
          [60000, (v) => Math.round(v / 60000) + 'm'],
          [300000, (v) => Math.round(v / 300000) * 5 + 'm'],
          [900000, (v) => Math.round(v / 900000) * 15 + 'm'],
          [1800000, (v) => Math.round(v / 1800000) * 30 + 'm'],
          [3600000, (v) => Math.round(v / 3600000) + 'h']
        ];
        const target = span / 5;
        const step = niceSteps.find(([s]) => s >= target) || niceSteps[niceSteps.length - 1];
        const div = Math.max(1, Math.floor(span / step[0]));
        const unitMs = step[0];
        const rulerLabels = [];
        for (let i = 0; i <= div; i++) rulerLabels.push(step[1](i * unitMs));

        let html = '<div class="gantt-ruler"><span>0</span>' +
          rulerLabels.slice(1).map(l => '<span>' + escapeHtml(l) + '</span>').join('') + '</div>';
        html += '<div class="gantt-body">';

        const multi = sessions && sessions.length > 1;
        if (multi) {
          const metaById = new Map(sessions.map(s => [s.id, s]));
          for (const session of sessions) {
            const laneTurns = turns.filter(t => (t.sessionId || sessions[0].id) === session.id);
            if (!laneTurns.length) continue;
            html += '<div class="gantt-lane-label">' + (session.is_primary ? '&#9733; ' : '&#8901; ') +
              escapeHtml(session.custom_title || session.short_name || session.id) +
              (session.model ? ' <span class="muted">(' + escapeHtml(session.model) + ')</span>' : '') + '</div>';
            html += renderGanttRows(laneTurns, start, span);
          }
        } else {
          html += renderGanttRows(turns, start, span);
        }
        html += '</div>';
        container.innerHTML = html;

        container.querySelectorAll('.gantt-row').forEach(node => {
          node.addEventListener('click', () => jumpTo(node.dataset.target));
        });
      }

      function renderGanttRows(turns, start, span) {
        let html = '';
        for (const turn of turns) {
          const left = ((turn.start - start) / span) * 100;
          const width = Math.min(Math.max(((turn.end || turn.start) - turn.start) / span * 100, 1.2), 100 - left);
          const label = turnLabel(turn);
          const target = turn.user ? 'entry-' + escapeHtml(turn.user.id) : '';
          html += '<div class="gantt-row" data-target="' + target + '" title="' + escapeHtml(label.text) + ' - ' + escapeHtml(label.duration) + '">' +
            '<div class="gantt-label">' + escapeHtml(label.text) + '</div>' +
            '<div class="gantt-bar-track"><div class="gantt-bar" style="left:' + left.toFixed(2) + '%;width:' + width.toFixed(2) + '%"></div></div>' +
            '<div class="gantt-meta">' +
            (label.tools ? '<span>' + label.tools + ' tools</span>' : '<span></span>') +
            '<span class="gantt-dur">' + escapeHtml(label.duration) + '</span>' +
            '<span class="gantt-time">' + escapeHtml(formatTimeShort(turn.user.timestamp)) + '</span>' +
            '</div></div>';
        }
        return html;
      }

      // ============================================================
      // SIDEBAR TREE
      // ============================================================

      let filterMode = 'default';
      let searchQuery = '';

      function findToolCall(callId) {
        for (const entry of entries) {
          if (entry.type === 'assistant') {
            for (const call of entry.tool_calls) if (call.id === callId) return call;
          }
        }
        return null;
      }

      function treeNodeText(entry) {
        switch (entry.type) {
          case 'user': return { role: 'user', text: truncate(entry.text.replace(/[\n\t]/g, ' ').trim()) };
          case 'assistant': {
            const text = entry.text.replace(/[\n\t]/g, ' ').trim();
            if (text) return { role: 'assistant', text: truncate(text) };
            if (entry.tool_calls.length) {
              const names = entry.tool_calls.map(c => c.name);
              const uniq = Array.from(new Set(names));
              return { role: 'tool', text: truncate(uniq.join(', ') + (names.length === uniq.length ? '' : ' x' + names.length), 90) };
            }
            return { role: 'assistant', text: '(no text)' };
          }
          case 'tool_result': {
            const call = findToolCall(entry.tool_use_id);
            return { role: 'tool', text: truncate((call ? call.name : 'tool') + ' result', 90) };
          }
          case 'compaction': return { role: 'compaction', text: 'compaction summary' };
          default: return { role: 'tool', text: '' };
        }
      }

      function searchableText(entry) {
        switch (entry.type) {
          case 'user': return entry.text;
          case 'assistant': return entry.text + '\n' + entry.thinking.join('\n') +
            '\n' + entry.tool_calls.map(c => c.name + ' ' + JSON.stringify(c.arguments || {})).join('\n');
          case 'tool_result': return entry.content;
          case 'compaction': return entry.summary;
          default: return '';
        }
      }

      function filterEntries(list) {
        const tokens = searchQuery.toLowerCase().split(/\s+/).filter(Boolean);
        return list.filter(entry => {
          if (filterMode === 'user-only') { if (entry.type !== 'user') return false; }
          else if (filterMode === 'default') {
            if (entry.type === 'assistant' && !entry.text.trim() && !entry.thinking.length) return false;
            if (entry.type === 'tool_result') return false;
          }
          if (tokens.length) {
            const hay = searchableText(entry).toLowerCase();
            if (!tokens.every(t => hay.includes(t))) return false;
          }
          return true;
        });
      }

      function renderTree() {
        const container = document.getElementById('tree-container');
        const visible = filterEntries(entries);
        const fragments = [];
        for (const entry of visible) {
          const info = treeNodeText(entry);
          const roleClass = {
            user: 'tree-role-user', assistant: 'tree-role-assistant',
            tool: 'tree-role-tool', compaction: 'tree-compaction'
          }[info.role] || 'tree-muted';
          const prefix = info.role === 'user' ? '> ' : info.role === 'assistant' ? '* ' : info.role === 'tool' ? '- ' : '# ';
          fragments.push('<div class="tree-node" data-target="entry-' + escapeHtml(entry.id) + '">' +
            '<span class="tree-time">' + escapeHtml(formatTimeShort(entry.timestamp)) + '</span>' +
            '<span class="tree-prefix">' + prefix + '</span>' +
            '<span class="tree-content"><span class="' + roleClass + '">' + escapeHtml(info.text) + '</span></span></div>');
        }
        container.innerHTML = fragments.join('');
        document.getElementById('tree-status').textContent = visible.length + ' / ' + entries.length + ' entries';
        container.querySelectorAll('.tree-node').forEach(node => {
          node.addEventListener('click', () => {
            container.querySelectorAll('.tree-node.active').forEach(n => n.classList.remove('active'));
            node.classList.add('active');
            jumpTo(node.dataset.target);
            if (window.innerWidth <= 900) closeSidebar();
          });
        });
      }

      function jumpTo(domId) {
        const target = document.getElementById(domId);
        if (!target) return;
        target.scrollIntoView({ behavior: 'smooth', block: 'start' });
        target.classList.add('flash');
        setTimeout(() => target.classList.remove('flash'), 1600);
      }

      // ============================================================
      // HEADER
      // ============================================================

      function sessionTitle() {
        return header.custom_title || header.title ||
          (header.short_name ? header.short_name + ' (' + header.id + ')' : header.id);
      }

      function sessionDurationText() {
        const created = parseTs(header.created_at);
        const updated = parseTs(header.updated_at);
        if (!created || !updated || isNaN(created.getTime()) || isNaN(updated.getTime())) return '';
        return formatSpan(Math.max(updated.getTime() - created.getTime(), 0));
      }

      function toolTopTools() {
        const counts = new Map();
        for (const entry of entries) {
          if (entry.type !== 'assistant') continue;
          for (const call of entry.tool_calls) counts.set(call.name, (counts.get(call.name) || 0) + 1);
        }
        return Array.from(counts.entries()).sort((a, b) => b[1] - a[1]).slice(0, 5);
      }

      function renderHeader() {
        const parts = [
          stats.user_messages + ' user',
          stats.assistant_messages + ' assistant',
          stats.tool_calls + ' tool calls'
        ];
        if (stats.compactions) parts.push(stats.compactions + ' compactions');
        if (sessions && sessions.length > 1) parts.push((sessions.length - 1) + ' subagent sessions');

        const tokenParts = [];
        if (stats.input_tokens) tokenParts.push('\u2191' + formatTokens(stats.input_tokens));
        if (stats.output_tokens) tokenParts.push('\u2193' + formatTokens(stats.output_tokens));
        if (stats.cache_read_tokens) tokenParts.push('R' + formatTokens(stats.cache_read_tokens));
        if (stats.cache_creation_tokens) tokenParts.push('W' + formatTokens(stats.cache_creation_tokens));

        const durationText = sessionDurationText();
        const topTools = toolTopTools();
        const modelValue = (header.provider_key ? header.provider_key + '/' : '') + (header.model || 'unknown');

        const html = '<div class="header">' +
          '<h1>jcode session: ' + escapeHtml(sessionTitle()) + '</h1>' +
          '<div class="muted" style="font-size: 10px; margin-bottom: 8px">Keys: ? help &middot; T thinking &middot; O tool details &middot; G timeline &middot; J/K prompts &middot; Shift+C turns &middot; Y theme</div>' +
          '<div class="header-actions">' +
          '<button type="button" class="header-toggle-btn" data-action="toggle-theme" title="Cycle theme (Y): auto -> light -> dark">' + themeButtonLabel() + '</button>' +
          '<button type="button" class="header-toggle-btn" data-action="toggle-thinking" title="Toggle thinking (T)">Thinking</button>' +
          '<button type="button" class="header-toggle-btn" data-action="toggle-tools" title="Expand/collapse all tool details (O)">Tool details</button>' +
          '<button type="button" class="header-toggle-btn" data-action="toggle-gantt" title="Show/hide timeline (G)">Timeline</button>' +
          '<button type="button" class="download-json-btn" data-action="download-json" title="Download full session JSON">\u2193 JSON</button>' +
          '</div>' +
          '<div class="header-info">' +
          '<div class="info-item"><span class="info-label">Session:</span><span class="info-value">' + escapeHtml(header.id) + '</span></div>' +
          '<div class="info-item"><span class="info-label">Created:</span><span class="info-value">' + escapeHtml(formatTimestamp(header.created_at)) +
          (durationText ? ' <span class="muted">(' + escapeHtml(durationText) + ')</span>' : '') + '</span></div>' +
          '<div class="info-item"><span class="info-label">Model:</span><span class="info-value">' + escapeHtml(modelValue) + '</span></div>' +
          (header.working_dir ? '<div class="info-item"><span class="info-label">Workdir:</span><span class="info-value">' + escapeHtml(header.working_dir) + '</span></div>' : '') +
          '<div class="info-item"><span class="info-label">Messages:</span><span class="info-value">' + parts.join(', ') + '</span></div>' +
          (topTools.length ? '<div class="info-item"><span class="info-label">Top tools:</span><span class="info-value">' +
            topTools.map(([name, count]) => escapeHtml(name) + ' \u00d7' + count).join(', ') + '</span></div>' : '') +
          '<div class="info-item"><span class="info-label">Tokens:</span><span class="info-value">' + (tokenParts.join(' ') || '-') + '</span></div>' +
          '</div></div>';
        document.getElementById('header-container').innerHTML = html;

        document.querySelector('[data-action="toggle-theme"]').addEventListener('click', cycleTheme);
        document.querySelector('[data-action="toggle-thinking"]').addEventListener('click', toggleThinking);
        document.querySelector('[data-action="toggle-tools"]').addEventListener('click', toggleToolOutputs);
        document.querySelector('[data-action="toggle-gantt"]').addEventListener('click', toggleGantt);
        document.querySelector('[data-action="download-json"]').addEventListener('click', downloadSessionJson);
      }

      function themeButtonLabel() {
        const pref = document.documentElement.getAttribute('data-theme-pref');
        return pref === 'light' ? 'Theme: light' : pref === 'dark' ? 'Theme: dark' : 'Theme: auto';
      }

      function downloadSessionJson() {
        const json = JSON.stringify({ header, entries, stats, sessions }, null, 2);
        const blob = new Blob([json], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = (header.id || 'session') + '.json';
        document.body.appendChild(a);
        a.click();
        a.remove();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
      }

      // ============================================================
      // TOGGLES + KEYBOARD
      // ============================================================

      let thinkingForced = null;
      let toolOutputsExpanded = false;
      let ganttVisible = true;

      function toggleThinking() {
        thinkingForced = thinkingForced === null ? true : !thinkingForced;
        document.querySelectorAll('.thinking-block').forEach(el => {
          el.classList.toggle('collapsed', !thinkingForced);
        });
      }

      function toggleToolOutputs() {
        toolOutputsExpanded = !toolOutputsExpanded;
        document.querySelectorAll('.tool-execution').forEach(el => el.classList.toggle('open', toolOutputsExpanded));
      }

      function toggleGantt() {
        ganttVisible = !ganttVisible;
        const gantt = document.querySelector('.gantt');
        if (gantt) gantt.classList.toggle('hidden', !ganttVisible);
      }

      function jumpUserPrompt(direction) {
        const ids = Array.from(document.querySelectorAll('[data-user-prompt]')).map(el => el.id);
        if (!ids.length) return;
        let target = null;
        if (direction > 0) {
          target = ids.find(id => document.getElementById(id).getBoundingClientRect().top > 80);
        } else {
          for (let i = ids.length - 1; i >= 0; i--) {
            if (document.getElementById(ids[i]).getBoundingClientRect().top < -40) { target = ids[i]; break; }
          }
          if (!target) target = ids[0];
        }
        if (target) jumpTo(target);
      }

      function toggleAllTurns() {
        const groups = document.querySelectorAll('.turn-group');
        if (!groups.length) return;
        const anyExpanded = Array.from(groups).some(g => !g.classList.contains('collapsed-turn'));
        groups.forEach(g => g.classList.toggle('collapsed-turn', anyExpanded));
      }

      function toggleHelpOverlay() {
        let overlay = document.getElementById('help-overlay');
        if (overlay) { overlay.remove(); return; }
        overlay = document.createElement('div');
        overlay.id = 'help-overlay';
        overlay.innerHTML =
          '<div class="help-card"><h2>Keyboard</h2><table>' +
          '<tr><td>?</td><td>this help</td></tr>' +
          '<tr><td>Y</td><td>cycle theme (auto / light / dark)</td></tr>' +
          '<tr><td>T</td><td>show/hide thinking</td></tr>' +
          '<tr><td>O</td><td>expand/collapse all tool details</td></tr>' +
          '<tr><td>G</td><td>show/hide timeline</td></tr>' +
          '<tr><td>J / K</td><td>next / previous user prompt</td></tr>' +
          '<tr><td>Shift+C</td><td>collapse/expand all turns</td></tr>' +
          '<tr><td>Esc</td><td>clear search / close help</td></tr>' +
          '</table><p>Click a tool row for arguments and result. Timeline rows jump to turns.</p></div>' +
          '<div class="help-backdrop"></div>';
        overlay.querySelector('.help-backdrop').addEventListener('click', () => overlay.remove());
        document.body.appendChild(overlay);
      }

      function setupScrollUi() {
        const progress = document.getElementById('read-progress');
        const topBtn = document.getElementById('back-to-top');
        window.addEventListener('scroll', () => {
          const doc = document.documentElement;
          const max = doc.scrollHeight - window.innerHeight;
          progress.style.width = (max > 0 ? (window.scrollY / max) * 100 : 0).toFixed(1) + '%';
          topBtn.classList.toggle('visible', window.scrollY > 900);
        }, { passive: true });
        topBtn.addEventListener('click', () => window.scrollTo({ top: 0, behavior: 'smooth' }));
      }

      // ============================================================
      // SIDEBAR RESIZE + MOBILE
      // ============================================================

      const WIDTH_KEY = 'jcode-export:sidebar-width';

      function setupSidebarResize() {
        const resizer = document.getElementById('sidebar-resizer');
        const sidebar = document.getElementById('sidebar');
        let startX = 0, startWidth = 0;

        const onMove = (e) => {
          const width = Math.min(720, Math.max(240, startWidth + (e.clientX - startX)));
          sidebar.style.width = sidebar.style.minWidth = sidebar.style.maxWidth = width + 'px';
        };
        const onUp = () => {
          document.body.classList.remove('sidebar-resizing');
          window.removeEventListener('pointermove', onMove);
          window.removeEventListener('pointerup', onUp);
          const match = sidebar.style.width.match(/^(\d+)px$/);
          if (match) { try { localStorage.setItem(WIDTH_KEY, match[1]); } catch (e) {} }
        };
        resizer.addEventListener('pointerdown', (e) => {
          startX = e.clientX;
          startWidth = sidebar.getBoundingClientRect().width;
          document.body.classList.add('sidebar-resizing');
          window.addEventListener('pointermove', onMove);
          window.addEventListener('pointerup', onUp);
          e.preventDefault();
        });
        try {
          const width = parseInt(localStorage.getItem(WIDTH_KEY), 10);
          if (width >= 240 && width <= 720) {
            sidebar.style.width = sidebar.style.minWidth = sidebar.style.maxWidth = width + 'px';
          }
        } catch (e) {}
      }

      function closeSidebar() {
        document.getElementById('sidebar').classList.remove('open');
        document.getElementById('sidebar-overlay').classList.remove('open');
      }

      // ============================================================
      // INIT
      // ============================================================

      const searchInput = document.getElementById('tree-search');
      searchInput.addEventListener('input', (e) => { searchQuery = e.target.value; renderTree(); });

      document.querySelectorAll('.filter-btn').forEach(btn => {
        btn.addEventListener('click', () => {
          document.querySelectorAll('.filter-btn').forEach(b => b.classList.remove('active'));
          btn.classList.add('active');
          filterMode = btn.dataset.filter;
          renderTree();
        });
      });

      const sidebar = document.getElementById('sidebar');
      const overlay = document.getElementById('sidebar-overlay');
      document.getElementById('hamburger').addEventListener('click', () => {
        sidebar.classList.add('open');
        overlay.classList.add('open');
      });
      overlay.addEventListener('click', closeSidebar);
      document.getElementById('sidebar-close').addEventListener('click', closeSidebar);

      const isEditableTarget = (element) => {
        if (!element) return false;
        const tag = element.tagName;
        return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || tag === 'BUTTON' || element.isContentEditable;
      };

      document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') {
          searchInput.value = ''; searchQuery = ''; renderTree();
          const help = document.getElementById('help-overlay');
          if (help) help.remove();
        }
        if (isEditableTarget(document.activeElement)) return;
        const key = e.key.toLowerCase();
        if (key === '?' || (e.shiftKey && key === '/')) { e.preventDefault(); toggleHelpOverlay(); }
        else if (e.shiftKey && key === 'c') { e.preventDefault(); toggleAllTurns(); }
        else if (key === 'y') { e.preventDefault(); cycleTheme(); }
        else if (key === 't') { e.preventDefault(); toggleThinking(); }
        else if (key === 'o') { e.preventDefault(); toggleToolOutputs(); }
        else if (key === 'g') { e.preventDefault(); toggleGantt(); }
        else if (key === 'j' || e.key === 'PageDown') { e.preventDefault(); jumpUserPrompt(1); }
        else if (key === 'k' || e.key === 'PageUp') { e.preventDefault(); jumpUserPrompt(-1); }
      });

      setupSidebarResize();
      setupScrollUi();
      renderHeader();
      renderGantt();

      const messagesEl = document.getElementById('messages');
      if (entries.length === 0) {
        messagesEl.innerHTML = '<div class="empty-state">This session has no exportable messages.</div>';
      } else {
        const parts = [];
        for (const entry of entries) {
          const html = renderEntry(entry);
          if (html) parts.push(html);
        }
        messagesEl.innerHTML = parts.join('');
      }

      renderTree();

      // Lazy markdown: plain text at load; render markdown when the block
      // approaches the viewport. Keeps first paint fast on big sessions.
      const mdObserver = new IntersectionObserver((seen) => {
        for (const item of seen) {
          if (!item.isIntersecting) continue;
          const el = item.target;
          mdObserver.unobserve(el);
          el.classList.remove('md-pending');
          el.innerHTML = safeMarkedParse(el.textContent);
        }
      }, { rootMargin: '600px' });
      document.querySelectorAll('.markdown-content.md-pending').forEach(el => mdObserver.observe(el));

      if (window.location.hash && window.location.hash.startsWith('#entry-')) {
        const domId = window.location.hash.slice(1);
        setTimeout(() => jumpTo(domId), 100);
      }
    })();
