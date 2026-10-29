(() => {
  const el = document.getElementById('status');
  fetch('/health')
    .then((r) => r.json())
    .then((d) => {
      el.textContent = d.status === 'ok'
        ? `ok · ${d.service} · port ${d.port} · auth0 ${d.auth0_configured ? 'configured' : 'NOT configured'}`
        : 'health check failed';
      el.classList.toggle('ok', d.status === 'ok');
    })
    .catch(() => {
      el.textContent = 'offline';
    });

  const login = document.getElementById('login');
  const signup = document.getElementById('signup');
  const logout = document.getElementById('logout');
  const user = document.getElementById('user');

  (async () => {
    window.client = await auth0.createAuth0Client({
      domain: 'dev-4vyek1k3zl2ztjsc.eu.auth0.com',
      clientId: '3gycR4ltgdYCsKPtOCK4HNiHwx2IaKdF',
      authorizationParams: { redirect_uri: location.origin },
    });

    if (location.search.includes('error=')) {
      const params = new URLSearchParams(location.search);
      if (user) user.textContent = `Error: ${params.get('error')} — ${params.get('error_description')}`;
      history.replaceState({}, '', location.pathname);
      login.style.display = 'none';
      signup.style.display = 'none';
      logout.style.display = 'none';
      return;
    }

    if (location.search.includes('code=') && location.search.includes('state=')) {
      await window.client.handleRedirectCallback();
      history.replaceState({}, '', location.pathname);
    }

    if (await window.client.isAuthenticated()) {
      const u = await window.client.getUser();
      logout.style.display = 'inline-block';
      if (user) user.textContent = JSON.stringify(u, null, 2);
    } else {
      login.style.display = 'inline-block';
      signup.style.display = 'inline-block';
      if (user) user.textContent = 'not authenticated';
    }
  })().catch(() => {
    login.style.display = 'inline-block';
    signup.style.display = 'inline-block';
  });

  async function loadConfig() {
    try {
      const r = await fetch('/api/config');
      const d = await r.json();
      const ta = document.getElementById('configEditor');
      if (ta) ta.value = d.content || '';
      const st = document.getElementById('configStatus');
      if (st) st.textContent = `loaded ${d.path || ''}`;
    } catch (e) {
      const st = document.getElementById('configStatus');
      if (st) st.textContent = `load failed: ${e}`;
    }
  }
  async function saveConfig() {
    try {
      const ta = document.getElementById('configEditor');
      const r = await fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ content: ta ? ta.value : '' }),
      });
      const d = await r.json();
      const st = document.getElementById('configStatus');
      if (st) st.textContent = JSON.stringify(d);
    } catch (e) {
      const st = document.getElementById('configStatus');
      if (st) st.textContent = `save failed: ${e}`;
    }
  }
  window.loadConfig = loadConfig;
  window.saveConfig = saveConfig;
  loadConfig();

  if ('serviceWorker' in navigator) {
    navigator.serviceWorker.register('/sw.js').catch(() => {});
  }
})();