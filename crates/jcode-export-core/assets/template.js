    (function() {
      'use strict';

      // ============================================================
      // DATA LOADING (base64 -> utf8 -> JSON, survives any quoting)
      // ============================================================

      const base64 = document.getElementById('session-data').textContent.trim();
      const binary = atob(base64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) {
        bytes[i] = binary.charCodeAt(i);
      }
      const data = JSON.parse(new TextDecoder('utf-8').decode(bytes));
      const { header, entries, stats, sessions } = data;

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

      function str(value) {
        return typeof value === 'string' ? value : null;
      }

      function truncate(s, maxLen) {
        s = String(s || '');
        maxLen = maxLen || 110;
        return s.length <= maxLen ? s : s.slice(0, maxLen) + '...';
      }

      function replaceTabs(text) {
        return String(text).replace(/\t/g, '    ');
      }

      function parseTs(ts) {
        return ts ? new Date(ts) : null;
      }

      function formatTimestamp(ts) {
        const d = parseTs(ts);
        if (!d || isNaN(d.getTime())) return '';
        return d.toLocaleString();
      }

      function formatTimeShort(ts) {
        const d = parseTs(ts);
        if (!d || isNaN(d.getTime())) return '';
        return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
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
        const rem = Math.round(s % 60);
        return m + 'm ' + rem + 's';
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
          rs: 'rust', js: 'javascript', mjs: 'javascript', cjs: 'javascript', ts: 'typescript',
          tsx: 'typescript', jsx: 'javascript', py: 'python', rb: 'ruby', go: 'go',
          c: 'c', h: 'c', cpp: 'cpp', cc: 'cpp', hpp: 'cpp', java: 'java',
          sh: 'bash', bash: 'bash', zsh: 'bash', json: 'json', yaml: 'yaml', yml: 'yaml',
          toml: 'toml', md: 'markdown', html: 'xml', css: 'css', sql: 'sql',
          php: 'php', swift: 'swift', kt: 'kotlin', cs: 'csharp', xml: 'xml'
        };
        const ext = base.includes('.') ? base.split('.').pop() : '';
        return map[ext] || null;
      }

      function shortenPath(path) {
        const parts = String(path || '').split('/');
        if (parts.length <= 3) return path;
        return '.../' + parts.slice(-2).join('/');
      }

      function sanitizeMarkdownUrl(value) {
        const href = String(value || '').trim().replace(/[\x00-\x1f\x7f]/g, '');
        if (!href) return href;
        const scheme = href.match(/^([A-Za-z][A-Za-z0-9+.-]*):/);
        if (scheme && !/^(https?|mailto|tel|ftp)$/i.test(scheme[1])) {
          return null;
        }
        return href;
      }

      function safeMarkedParse(text) {
        try {
          return marked.parse(text);
        } catch (e) {
          return '<div>' + escapeHtml(text) + '</div>';
        }
      }

      marked.use({
        breaks: true,
        gfm: true,
        tokenizer: {
          html() { return undefined; },
          tag() { return undefined; }
        },
        renderer: {
          link(token) {
            const href = sanitizeMarkdownUrl(token.href);
            if (href === null) {
              return this.parser.parseInline(token.tokens);
            }
            let out = '<a href="' + escapeHtml(href) + '"';
            if (token.title) out += ' title="' + escapeHtml(token.title) + '"';
            out += '>' + this.parser.parseInline(token.tokens) + '</a>';
            return out;
          }
        }
      });

      // ============================================================
      // TOOL RESULT INDEX + GANTT DATA
      // ============================================================

      const resultByCallId = new Map();
      for (const entry of entries) {
        if (entry.type === 'tool_result') {
          resultByCallId.set(entry.tool_use_id, entry);
        }
      }

      // ============================================================
      // TOOL OUTPUT (highlighted, expandable)
      // ============================================================

      function formatExpandableOutput(text, maxLines, lang, startExpanded) {
        text = replaceTabs(text);
        const lines = text.split('\n');
        const displayLines = lines.slice(0, maxLines);
        const remaining = lines.length - maxLines;
        const expandAttr = startExpanded ? ' expanded' : '';

        const highlight = (code) => {
          if (!lang) return escapeHtml(code);
          try {
            return hljs.highlight(code, { language: lang }).value;
          } catch (e) {
            return escapeHtml(code);
          }
        };

        if (remaining > 0) {
          return '<div class="tool-output expandable' + expandAttr + '" onclick="if(window.getSelection().toString())return;this.classList.toggle(\'expanded\')">' +
            '<div class="output-preview"><pre><code class="hljs">' + highlight(displayLines.join('\n')) + '</code></pre>' +
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
        html += '<div class="diff-summary">+' + added + ' / -' + removed + ' lines (click to ' +
          (lines.length > 12 ? 'show ' + lines.length + ' diff lines)' : 'toggle)') + '</div>';
        html += '<div class="diff-lines">';
        for (const line of lines) {
          const cls = line.startsWith('+') ? 'diff-added' : line.startsWith('-') ? 'diff-removed' : 'diff-context';
          html += '<div class="' + cls + '">' + escapeHtml(replaceTabs(line)) + '</div>';
        }
        html += '</div></div>';
        return html;
      }

      function pathFromArgs(args, keys) {
        for (const key of keys) {
          const value = str(args[key]);
          if (value) return value;
        }
        return null;
      }

      // ============================================================
      // TOOL CALL: compact TUI-style row + expandable detail
      // ============================================================

      function oneLineSummary(name, args) {
        switch (name) {
          case 'bash': {
            const command = str(args.command);
            return command === null ? '[invalid arg]' : (command || '...');
          }
          case 'read':
          case 'write':
          case 'edit': {
            const filePath = pathFromArgs(args, ['file_path', 'path']);
            return filePath === null ? '[invalid path]' : (filePath || '');
          }
          case 'glob':
          case 'grep': {
            const pattern = str(args.pattern) || str(args.query) || '';
            const searchPath = pathFromArgs(args, ['path']);
            return pattern + (searchPath ? '  in ' + searchPath : '');
          }
          case 'agentgrep': {
            return str(args.query) || str(args.file) || '';
          }
          case 'webfetch': {
            return str(args.url) || '';
          }
          case 'subagent': {
            return str(args.label) || str(args.prompt) || '';
          }
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
        if (!result || !result.content) return null;
        const lines = result.content.split('\n').length;
        return lines > 0 ? lines : null;
      }

      function editDiffStat(args, result) {
        // Prefer result content diff lines; fall back to old/new_string args.
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
        if (!added && !removed) return null;
        return '+' + added + ' -' + removed;
      }

      function renderToolCall(call) {
        const result = resultByCallId.get(call.id);
        const isError = result ? result.is_error : false;
        const args = call.arguments || {};
        const name = call.name;
        const duration = result && result.duration_ms != null ? result.duration_ms : null;

        // Compact status row (TUI-like): status icon, tool name, one-line info.
        const icon = !result ? '&#9679;' : (isError ? '&#10007;' : '&#10003;');
        const statusText = !result ? 'run' : (isError ? 'err' : 'ok');
        const summary = truncate(oneLineSummary(name, args), 96);
        const intentText = str(args.intent) || call.intent || '';
        const detail = escapeHtml(intentText && intentText !== summary ? intentText : summary);
        const durationHtml = duration != null
          ? '<span class="tool-duration">' + escapeHtml(formatDuration(duration)) + '</span>' : '';
        const timeHtml = result && result.timestamp
          ? '<span class="tool-time">' + escapeHtml(formatTimeShort(result.timestamp)) + '</span>' : '';

        // Extra row badges: result line count + edit diff stat.
        const resultLines = countResultLines(result);
        const linesBadge = (resultLines != null && resultLines > 3)
          ? '<span class="tool-lines">' + resultLines + ' lines</span>' : '';
        const diffStat = (name === 'edit' || name === 'write') ? editDiffStat(args, result) : null;
        const diffBadge = diffStat
          ? '<span class="tool-diffstat">' + escapeHtml(diffStat) + '</span>' : '';

        // Expanded body: full arguments + full output.
        let body = '<div class="tool-detail">';
        body += '<div class="tool-args"><div class="detail-label">arguments</div>' +
          formatExpandableOutput(JSON.stringify(args, null, 2), 14, 'json', true) + '</div>';
        if (result) {
          const content = result.content || '';
          if (content.trim()) {
            const isDiff = (name === 'edit') && /(^|\n)[+-]/.test(content);
            if (isDiff) {
              body += '<div class="tool-result"><div class="detail-label">result</div>' + renderDiff(content) + '</div>';
            } else {
              const filePath = pathFromArgs(args, ['file_path', 'path']);
              body += '<div class="tool-result"><div class="detail-label">result' +
                (isError ? ' (error)' : '') + '</div>' +
                formatExpandableOutput(content, 16, isError ? null : getLanguageFromPath(filePath)) + '</div>';
            }
          }
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

      // Turn-group tracking: an assistant entry closes the group opened by
      // the nearest preceding user prompt. Global flag set while rendering.
      let turnGroupOpen = false;

      function entryBelongsToTurnGroup(entry) {
        if (turnGroupOpen) {
          turnGroupOpen = false;
          return true;
        }
        return false;
      }

      function renderEntry(entry) {
        const ts = formatTimestamp(entry.timestamp);
        const tsHtml = ts ? '<div class="message-timestamp">' + escapeHtml(ts) + '</div>' : '';
        const entryDomId = 'entry-' + escapeHtml(entry.id);

        if (entry.type === 'user') {
          const roleTag = entry.display_role
            ? '<div class="display-role-tag">[' + escapeHtml(entry.display_role) + ']</div>' : '';
          // Turn group: this user prompt plus the assistant response that
          // follows it collapse together (Collapse turns, Shift+C).
          turnGroupOpen = true;
          return '<div class="turn-group" id="turn-' + escapeHtml(entry.id) + '">' +
            '<div class="turn-toggle" onclick="if(window.getSelection().toString())return;this.parentElement.classList.toggle(\'collapsed-turn\')" title="Collapse/expand this turn (Shift+C toggles all)">&#9662;</div>' +
            '<div class="user-message" id="' + entryDomId + '" data-user-prompt>' + roleTag + tsHtml +
            '<div class="markdown-content">' + safeMarkedParse(entry.text) + '</div></div>';
        }

        if (entry.type === 'assistant') {
          let html = '<div class="assistant-message" id="' + entryDomId + '">' + tsHtml;
          for (const thinking of entry.thinking) {
            // Collapsed by default: thinking is context, not the answer.
            html += '<div class="thinking-block collapsed" onclick="if(window.getSelection().toString())return;this.classList.toggle(\'collapsed\')">' +
              '<div class="thinking-label">&#9654; thinking</div>' +
              '<div class="thinking-text">' + escapeHtml(thinking) + '</div>' +
              '</div>';
          }
          if (entry.text && entry.text.trim()) {
            html += '<div class="assistant-text markdown-content">' + safeMarkedParse(entry.text) + '</div>';
          }
          for (const call of entry.tool_calls) {
            html += renderToolCall(call);
          }
          html += '</div>';
          // Close the turn group opened by the preceding user prompt (if any).
          if (entryBelongsToTurnGroup(entry)) { html += '</div>'; }
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
      // GANTT: timeline of messages + tool calls
      // ============================================================

      function computeTimeline() {
        const rows = [];
        for (const entry of entries) {
          const t = parseTs(entry.timestamp);
          if (!t || isNaN(t.getTime())) continue;
          const time = t.getTime();
          let end = null;
          if (entry.type === 'tool_result' && entry.duration_ms != null) {
            end = time + entry.duration_ms;
          }
          rows.push({
            entry,
            time,
            end,
            label: entryLabel(entry),
            kind: entryKind(entry),
            sessionId: entry.session_id || null
          });
        }
        return rows;
      }

      function entryLabel(entry) {
        switch (entry.type) {
          case 'user': return truncate(entry.text.replace(/\s+/g, ' ').trim(), 64) || 'user';
          case 'assistant': {
            const text = entry.text.replace(/\s+/g, ' ').trim();
            if (text) return truncate(text, 64);
            if (entry.tool_calls.length) return entry.tool_calls.map(c => c.name).join(', ').slice(0, 64);
            return 'assistant';
          }
          case 'tool_result': {
            const call = findToolCall(entry.tool_use_id);
            return (call ? call.name : 'tool') + (entry.duration_ms != null ? ' ' + formatDuration(entry.duration_ms) : '');
          }
          case 'compaction': return 'compaction';
          default: return '';
        }
      }

      function entryKind(entry) {
        if (entry.type === 'user') return 'user';
        if (entry.type === 'assistant') return entry.text.trim() ? 'assistant' : 'tool';
        if (entry.type === 'tool_result') return entry.is_error ? 'error' : 'tool';
        return 'meta';
      }

      function findToolCall(callId) {
        for (const entry of entries) {
          if (entry.type === 'assistant') {
            for (const call of entry.tool_calls) {
              if (call.id === callId) return call;
            }
          }
        }
        return null;
      }

      function sessionLabel(meta) {
        if (!meta) return '';
        return meta.custom_title || meta.title || meta.short_name || meta.id;
      }

      function renderGantt() {
        const container = document.getElementById('gantt-container');
        if (!container) return;
        const rows = computeTimeline();
        if (rows.length < 3) {
          container.closest('.gantt').style.display = 'none';
          return;
        }
        const start = rows[0].time;
        const end = Math.max(rows[rows.length - 1].time, ...rows.map(r => r.end || 0));
        const span = Math.max(end - start, 1);
        const width = 100; // percent-based rows

        let html = '<div class="gantt-header"><span>' + escapeHtml(formatTimestamp(rows[0].entry.timestamp)) +
          '</span><span>' + escapeHtml(formatSpan(span)) + '</span><span>' +
          escapeHtml(formatTimestamp(new Date(end).toISOString())) + '</span></div>';
        html += '<div class="gantt-body">';

        // Multi-session export: one labeled lane per session (subagents).
        if (sessions && sessions.length > 1) {
          const metaById = new Map(sessions.map(s => [s.id, s]));
          for (const session of sessions) {
            const laneRows = rows.filter(r => (r.sessionId || sessions[0].id) === session.id);
            if (!laneRows.length) continue;
            html += '<div class="gantt-lane-label">' +
              (session.is_primary ? '&#9733; ' : '&#8901; ') +
              escapeHtml(sessionLabel(session)) +
              (session.model ? ' <span class="muted">(' + escapeHtml(session.model) + ')</span>' : '') +
              '</div>';
            for (const row of laneRows) {
              const left = ((row.time - start) / span) * width;
              const widthPct = row.end != null ? Math.max(((row.end - row.time) / span) * width, 0.4) : 0.6;
              html += '<div class="gantt-row" data-target="entry-' + escapeHtml(row.entry.id) + '" title="' + escapeHtml(row.label) + '">' +
                '<div class="gantt-bar gantt-' + row.kind + '" style="left:' + left.toFixed(2) + '%;width:' + widthPct.toFixed(2) + '%"></div>' +
                '<div class="gantt-label">' + escapeHtml(row.label) + '</div>' +
                '</div>';
            }
          }
        } else {
          for (const row of rows) {
            const left = ((row.time - start) / span) * width;
            const widthPct = row.end != null ? Math.max(((row.end - row.time) / span) * width, 0.4) : 0.6;
            html += '<div class="gantt-row" data-target="entry-' + escapeHtml(row.entry.id) + '" title="' + escapeHtml(row.label) + '">' +
              '<div class="gantt-bar gantt-' + row.kind + '" style="left:' + left.toFixed(2) + '%;width:' + widthPct.toFixed(2) + '%"></div>' +
              '<div class="gantt-label">' + escapeHtml(row.label) + '</div>' +
              '</div>';
          }
        }
        html += '</div>';
        container.innerHTML = html;
        container.querySelectorAll('.gantt-row').forEach(node => {
          node.addEventListener('click', () => {
            const target = document.getElementById(node.dataset.target);
            if (target) {
              target.scrollIntoView({ behavior: 'smooth', block: 'start' });
              target.classList.add('flash');
              setTimeout(() => target.classList.remove('flash'), 1600);
            }
            if (window.innerWidth <= 900) closeSidebar();
          });
        });
      }

      // ============================================================
      // SIDEBAR TREE
      // ============================================================

      let filterMode = 'default';
      let searchQuery = '';

      function treeNodeText(entry) {
        switch (entry.type) {
          case 'user':
            return { role: 'user', text: truncate(entry.text.replace(/[\n\t]/g, ' ').trim()) };
          case 'assistant': {
            const text = entry.text.replace(/[\n\t]/g, ' ').trim();
            if (text) return { role: 'assistant', text: truncate(text) };
            if (entry.tool_calls.length > 0) {
              const names = entry.tool_calls.map(c => c.name);
              const uniq = Array.from(new Set(names));
              const count = names.length === uniq.length ? '' : ' x' + names.length;
              return { role: 'tool', text: truncate(uniq.join(', ') + count, 90) };
            }
            return { role: 'assistant', text: '(no text)' };
          }
          case 'tool_result': {
            const call = findToolCall(entry.tool_use_id);
            return { role: 'tool', text: truncate((call ? call.name : 'tool') + ' result', 90) };
          }
          case 'compaction':
            return { role: 'compaction', text: 'compaction summary' };
          default:
            return { role: 'tool', text: '' };
        }
      }

      function filterEntries(list) {
        const tokens = searchQuery.toLowerCase().split(/\s+/).filter(Boolean);
        return list.filter(entry => {
          if (filterMode === 'user-only') {
            if (entry.type !== 'user') return false;
          } else if (filterMode === 'default') {
            if (entry.type === 'assistant' && !entry.text.trim() && !entry.thinking.length) return false;
            if (entry.type === 'tool_result') return false;
          }
          if (tokens.length > 0) {
            const hay = searchableText(entry).toLowerCase();
            if (!tokens.every(t => hay.includes(t))) return false;
          }
          return true;
        });
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

      function renderTree() {
        const container = document.getElementById('tree-container');
        const visible = filterEntries(entries);
        const fragments = [];
        visible.forEach(entry => {
          const info = treeNodeText(entry);
          const roleClass = {
            user: 'tree-role-user',
            assistant: 'tree-role-assistant',
            tool: 'tree-role-tool',
            compaction: 'tree-compaction'
          }[info.role] || 'tree-muted';
          const prefix = info.role === 'user' ? '> ' : info.role === 'assistant' ? '* ' : info.role === 'tool' ? '- ' : '# ';
          const time = formatTimeShort(entry.timestamp);
          fragments.push('<div class="tree-node" data-entry-id="' + escapeHtml(entry.id) + '" data-target="entry-' + escapeHtml(entry.id) + '">' +
            '<span class="tree-time">' + escapeHtml(time) + '</span>' +
            '<span class="tree-prefix">' + prefix + '</span>' +
            '<span class="tree-content"><span class="' + roleClass + '">' + escapeHtml(info.text) + '</span></span></div>');
        });
        container.innerHTML = fragments.join('');
        document.getElementById('tree-status').textContent =
          visible.length + ' / ' + entries.length + ' entries';

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
        // top-5 tools by count, from entries
        const counts = new Map();
        for (const entry of entries) {
          if (entry.type !== 'assistant') continue;
          for (const call of entry.tool_calls) {
            counts.set(call.name, (counts.get(call.name) || 0) + 1);
          }
        }
        return Array.from(counts.entries()).sort((a, b) => b[1] - a[1]).slice(0, 5);
      }

      function renderHeader() {
        const parts = [];
        parts.push(stats.user_messages + ' user');
        parts.push(stats.assistant_messages + ' assistant');
        parts.push(stats.tool_calls + ' tool calls');
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

        let html = '<div class="header">' +
          '<h1>jcode session: ' + escapeHtml(sessionTitle()) + '</h1>' +
          '<div class="muted" style="font-size: 10px; margin-bottom: 8px">Keys: ? help &middot; T thinking &middot; O tool details &middot; G timeline &middot; J/K prompts &middot; Shift+C turns</div>' +
          '<div class="header-actions">' +
          '<button type="button" class="header-toggle-btn" data-action="toggle-thinking" title="Toggle thinking (T)">Thinking</button>' +
          '<button type="button" class="header-toggle-btn" data-action="toggle-tools" title="Expand every tool call (O)">Tool details</button>' +
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

        document.querySelector('[data-action="toggle-thinking"]').addEventListener('click', toggleThinking);
        document.querySelector('[data-action="toggle-tools"]').addEventListener('click', toggleToolOutputs);
        document.querySelector('[data-action="toggle-gantt"]').addEventListener('click', toggleGantt);
        document.querySelector('[data-action="download-json"]').addEventListener('click', downloadSessionJson);
      }

      function downloadSessionJson() {
        const json = JSON.stringify({ header: header, entries: entries, stats: stats }, null, 2);
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
      // TOGGLES + KEYBOARD NAV
      // ============================================================

      let thinkingForced = null; // null = default (collapsed), true/false = forced
      let toolOutputsExpanded = false;
      let ganttVisible = true;

      function toggleThinking() {
        thinkingForced = thinkingForced === null ? true : !thinkingForced;
        document.querySelectorAll('.thinking-block').forEach(el => {
          if (thinkingForced === null) return;
          el.classList.toggle('collapsed', !thinkingForced);
        });
      }

      function toggleToolOutputs() {
        toolOutputsExpanded = !toolOutputsExpanded;
        document.querySelectorAll('.tool-execution').forEach(el => {
          el.classList.toggle('open', toolOutputsExpanded);
        });
      }

      function toggleGantt() {
        ganttVisible = !ganttVisible;
        const gantt = document.querySelector('.gantt');
        if (gantt) gantt.classList.toggle('hidden', !ganttVisible);
      }

      function userPromptIds() {
        return Array.from(document.querySelectorAll('[data-user-prompt]')).map(el => el.id);
      }

      function jumpUserPrompt(direction) {
        const ids = userPromptIds();
        if (!ids.length) return;
        const positions = ids.map(id => {
          const el = document.getElementById(id);
          return { id, top: el.getBoundingClientRect().top };
        });
        const current = positions.find(p => p.top >= 40) ? positions.filter(p => p.top >= -20) : positions;
        const viewportTop = window.scrollY;
        let target = null;
        if (direction > 0) {
          target = positions.find(p => document.getElementById(p.id).getBoundingClientRect().top > 80);
        } else {
          for (let i = positions.length - 1; i >= 0; i--) {
            if (document.getElementById(positions[i].id).getBoundingClientRect().top < -40) {
              target = positions[i];
              break;
            }
          }
          if (!target && positions.length) target = positions[0];
        }
        if (target) jumpTo(target.id);
      }

      // Reading progress + back-to-top
      function setupScrollUi() {
        const progress = document.getElementById('read-progress');
        const topBtn = document.getElementById('back-to-top');
        window.addEventListener('scroll', () => {
          const doc = document.documentElement;
          const max = doc.scrollHeight - window.innerHeight;
          const pct = max > 0 ? (window.scrollY / max) * 100 : 0;
          progress.style.width = pct.toFixed(1) + '%';
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
        let startX = 0;
        let startWidth = 0;

        const onMove = (e) => {
          const width = Math.min(720, Math.max(240, startWidth + (e.clientX - startX)));
          sidebar.style.width = width + 'px';
          sidebar.style.minWidth = width + 'px';
          sidebar.style.maxWidth = width + 'px';
        };
        const onUp = () => {
          document.body.classList.remove('sidebar-resizing');
          window.removeEventListener('pointermove', onMove);
          window.removeEventListener('pointerup', onUp);
          const match = sidebar.style.width.match(/^(\d+)px$/);
          if (match) {
            try { localStorage.setItem(WIDTH_KEY, match[1]); } catch (e) { /* private mode */ }
          }
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
          const saved = localStorage.getItem(WIDTH_KEY);
          const width = parseInt(saved, 10);
          if (width >= 240 && width <= 720) {
            sidebar.style.width = width + 'px';
            sidebar.style.minWidth = width + 'px';
            sidebar.style.maxWidth = width + 'px';
          }
        } catch (e) { /* ignore */ }
      }

      function closeSidebar() {
        document.getElementById('sidebar').classList.remove('open');
        document.getElementById('sidebar-overlay').classList.remove('open');
      }

      // ============================================================
      // INIT
      // ============================================================

      const searchInput = document.getElementById('tree-search');
      searchInput.addEventListener('input', (e) => {
        searchQuery = e.target.value;
        renderTree();
      });

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
        return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || tag === 'BUTTON' ||
          element.isContentEditable;
      };

      document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') {
          searchInput.value = '';
          searchQuery = '';
          renderTree();
        }
        if (isEditableTarget(document.activeElement)) return;
        const key = e.key.toLowerCase();
        if (key === '?' || (e.shiftKey && key === '/')) {
          e.preventDefault();
          toggleHelpOverlay();
        }
        else if (e.shiftKey && key === 'c') {
          e.preventDefault();
          toggleAllTurns();
        }
        else if (key === 't') { e.preventDefault(); toggleThinking(); }
        else if (key === 'o') { e.preventDefault(); toggleToolOutputs(); }
        else if (key === 'g') { e.preventDefault(); toggleGantt(); }
        else if (key === 'j' || e.key === 'PageDown') { e.preventDefault(); jumpUserPrompt(1); }
        else if (key === 'k' || e.key === 'PageUp') { e.preventDefault(); jumpUserPrompt(-1); }
      });

      function toggleHelpOverlay() {
        let overlay = document.getElementById('help-overlay');
        if (overlay) {
          overlay.remove();
          return;
        }
        overlay = document.createElement('div');
        overlay.id = 'help-overlay';
        overlay.innerHTML =
          '<div class="help-card"><h2>Keyboard</h2><table>' +
          '<tr><td>?</td><td>this help</td></tr>' +
          '<tr><td>T</td><td>show/hide thinking</td></tr>' +
          '<tr><td>O</td><td>expand/collapse all tool details</td></tr>' +
          '<tr><td>G</td><td>show/hide timeline</td></tr>' +
          '<tr><td>J / K</td><td>next / previous user prompt</td></tr>' +
          '<tr><td>Shift+C</td><td>collapse/expand all turns</td></tr>' +
          '<tr><td>Esc</td><td>clear search / close help</td></tr>' +
          '</table><p>Click a tool row to expand its arguments and result. Click a sidebar or timeline row to jump with a highlight.</p></div>' +
          '<div class="help-backdrop"></div>';
        overlay.querySelector('.help-backdrop').addEventListener('click', () => overlay.remove());
        document.body.appendChild(overlay);
      }

      function toggleAllTurns() {
        const groups = document.querySelectorAll('.turn-group');
        if (!groups.length) return;
        // If any group is expanded, collapse all; otherwise expand all.
        const anyExpanded = Array.from(groups).some(g => !g.classList.contains('collapsed-turn'));
        groups.forEach(g => g.classList.toggle('collapsed-turn', anyExpanded));
      }

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

      // Deep link: #entry-mN
      if (window.location.hash && window.location.hash.startsWith('#entry-')) {
        const domId = window.location.hash.slice(1);
        setTimeout(() => jumpTo(domId), 100);
      }
    })();
