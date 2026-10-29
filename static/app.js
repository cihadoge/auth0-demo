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
  const logout = document.getElementById('logout');
  const user = document.getElementById('user');
  fetch('/api/me')
    .then((r) => r.json())
    .then((d) => {
      if (d.authenticated) {
        logout.style.display = 'inline-block';
        user.textContent = JSON.stringify(d.user, null, 2);
      } else {
        login.style.display = 'inline-block';
        user.textContent = 'not authenticated';
      }
    })
    .catch(() => {
      login.style.display = 'inline-block';
    });

  if ('serviceWorker' in navigator) {
    navigator.serviceWorker.register('/sw.js').catch(() => {});
  }
})();
