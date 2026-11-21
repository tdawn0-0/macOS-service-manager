(() => {
  'use strict';

  const $ = (id) => document.getElementById(id);
  const $$ = (sel) => document.querySelectorAll(sel);
  const root = document.documentElement;
  const metaTheme = $('meta-theme-color');

  /* Theme */
  const applyTheme = (theme, persist) => {
    const dark = theme === 'dark';
    root.classList.toggle('dark', dark);
    metaTheme?.setAttribute('content', dark ? '#09090b' : '#fcfcfb');
    $$('[data-set-theme]').forEach((b) =>
      b.setAttribute('aria-pressed', String(b.dataset.setTheme === (dark ? 'dark' : 'light')))
    );
    if (persist) {
      try { localStorage.setItem('devbase-theme', dark ? 'dark' : 'light'); } catch (_) {}
    }
  };

  applyTheme(root.classList.contains('dark') ? 'dark' : 'light', false);
  $('theme-toggle')?.addEventListener('click', () => applyTheme(root.classList.contains('dark') ? 'light' : 'dark', true));
  $$('[data-set-theme]').forEach((b) => b.addEventListener('click', () => applyTheme(b.dataset.setTheme, true)));

  if (window.matchMedia) {
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const onSystemChange = (e) => {
      try {
        if (!localStorage.getItem('devbase-theme')) applyTheme(e.matches ? 'dark' : 'light', false);
      } catch (_) {}
    };
    if (mq.addEventListener) mq.addEventListener('change', onSystemChange);
  }

  /* Header & Mobile Drawer */
  const nav = $('nav');
  const onScroll = () => nav?.setAttribute('data-scrolled', String(window.scrollY > 12));
  window.addEventListener('scroll', onScroll, { passive: true });
  onScroll();

  const mobileBtn = $('mobile-menu-btn');
  const mobileMenu = $('mobile-menu');
  const closeMobile = () => {
    mobileMenu?.classList.remove('open');
    mobileBtn?.setAttribute('aria-expanded', 'false');
  };
  mobileBtn?.addEventListener('click', () => {
    const open = mobileMenu.classList.toggle('open');
    mobileBtn.setAttribute('aria-expanded', String(open));
  });
  mobileMenu?.querySelectorAll('a').forEach((a) => a.addEventListener('click', closeMobile));

  /* Clipboard */
  const fallbackCopy = (text) => {
    try {
      const ta = Object.assign(document.createElement('textarea'), { value: text });
      ta.setAttribute('readonly', '');
      ta.style.cssText = 'position:fixed;opacity:0;pointer-events:none';
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      ta.remove();
    } catch (_) {}
  };

  const copyText = (text) => {
    if (navigator.clipboard?.writeText) {
      return navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
    }
    fallbackCopy(text);
    return Promise.resolve();
  };

  /* Interactive Desktop Preview */
  const state = {
    'pg-main': true,
    'redis-cache': true,
    'mysql-analytics': false,
    'https-proxy': true,
    'cf-tunnel': true
  };

  const meta = {
    'pg-main': ['db', 'pg-main', 'db'],
    'redis-cache': ['db', 'redis-cache', 'db'],
    'mysql-analytics': ['db', 'mysql-analytics', 'db'],
    'https-proxy': ['net', 'proxy', 'proxy'],
    'cf-tunnel': ['net', 'tunnel', 'tunnel']
  };

  const titles = { software: 'Software', databases: 'Databases', https: 'HTTPS', services: 'Services' };
  let activeTab = 'databases';

  let toastTimer = null;
  const toastEl = $('demo-toast');
  const showToast = (msg) => {
    if (!toastEl) return;
    toastEl.textContent = msg;
    toastEl.classList.add('show');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toastEl.classList.remove('show'), 2200);
  };

  const logLines = $('demo-log-lines');
  let logFilter = 'all';

  const appendLog = (stream, src, msg, ok) => {
    if (!logLines) return;
    const now = new Date();
    const ts = [now.getHours(), now.getMinutes(), now.getSeconds()].map((n) => String(n).padStart(2, '0')).join(':');
    const row = document.createElement('div');
    row.className = 'log-line';
    row.dataset.stream = stream;
    if (logFilter !== 'all' && logFilter !== stream) row.style.display = 'none';
    const mk = (cls, txt) => Object.assign(document.createElement('span'), { className: cls, textContent: txt });
    row.append(mk('log-ts', ts), mk('log-src', `[${src}]`), mk(ok ? 'log-ok' : '', msg));
    logLines.append(row);
    while (logLines.children.length > 16) logLines.firstChild.remove();
    logLines.scrollTop = logLines.scrollHeight;
  };

  const updateViewMeta = () => {
    const el = $('ws-view-meta');
    if (!el) return;
    const dbRunning = ['pg-main', 'redis-cache', 'mysql-analytics'].filter((k) => state[k]).length;
    if (activeTab === 'databases') el.textContent = `${dbRunning} of 3 running`;
    else if (activeTab === 'software') el.textContent = '3 runtimes · arm64 & x86_64';
    else if (activeTab === 'https') el.textContent = state['https-proxy'] ? 'Proxy :443 · 3 routes' : 'Proxy stopped';
    else if (activeTab === 'services') el.textContent = '5 discovered';
  };

  const syncDemo = () => {
    let dbRunning = 0;
    let totalActive = 0;
    for (const [id, on] of Object.entries(state)) {
      if (on) {
        totalActive += 1;
        if (meta[id][0] === 'db') dbRunning += 1;
      }
      $$(`[data-toggle-service="${id}"]`).forEach((sw) => sw.setAttribute('aria-checked', String(on)));
      $$(`[data-status-dot="${id}"]`).forEach((dot) => dot.classList.toggle('is-off', !on));
    }
    if ($('sidebar-db-count')) $('sidebar-db-count').textContent = String(dbRunning);
    if ($('sidebar-https-pill')) $('sidebar-https-pill').textContent = state['https-proxy'] ? 'ON' : 'OFF';
    if ($('mb-running-label')) $('mb-running-label').textContent = `${totalActive} running`;
    if ($('mb-pop-count')) $('mb-pop-count').textContent = `${totalActive} active`;
    $('mb-status-dot')?.classList.toggle('is-off', totalActive === 0);
    updateViewMeta();
  };

  $$('[data-toggle-service]').forEach((sw) =>
    sw.addEventListener('click', () => {
      const id = sw.dataset.toggleService;
      if (!(id in state)) return;
      state[id] = !state[id];
      syncDemo();
      const [, src, stream] = meta[id];
      const verb = state[id] ? 'started' : 'stopped';
      appendLog(stream, src, `${id} ${verb}`, state[id]);
      showToast(`${id} ${verb}`);
    })
  );
  syncDemo();

  const selectTab = (key) => {
    if (!titles[key]) return;
    activeTab = key;
    $$('[data-app-tab]').forEach((b) => b.setAttribute('aria-selected', String(b.dataset.appTab === key)));
    $$('.app-view').forEach((v) => v.classList.toggle('active', v.id === `app-view-${key}`));
    if ($('ws-view-title')) $('ws-view-title').textContent = titles[key];
    updateViewMeta();
  };
  $$('[data-app-tab]').forEach((b) => b.addEventListener('click', () => selectTab(b.dataset.appTab)));

  /* Menu Bar Popover */
  const mbTrigger = $('mb-popover-trigger');
  const mbPopover = $('mb-popover');
  const setPopover = (open) => {
    mbPopover?.classList.toggle('open', open);
    mbTrigger?.setAttribute('aria-expanded', String(open));
  };

  mbTrigger?.addEventListener('click', (e) => {
    e.stopPropagation();
    setPopover(!mbPopover.classList.contains('open'));
  });
  document.addEventListener('click', (e) => {
    if (mbPopover?.classList.contains('open') && !mbPopover.contains(e.target) && !mbTrigger?.contains(e.target)) {
      setPopover(false);
    }
  });

  $('mb-stop-all')?.addEventListener('click', () => {
    state['pg-main'] = state['redis-cache'] = state['mysql-analytics'] = false;
    syncDemo();
    appendLog('db', 'devbase', 'stopped all databases (external data retained)', false);
    showToast('Stopped all databases');
  });
  $('mb-pop-copy-url')?.addEventListener('click', () => {
    copyText('https://api.devbase.localhost');
    showToast('Copied https://api.devbase.localhost');
    setPopover(false);
  });
  $('mb-pop-close')?.addEventListener('click', () => setPopover(false));

  /* Live Log Drawer */
  const logsDrawer = $('demo-log-drawer');
  const setLogs = (open) => {
    logsDrawer?.classList.toggle('open', open);
    $$('[data-toggle-logs]').forEach((b) => b.setAttribute('aria-pressed', String(open)));
  };
  $$('[data-toggle-logs]').forEach((b) => b.addEventListener('click', () => setLogs(!logsDrawer?.classList.contains('open'))));
  $('demo-logs-close')?.addEventListener('click', () => setLogs(false));
  $('mb-pop-logs')?.addEventListener('click', () => { setPopover(false); setLogs(true); });

  $$('[data-log-filter]').forEach((tab) =>
    tab.addEventListener('click', () => {
      logFilter = tab.dataset.logFilter;
      $$('[data-log-filter]').forEach((t) => t.classList.toggle('active', t === tab));
      if (!logLines) return;
      [...logLines.children].forEach((line) => {
        line.style.display = logFilter === 'all' || logFilter === line.dataset.stream ? '' : 'none';
      });
    })
  );

  /* Command Palette */
  const cmdk = $('demo-cmdk');
  const cmdkInput = $('demo-cmdk-input');
  const cmdkList = $('demo-cmdk-list');

  const filterCmdk = (q) => {
    const needle = q.trim().toLowerCase();
    cmdkList?.querySelectorAll('.cmdk-item').forEach((item) => {
      item.style.display = !needle || item.textContent.toLowerCase().includes(needle) ? '' : 'none';
    });
  };

  const setCmdk = (open) => {
    cmdk?.classList.toggle('open', open);
    if (open && cmdkInput) {
      cmdkInput.value = '';
      filterCmdk('');
      cmdkInput.focus();
    }
  };

  $('demo-cmdk-btn')?.addEventListener('click', () => setCmdk(true));
  $('sidebar-cmdk-btn')?.addEventListener('click', () => setCmdk(true));
  $('demo-cmdk-close')?.addEventListener('click', () => setCmdk(false));
  cmdk?.addEventListener('click', (e) => { if (e.target === cmdk) setCmdk(false); });
  cmdkInput?.addEventListener('input', () => filterCmdk(cmdkInput.value));
  cmdkList?.querySelectorAll('[data-cmd-action]').forEach((item) =>
    item.addEventListener('click', () => {
      const act = item.dataset.cmdAction;
      setCmdk(false);
      if (act.startsWith('tab:')) selectTab(act.slice(4));
      else if (act === 'toggle-popover') setPopover(!mbPopover?.classList.contains('open'));
      else if (act === 'toggle-logs') setLogs(!logsDrawer?.classList.contains('open'));
    })
  );

  /* QR, Shell Cycler, Real Copy Toasts, Info Toasts, Backup, Services Filter */
  const qrBtn = $('demo-qr-btn');
  const qrPanel = $('demo-qr-panel');
  qrBtn?.addEventListener('click', () => qrBtn.setAttribute('aria-expanded', String(qrPanel.classList.toggle('open'))));

  const shells = ['zsh', 'bash', 'fish'];
  $$('[data-shell-cycle]').forEach((btn) =>
    btn.addEventListener('click', () => {
      const next = shells[(shells.indexOf(btn.dataset.shellCycle || 'zsh') + 1) % shells.length];
      btn.dataset.shellCycle = next;
      btn.textContent = `Shell: ${next}`;
      showToast(`Reversible ${next} hook active`);
    })
  );

  $$('[data-copy-toast]').forEach((btn) =>
    btn.addEventListener('click', () => {
      const value = btn.dataset.copyValue || btn.dataset.copyToast;
      copyText(value);
      showToast(btn.dataset.copyToast);
    })
  );

  $$('[data-info-toast]').forEach((btn) =>
    btn.addEventListener('click', () => showToast(btn.dataset.infoToast))
  );

  $$('[data-backup-db]').forEach((btn) =>
    btn.addEventListener('click', () => {
      const db = btn.dataset.backupDb;
      appendLog('db', db, `snapshot saved ${db}-20261121.sql.gz`, true);
      showToast(`Backup saved for ${db}`);
    })
  );

  const svcSearch = $('demo-svc-search');
  let svcSource = 'all';
  const filterServices = () => {
    const q = (svcSearch?.value || '').trim().toLowerCase();
    $$('[data-svc-item]').forEach((row) => {
      const okSrc = svcSource === 'all' || row.dataset.source === svcSource;
      const okQ = !q || (row.dataset.name || '').toLowerCase().includes(q);
      row.style.display = okSrc && okQ ? '' : 'none';
    });
  };
  svcSearch?.addEventListener('input', filterServices);
  $$('[data-svc-source]').forEach((pill) =>
    pill.addEventListener('click', () => {
      svcSource = pill.dataset.svcSource;
      $$('[data-svc-source]').forEach((p) => p.classList.toggle('active', p === pill));
      filterServices();
    })
  );

  /* Keyboard Shortcuts (Guarded on Editable Elements) */
  const isEditable = (el) => {
    if (!el || el === document.body) return false;
    const tag = el.tagName;
    return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable;
  };

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      setCmdk(false);
      setPopover(false);
      closeMobile();
      return;
    }

    if (isEditable(document.activeElement)) return;

    const k = e.key.toLowerCase();
    if ((e.metaKey || e.ctrlKey) && !e.altKey && k === 'k') {
      e.preventDefault();
      setCmdk(!cmdk?.classList.contains('open'));
    } else if (e.ctrlKey && e.altKey && k === 'd') {
      e.preventDefault();
      setPopover(!mbPopover?.classList.contains('open'));
    } else if ((e.metaKey || e.ctrlKey) && !e.altKey && k === 'l') {
      e.preventDefault();
      setLogs(!logsDrawer?.classList.contains('open'));
    }
  });

  /* CLI & MCP 3-Tab Terminal */
  const cliCopyBtn = $('cli-copy-btn');
  let activeTerm = 'schema';

  $$('[data-term-tab]').forEach((tab) =>
    tab.addEventListener('click', () => {
      activeTerm = tab.dataset.termTab;
      $$('[data-term-tab]').forEach((t) => t.setAttribute('aria-selected', String(t === tab)));
      $$('.terminal-pane').forEach((p) => p.classList.toggle('active', p.id === `term-${activeTerm}`));
      if (cliCopyBtn) {
        cliCopyBtn.classList.remove('copied');
        const lbl = cliCopyBtn.querySelector('span');
        if (lbl) lbl.textContent = 'Copy';
      }
    })
  );

  cliCopyBtn?.addEventListener('click', () => {
    const pane = $(`term-${activeTerm}`);
    if (!pane) return;
    copyText(pane.dataset.copyText || pane.textContent).then(() => {
      cliCopyBtn.classList.add('copied');
      const lbl = cliCopyBtn.querySelector('span');
      if (lbl) lbl.textContent = 'Copied';
      setTimeout(() => {
        cliCopyBtn.classList.remove('copied');
        if (lbl) lbl.textContent = 'Copy';
      }, 1600);
    });
  });
})();
