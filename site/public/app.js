(function () {
  'use strict';

  var STORAGE_KEY = 'devbase-theme';
  var root = document.documentElement;
  var metaTheme = document.getElementById('meta-theme-color');

  /* ---------- 1. Theme Sync (Header Toggle + Footer Pill) ---------- */

  function applyTheme(theme, persist) {
    var isDark = theme === 'dark';
    root.classList.toggle('dark', isDark);
    if (metaTheme) {
      metaTheme.setAttribute('content', isDark ? '#0a0a0d' : '#fafaf8');
    }
    var pillBtns = document.querySelectorAll('[data-set-theme]');
    pillBtns.forEach(function (btn) {
      btn.setAttribute('aria-pressed', String(btn.getAttribute('data-set-theme') === (isDark ? 'dark' : 'light')));
    });
    if (persist) {
      try {
        localStorage.setItem(STORAGE_KEY, isDark ? 'dark' : 'light');
      } catch (_) {}
    }
  }

  applyTheme(root.classList.contains('dark') ? 'dark' : 'light', false);

  var headerThemeBtn = document.getElementById('theme-toggle');
  if (headerThemeBtn) {
    headerThemeBtn.addEventListener('click', function () {
      applyTheme(root.classList.contains('dark') ? 'light' : 'dark', true);
    });
  }

  document.querySelectorAll('[data-set-theme]').forEach(function (btn) {
    btn.addEventListener('click', function () {
      applyTheme(btn.getAttribute('data-set-theme'), true);
    });
  });

  /* ---------- 2. Sticky Frosted Header & Mobile Drawer ---------- */

  var nav = document.getElementById('nav');
  function onScroll() {
    if (!nav) return;
    nav.setAttribute('data-scrolled', window.scrollY > 12 ? 'true' : 'false');
  }
  window.addEventListener('scroll', onScroll, { passive: true });
  onScroll();

  var mobileBtn = document.getElementById('mobile-menu-btn');
  var mobileMenu = document.getElementById('mobile-menu');
  if (mobileBtn && mobileMenu) {
    mobileBtn.addEventListener('click', function () {
      var open = mobileMenu.classList.toggle('open');
      mobileBtn.setAttribute('aria-expanded', String(open));
    });
    mobileMenu.querySelectorAll('a').forEach(function (link) {
      link.addEventListener('click', function () {
        mobileMenu.classList.remove('open');
        mobileBtn.setAttribute('aria-expanded', 'false');
      });
    });
  }

  /* ---------- 3. Clipboard Helper ---------- */

  function copyText(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(text).catch(function () {
        return fallbackCopy(text);
      });
    }
    return Promise.resolve(fallbackCopy(text));
  }

  function fallbackCopy(text) {
    try {
      var ta = document.createElement('textarea');
      ta.value = text;
      ta.setAttribute('readonly', '');
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      document.body.removeChild(ta);
    } catch (_) {}
  }

  /* ---------- 4. Interactive macOS 26 Demo Frame ---------- */

  var demoState = {
    'pg-main': true,
    'redis-cache': true,
    'mysql-analytics': false,
    'https-proxy': true,
    'cf-tunnel': true
  };

  var serviceMeta = {
    'pg-main': { kind: 'db', label: 'db-pg-main', stream: 'db', onBadge: 'Running', offBadge: 'Stopped' },
    'redis-cache': { kind: 'db', label: 'db-redis-cache', stream: 'db', onBadge: 'Running', offBadge: 'Stopped' },
    'mysql-analytics': { kind: 'db', label: 'db-mysql-analytics', stream: 'db', onBadge: 'Running', offBadge: 'Stopped' },
    'https-proxy': { kind: 'net', label: 'proxy', stream: 'proxy', onBadge: 'Listening :443', offBadge: 'Stopped' },
    'cf-tunnel': { kind: 'net', label: 'tunnel', stream: 'tunnel', onBadge: 'Tunnel connected', offBadge: 'Disconnected' }
  };

  var viewTitles = {
    software: 'Software',
    databases: 'Databases',
    https: 'HTTPS & Tunnels',
    services: 'All Items — Services Inspector'
  };

  var toastEl = document.getElementById('demo-toast');
  var toastTimer = null;

  function showDemoToast(message) {
    if (!toastEl) return;
    toastEl.textContent = message;
    toastEl.classList.add('show');
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(function () {
      toastEl.classList.remove('show');
    }, 2400);
  }

  var logLinesEl = document.getElementById('demo-log-lines');
  var activeLogFilter = 'all';

  function formatNow() {
    var d = new Date();
    var hh = String(d.getHours()).padStart(2, '0');
    var mm = String(d.getMinutes()).padStart(2, '0');
    var ss = String(d.getSeconds()).padStart(2, '0');
    return hh + ':' + mm + ':' + ss;
  }

  function appendDemoLog(stream, source, message, highlight) {
    if (!logLinesEl) return;
    var row = document.createElement('div');
    row.className = 'log-line';
    row.setAttribute('data-stream', stream);

    var tsSpan = document.createElement('span');
    tsSpan.className = 'log-ts';
    tsSpan.textContent = formatNow();

    var srcSpan = document.createElement('span');
    srcSpan.className = 'log-src';
    srcSpan.textContent = '[' + source + ']';

    var msgSpan = document.createElement('span');
    if (highlight) msgSpan.className = 'log-ok';
    msgSpan.textContent = message;

    row.appendChild(tsSpan);
    row.appendChild(srcSpan);
    row.appendChild(msgSpan);

    if (activeLogFilter !== 'all' && activeLogFilter !== stream) {
      row.style.display = 'none';
    }

    logLinesEl.appendChild(row);
    while (logLinesEl.children.length > 18) {
      logLinesEl.removeChild(logLinesEl.firstChild);
    }
    logLinesEl.scrollTop = logLinesEl.scrollHeight;
  }

  function syncDemoUI() {
    var dbRunning = 0;
    var totalActive = 0;

    Object.keys(demoState).forEach(function (id) {
      var on = !!demoState[id];
      var meta = serviceMeta[id];
      if (on) {
        totalActive += 1;
        if (meta && meta.kind === 'db') dbRunning += 1;
      }

      document.querySelectorAll('[data-toggle-service="' + id + '"]').forEach(function (sw) {
        sw.setAttribute('aria-checked', String(on));
      });

      document.querySelectorAll('[data-status-badge="' + id + '"]').forEach(function (badge) {
        badge.textContent = on ? meta.onBadge : meta.offBadge;
        badge.classList.toggle('status-chip-green', on);
        badge.classList.toggle('status-chip-muted', !on);
      });
    });

    var dbCountEl = document.getElementById('sidebar-db-count');
    if (dbCountEl) dbCountEl.textContent = String(dbRunning);

    var httpsPill = document.getElementById('sidebar-https-pill');
    if (httpsPill) httpsPill.textContent = demoState['https-proxy'] ? 'ON' : 'OFF';

    var mbLabel = document.getElementById('mb-running-label');
    if (mbLabel) mbLabel.textContent = totalActive + ' running';

    var mbPopCount = document.getElementById('mb-pop-count');
    if (mbPopCount) {
      mbPopCount.textContent = totalActive + ' active';
      mbPopCount.classList.toggle('status-chip-green', totalActive > 0);
      mbPopCount.classList.toggle('status-chip-muted', totalActive === 0);
    }

    var mbDot = document.getElementById('mb-status-dot');
    if (mbDot) {
      mbDot.style.background = totalActive > 0 ? '#22c55e' : '#71717a';
    }
  }

  document.querySelectorAll('[data-toggle-service]').forEach(function (sw) {
    sw.addEventListener('click', function () {
      var id = sw.getAttribute('data-toggle-service');
      if (!Object.prototype.hasOwnProperty.call(demoState, id)) return;
      demoState[id] = !demoState[id];
      syncDemoUI();
      var meta = serviceMeta[id];
      var verb = demoState[id] ? 'started' : 'stopped';
      appendDemoLog(meta.stream, meta.label, id + ' ' + verb + ' cleanly', demoState[id]);
      showDemoToast(id + ' ' + verb);
    });
  });

  syncDemoUI();

  /* Sidebar tab switching inside Demo */
  function selectAppTab(tabKey) {
    document.querySelectorAll('[data-app-tab]').forEach(function (btn) {
      btn.setAttribute('aria-selected', String(btn.getAttribute('data-app-tab') === tabKey));
    });
    document.querySelectorAll('.app-view').forEach(function (view) {
      view.classList.toggle('active', view.id === 'app-view-' + tabKey);
    });
    var titleEl = document.getElementById('ws-view-title');
    if (titleEl && viewTitles[tabKey]) {
      titleEl.textContent = viewTitles[tabKey];
    }
  }

  document.querySelectorAll('[data-app-tab]').forEach(function (btn) {
    btn.addEventListener('click', function () {
      selectAppTab(btn.getAttribute('data-app-tab'));
    });
  });

  /* Menu Bar Popover Toggle */
  var mbTrigger = document.getElementById('mb-popover-trigger');
  var mbPopover = document.getElementById('mb-popover');

  function setPopoverOpen(open) {
    if (!mbTrigger || !mbPopover) return;
    mbPopover.classList.toggle('open', open);
    mbTrigger.setAttribute('aria-expanded', String(open));
  }

  if (mbTrigger && mbPopover) {
    mbTrigger.addEventListener('click', function (e) {
      e.stopPropagation();
      setPopoverOpen(!mbPopover.classList.contains('open'));
    });

    document.addEventListener('click', function (e) {
      if (mbPopover.classList.contains('open') && !mbPopover.contains(e.target) && e.target !== mbTrigger) {
        setPopoverOpen(false);
      }
    });
  }

  var mbPopClose = document.getElementById('mb-pop-close');
  if (mbPopClose) {
    mbPopClose.addEventListener('click', function () {
      setPopoverOpen(false);
    });
  }

  var mbStopAll = document.getElementById('mb-stop-all');
  if (mbStopAll) {
    mbStopAll.addEventListener('click', function () {
      demoState['pg-main'] = false;
      demoState['redis-cache'] = false;
      demoState['mysql-analytics'] = false;
      syncDemoUI();
      appendDemoLog('db', 'devbase', 'Stopped all managed database instances (external data retained)', false);
      showDemoToast('Stopped all managed databases');
    });
  }

  var mbCopyUrl = document.getElementById('mb-pop-copy-url');
  if (mbCopyUrl) {
    mbCopyUrl.addEventListener('click', function () {
      copyText('https://api.devbase.localhost');
      showDemoToast('Copied https://api.devbase.localhost');
      setPopoverOpen(false);
    });
  }

  /* Live Log Drawer Toggle & Stream Filtering */
  var logsBtn = document.getElementById('demo-logs-btn');
  var logsDrawer = document.getElementById('demo-log-drawer');
  var logsClose = document.getElementById('demo-logs-close');
  var mbPopLogs = document.getElementById('mb-pop-logs');

  function setLogsOpen(open) {
    if (!logsDrawer || !logsBtn) return;
    logsDrawer.classList.toggle('open', open);
    logsBtn.setAttribute('aria-pressed', String(open));
  }

  if (logsBtn) {
    logsBtn.addEventListener('click', function () {
      setLogsOpen(!logsDrawer.classList.contains('open'));
    });
  }
  if (logsClose) {
    logsClose.addEventListener('click', function () {
      setLogsOpen(false);
    });
  }
  if (mbPopLogs) {
    mbPopLogs.addEventListener('click', function () {
      setPopoverOpen(false);
      setLogsOpen(true);
    });
  }

  document.querySelectorAll('[data-log-filter]').forEach(function (tab) {
    tab.addEventListener('click', function () {
      activeLogFilter = tab.getAttribute('data-log-filter');
      document.querySelectorAll('[data-log-filter]').forEach(function (t) {
        t.classList.toggle('active', t === tab);
      });
      if (!logLinesEl) return;
      Array.prototype.forEach.call(logLinesEl.children, function (line) {
        var stream = line.getAttribute('data-stream');
        line.style.display = activeLogFilter === 'all' || activeLogFilter === stream ? '' : 'none';
      });
    });
  });

  /* Command Palette (⌘K) inside Simulated App */
  var cmdkBackdrop = document.getElementById('demo-cmdk');
  var cmdkBtn = document.getElementById('demo-cmdk-btn');
  var cmdkClose = document.getElementById('demo-cmdk-close');
  var cmdkInput = document.getElementById('demo-cmdk-input');
  var cmdkList = document.getElementById('demo-cmdk-list');

  function setCmdkOpen(open) {
    if (!cmdkBackdrop) return;
    cmdkBackdrop.classList.toggle('open', open);
    if (open && cmdkInput) {
      cmdkInput.value = '';
      filterCmdk('');
      cmdkInput.focus();
    }
  }

  function filterCmdk(query) {
    if (!cmdkList) return;
    var q = query.trim().toLowerCase();
    cmdkList.querySelectorAll('.cmdk-item').forEach(function (item) {
      var text = item.textContent.toLowerCase();
      item.style.display = !q || text.indexOf(q) !== -1 ? '' : 'none';
    });
  }

  if (cmdkBtn) {
    cmdkBtn.addEventListener('click', function () {
      setCmdkOpen(true);
    });
  }
  if (cmdkClose) {
    cmdkClose.addEventListener('click', function () {
      setCmdkOpen(false);
    });
  }
  if (cmdkBackdrop) {
    cmdkBackdrop.addEventListener('click', function (e) {
      if (e.target === cmdkBackdrop) setCmdkOpen(false);
    });
  }
  if (cmdkInput) {
    cmdkInput.addEventListener('input', function () {
      filterCmdk(cmdkInput.value);
    });
  }
  if (cmdkList) {
    cmdkList.querySelectorAll('[data-cmd-action]').forEach(function (item) {
      item.addEventListener('click', function () {
        var action = item.getAttribute('data-cmd-action');
        setCmdkOpen(false);
        if (action.indexOf('tab:') === 0) {
          selectAppTab(action.slice(4));
        } else if (action === 'toggle-popover') {
          setPopoverOpen(!mbPopover.classList.contains('open'));
        } else if (action === 'toggle-logs') {
          setLogsOpen(!logsDrawer.classList.contains('open'));
        }
      });
    });
  }

  /* QR Panel Toggle inside HTTPS View */
  var qrBtn = document.getElementById('demo-qr-btn');
  var qrPanel = document.getElementById('demo-qr-panel');
  if (qrBtn && qrPanel) {
    qrBtn.addEventListener('click', function () {
      var open = qrPanel.classList.toggle('open');
      qrBtn.setAttribute('aria-expanded', String(open));
    });
  }

  /* Shell Hook Cycler inside Software View */
  var shells = ['zsh', 'bash', 'fish'];
  document.querySelectorAll('[data-shell-cycle]').forEach(function (btn) {
    btn.addEventListener('click', function () {
      var current = btn.getAttribute('data-shell-cycle') || 'zsh';
      var next = shells[(shells.indexOf(current) + 1) % shells.length];
      btn.setAttribute('data-shell-cycle', next);
      btn.textContent = 'Shell: ' + next;
      showDemoToast('Reversible ' + next + ' integration + NODE_EXTRA_CA_CERTS active');
    });
  });

  /* Demo Copy-Toast & Backup Buttons */
  document.querySelectorAll('[data-copy-toast]').forEach(function (btn) {
    btn.addEventListener('click', function () {
      var msg = btn.getAttribute('data-copy-toast');
      showDemoToast(msg);
    });
  });

  document.querySelectorAll('[data-backup-db]').forEach(function (btn) {
    btn.addEventListener('click', function () {
      var db = btn.getAttribute('data-backup-db');
      appendDemoLog('db', 'db-' + db, 'Created snapshot backup ' + db + '-20261121.sql.gz', true);
      showDemoToast('Backup saved for ' + db);
    });
  });

  /* Services 5-Source Live Filter */
  var svcSearch = document.getElementById('demo-svc-search');
  var svcSource = 'all';

  function filterServices() {
    var q = svcSearch ? svcSearch.value.trim().toLowerCase() : '';
    document.querySelectorAll('[data-svc-item]').forEach(function (row) {
      var src = row.getAttribute('data-source');
      var name = (row.getAttribute('data-name') || '').toLowerCase();
      var matchSource = svcSource === 'all' || src === svcSource;
      var matchQuery = !q || name.indexOf(q) !== -1;
      row.style.display = matchSource && matchQuery ? '' : 'none';
    });
  }

  if (svcSearch) {
    svcSearch.addEventListener('input', filterServices);
  }
  document.querySelectorAll('[data-svc-source]').forEach(function (pill) {
    pill.addEventListener('click', function () {
      svcSource = pill.getAttribute('data-svc-source');
      document.querySelectorAll('[data-svc-source]').forEach(function (p) {
        p.classList.toggle('active', p === pill);
      });
      filterServices();
    });
  });

  /* Global Keyboard Shortcuts (Escape, ⌘K, ⌃⌥⌘D) */
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape') {
      setCmdkOpen(false);
      setPopoverOpen(false);
      if (mobileMenu && mobileMenu.classList.contains('open')) {
        mobileMenu.classList.remove('open');
        if (mobileBtn) mobileBtn.setAttribute('aria-expanded', 'false');
      }
      return;
    }
    if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      var isOpen = cmdkBackdrop && cmdkBackdrop.classList.contains('open');
      setCmdkOpen(!isOpen);
      return;
    }
    if (e.ctrlKey && e.altKey && e.key.toLowerCase() === 'd') {
      e.preventDefault();
      setPopoverOpen(!(mbPopover && mbPopover.classList.contains('open')));
    }
  });

  /* ---------- 5. Four Pillars Segmented Tabs (#features) ---------- */

  document.querySelectorAll('[data-pillar-tab]').forEach(function (tab) {
    tab.addEventListener('click', function () {
      var key = tab.getAttribute('data-pillar-tab');
      document.querySelectorAll('[data-pillar-tab]').forEach(function (t) {
        t.setAttribute('aria-selected', String(t === tab));
      });
      document.querySelectorAll('.pillar-panel').forEach(function (panel) {
        panel.classList.toggle('active', panel.id === 'pillar-' + key);
      });
    });
  });

  /* ---------- 6. CLI & MCP Interactive Terminal (#cli) ---------- */

  var cliCopyBtn = document.getElementById('cli-copy-btn');
  var activeTermKey = 'mcp';

  document.querySelectorAll('[data-term-tab]').forEach(function (tab) {
    tab.addEventListener('click', function () {
      activeTermKey = tab.getAttribute('data-term-tab');
      document.querySelectorAll('[data-term-tab]').forEach(function (t) {
        t.setAttribute('aria-selected', String(t === tab));
      });
      document.querySelectorAll('.terminal-pane').forEach(function (pane) {
        pane.classList.toggle('active', pane.id === 'term-' + activeTermKey);
      });
      if (cliCopyBtn) {
        cliCopyBtn.classList.remove('copied');
        var lbl = cliCopyBtn.querySelector('span');
        if (lbl) lbl.textContent = 'Copy';
      }
    });
  });

  if (cliCopyBtn) {
    cliCopyBtn.addEventListener('click', function () {
      var activePane = document.getElementById('term-' + activeTermKey);
      if (!activePane) return;
      var text = activePane.getAttribute('data-copy-text') || activePane.textContent;
      copyText(text).then(function () {
        cliCopyBtn.classList.add('copied');
        var lbl = cliCopyBtn.querySelector('span');
        if (lbl) lbl.textContent = 'Copied';
        setTimeout(function () {
          cliCopyBtn.classList.remove('copied');
          if (lbl) lbl.textContent = 'Copy';
        }, 1800);
      });
    });
  }

  /* ---------- 7. Split Download Card Switcher (#get) ---------- */

  document.querySelectorAll('[data-install-tab]').forEach(function (btn) {
    btn.addEventListener('click', function () {
      var mode = btn.getAttribute('data-install-tab');
      document.querySelectorAll('[data-install-tab]').forEach(function (b) {
        b.setAttribute('aria-selected', String(b === btn));
      });
      document.querySelectorAll('.split-mode').forEach(function (panel) {
        panel.classList.toggle('active', panel.id === 'install-' + mode);
      });
    });
  });

  var copyInstallBtn = document.getElementById('copy-install-snippet');
  var installCode = document.getElementById('install-snippet-code');
  if (copyInstallBtn && installCode) {
    copyInstallBtn.addEventListener('click', function () {
      copyText(installCode.textContent.trim()).then(function () {
        var orig = copyInstallBtn.textContent;
        copyInstallBtn.textContent = 'Copied to clipboard';
        setTimeout(function () {
          copyInstallBtn.textContent = orig;
        }, 1800);
      });
    });
  }
})();
