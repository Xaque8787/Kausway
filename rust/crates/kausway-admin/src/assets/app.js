'use strict';
let setupToken = new URLSearchParams(location.hash.slice(1)).get('token') || '';
if (location.hash) history.replaceState(null, '', location.pathname);
let csrf = '';
const message = document.getElementById('message');
async function api(path, body) {
  const response = await fetch(path, body === undefined ? {cache: 'no-store'} : {
    method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body)
  });
  const value = await response.json();
  if (!response.ok) throw new Error(value.error || 'Request failed.');
  return value;
}
async function initialize() {
  try {
    const status = await api('/api/status');
    const section = status.authenticated ? 'account' : status.configured ? 'login' : 'setup';
    document.getElementById(section).hidden = false;
    csrf = status.csrf || '';
    if (section === 'setup') document.querySelector('#setup-form [name=token]').value = setupToken;
    if (section === 'account') {
      document.getElementById('signed-in').textContent = `Signed in as ${status.username}`;
      document.querySelector('#password-form [name=username]').value = status.username;
    }
  } catch (error) { message.textContent = error.message; }
}
for (const [id, path, destination] of [
  ['setup-form', '/api/setup', '/login'], ['login-form', '/api/login', '/admin'],
  ['password-form', '/api/password', '/login']
]) {
  document.getElementById(id).addEventListener('submit', async event => {
    event.preventDefault();
    const form = event.currentTarget;
    const fields = Object.fromEntries(new FormData(form));
    if ('confirm' in fields && fields.confirm !== fields.password) {
      message.textContent = 'Passwords do not match.'; return;
    }
    if (id !== 'login-form') {
      const length = new TextEncoder().encode(fields.password).length;
      if (length < 12 || length > 1024) {
        message.textContent = 'Password must contain 12–1024 bytes.'; return;
      }
    }
    delete fields.confirm;
    if (id === 'password-form') fields.csrf = csrf;
    const button = form.querySelector('button'); button.disabled = true;
    try {
      await api(path, fields); setupToken = ''; form.reset(); location.replace(destination);
    } catch (error) { message.textContent = error.message; }
    finally { button.disabled = false; }
  });
}
document.getElementById('logout').addEventListener('click', async () => {
  try { await api('/api/logout', {csrf}); location.replace('/login'); }
  catch (error) { message.textContent = error.message; }
});
initialize();
