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
  if (!response.ok) { if (response.status === 401 && csrf) location.replace('/login'); throw new Error(value.error || 'Request failed.'); }
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
      await loadAdmin();
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
let configuration = null;
let settingsDirty = false;
let loadingConfiguration = null;
let overview = null;
let refreshing = false;
function text(id, value) { document.getElementById(id).textContent = value; }
function valueText(value) { return value === null || value === undefined ? 'Unavailable / not saved' : typeof value === 'object' ? JSON.stringify(value) : String(value); }
function table(id, headings, rows) {
  const container = document.getElementById(id); container.replaceChildren();
  if (!rows.length) { container.textContent = 'No entries.'; return; }
  const table = document.createElement('table'); const head = table.createTHead().insertRow();
  for (const heading of headings) { const cell = document.createElement('th'); cell.textContent = heading; head.append(cell); }
  const body = table.createTBody();
  for (const cells of rows) {
    const row = body.insertRow();
    for (const content of cells) {
      const cell = row.insertCell();
      if (content instanceof Node) cell.append(content); else cell.textContent = valueText(content);
    }
  }
  container.append(table);
}
function definitionList(id, entries) {
  const list = document.getElementById(id); list.replaceChildren();
  for (const [key, value] of entries) {
    const term = document.createElement('dt'); term.textContent = key;
    const description = document.createElement('dd'); description.textContent = valueText(value);
    list.append(term, description);
  }
}
function action(label, path, body, confirmation) {
  const button = document.createElement('button'); button.textContent = label;
  button.addEventListener('click', async () => {
    if (confirmation && !confirm(confirmation)) return;
    button.disabled = true;
    try { await api(path, {...body, csrf}); await refreshOverview(); }
    catch (error) { message.textContent = error.message; }
    finally { button.disabled = false; }
  });
  return button;
}
async function refreshOverview() {
  if (refreshing) return;
  refreshing = true;
  try {
    overview = await api('/api/overview');
    const host = overview.host; const manager = overview.manager;
    text('host-state', host.available ? 'Host is running.' : host.error || 'Host is unavailable.');
    const status = host.status || {};
    definitionList('host-summary', host.available ? [
      ['Endpoint identity', status.endpoint_id], ['Origin', status.origin], ['Transport policy', status.policy.configured],
      ['Policy enforcement', status.policy.enforcement], ['Transport restart required', status.policy.restart_required], ['Relay connected', status.relay_connected],
      ['Authorized devices', status.devices], ['Active sessions', status.sessions], ['Uptime seconds', status.uptime_secs]
    ] : []);
    text('manager-state', !manager.configured ? 'No manager configured. The host is externally supervised.' : manager.available ? `Manager available. Host ${manager.status.host.observed}; desired ${manager.status.desired}.` : manager.error);
    const application = overview.lifecycle.application_name || 'the application';
    text('lifecycle-note', manager.configured && !manager.available ? 'Configured manager unavailable. Lifecycle controls are disabled.' : overview.lifecycle.restart_policy === 'application' ? `Restart ${application} for startup changes to take effect.` : !manager.available ? 'Host lifecycle controls require the configured owning manager.' : 'Stopping or restarting the host interrupts active client connections.');
    document.getElementById('lifecycle-actions').hidden = !overview.lifecycle.allowed;
    document.getElementById('save-restart').hidden = !overview.lifecycle.allowed;
    for (const button of document.querySelectorAll('[data-lifecycle]')) {
      const observed = manager.status?.host.observed;
      const active = ['running', 'starting'].includes(observed);
      button.disabled = !overview.lifecycle.allowed || observed === 'stopping' || (button.dataset.lifecycle === 'start' ? active : !active);
    }
    document.getElementById('create-invite').disabled = !host.available;
    document.getElementById('apply-config').disabled = !host.available;
    table('invitation-list', ['Label', 'Expires', 'Action'], overview.invitations.map(invitation => [invitation.label, new Date(invitation.expires_at * 1000).toLocaleString(), action('Cancel', '/api/invitations/cancel', {id: invitation.id}, 'Cancel this invitation?')]));
    table('device-list', ['Name', 'Endpoint', 'State', 'Action'], overview.devices.map(device => [device.name, device.endpoint_id, device.revoked_at ? 'Revoked' : device.connected ? 'Connected' : 'Authorized', device.revoked_at ? '' : action('Revoke', '/api/devices/revoke', {id: device.endpoint_id}, 'Revoke this device? Its active connections will end.')]));
    table('session-list', ['Device', 'Endpoint', 'Path', 'RTT ms', 'Requests', 'Bytes sent'], overview.sessions.map(session => [session.name, session.endpoint_id, session.path?.kind, session.path?.rtt_ms, session.requests, session.bytes_sent]));
    const diagnostics = document.getElementById('diagnostic-list'); diagnostics.replaceChildren();
    text('diagnostic-note', manager.available ? 'Recent manager lifecycle diagnostics.' : manager.configured ? manager.error : 'Process diagnostics require a manager. Live connection details appear under Sessions.');
    for (const line of manager.status?.diagnostics || []) { const item = document.createElement('li'); item.textContent = line; diagnostics.append(item); }
    for (const key of ['devices_error', 'invitations_error', 'sessions_error']) { if (overview[key]) { const item = document.createElement('li'); item.textContent = overview[key]; diagnostics.append(item); } }
  } catch (error) { message.textContent = error.message; }
  finally { refreshing = false; }
}
function getPath(object, path) { return path.split('.').reduce((current, key) => current?.[key], object); }
function setPath(object, path, value) {
  const keys = path.split('.'); let current = object;
  for (const key of keys.slice(0, -1)) current = current[key];
  current[keys.at(-1)] = value;
}
async function reloadConfiguration() {
  if (loadingConfiguration) return loadingConfiguration;
  loadingConfiguration = loadConfiguration();
  try { return await loadingConfiguration; } finally { loadingConfiguration = null; }
}
async function loadConfiguration() {
  const controls = document.querySelectorAll('#settings-form input, #settings-form select, #settings-form button');
  for (const control of controls) control.disabled = true;
  try {
  configuration = await api('/api/config');
  const form = document.getElementById('settings-form');
  for (const field of form.querySelectorAll('[name]')) {
    const value = getPath(configuration.saved, field.name); field.value = value === null || value === undefined ? '' : String(value);
  }
  text('config-note', configuration.effective_available ? 'Saved changes do not automatically change running settings. Apply supported live settings or restart as indicated.' : `Effective host settings are unavailable. ${configuration.effective_error || ''} You can edit saved settings while the host is stopped.`);
  document.getElementById('config-note').textContent += ` ${configuration.restart_note || ''}`;
  if (configuration.transport_restart_required) document.getElementById('config-note').textContent += ' Transport construction also requires a restart.';
  table('config-fields', ['Setting', 'Saved', 'Effective', 'Source', 'Change', 'Status'], Object.entries(configuration.fields).map(([key, field]) => [key, field.saved, field.effective, field.source, field.change, (configuration.overridden || []).includes(key) ? 'Overridden at launch' : (configuration.pending || []).includes(key) ? 'Pending apply / restart' : field.source === 'unavailable' ? 'Effective value unavailable' : 'Current']));
  definitionList('storage-paths', Object.entries(configuration.storage));
  settingsDirty = false;
  } finally {
    for (const control of controls) control.disabled = false;
    document.getElementById('apply-config').disabled = !overview?.host.available;
  }
}
function editedSettings() {
  if (!configuration) throw new Error('Load configuration first.');
  const settings = structuredClone(configuration.saved);
  const numeric = new Set(['manager.shutdown_timeout_secs', 'manager.restart_backoff_initial_secs', 'manager.restart_backoff_max_secs', 'manager.max_failures', 'manager.readiness_timeout_secs']);
  const booleans = new Set(['host.websockets', 'host.control.allow_remote', 'manager.restart_on_failure', 'admin.enabled']);
  for (const field of document.getElementById('settings-form').querySelectorAll('[name]')) {
    const value = field.value.trim();
    setPath(settings, field.name, value === '' ? null : numeric.has(field.name) ? Number(value) : booleans.has(field.name) ? value === 'true' : value);
  }
  return settings;
}
async function saveSettings(restart) {
  if (!document.getElementById('settings-form').reportValidity()) return;
  const settings = editedSettings();
  if (settings.host.origin !== configuration.saved.host.origin && !confirm('Changing the origin changes the service available to authorized clients. Save this change?')) return;
  if (restart && !confirm('Save and restart the host? Active client connections will be interrupted.')) return;
  const result = await api(restart ? '/api/config/save-restart' : '/api/config/save', {csrf, revision: configuration.revision, settings});
  message.textContent = result.restart_error ? `Saved. Restart failed: ${result.restart_error}` : restart ? 'Saved. Restart requested; check Overview for readiness or failures.' : `Saved. Running settings have not been changed. ${configuration.restart_note || 'Review pending settings and launch overrides.'}`;
  await reloadConfiguration(); await refreshOverview();
}
async function loadAdmin() { await refreshOverview(); await reloadConfiguration(); }
for (const button of document.querySelectorAll('[data-panel]')) {
  button.addEventListener('click', () => {
    if (button.dataset.panel === 'settings' && !settingsDirty) reloadConfiguration().catch(error => { message.textContent = error.message; });
    for (const section of document.querySelectorAll('[id^=panel-]')) section.hidden = section.id !== `panel-${button.dataset.panel}`;
    for (const item of document.querySelectorAll('[data-panel]')) item.setAttribute('aria-current', String(item === button));
  });
}
document.querySelector('[data-panel=overview]').setAttribute('aria-current', 'true');
for (const button of document.querySelectorAll('[data-lifecycle]')) {
  button.addEventListener('click', async () => {
    const command = button.dataset.lifecycle;
    if (command !== 'start' && !confirm(`${command === 'stop' ? 'Stop' : 'Restart'} the host? Active client connections will be interrupted.`)) return;
    button.disabled = true;
    try { await api('/api/lifecycle', {csrf, action: command}); message.textContent = 'Lifecycle request accepted. Check host readiness below.'; await refreshOverview(); }
    catch (error) { message.textContent = error.message; button.disabled = false; }
  });
}
document.getElementById('refresh-host').addEventListener('click', async () => {
  document.getElementById('invite-link').value = ''; document.getElementById('new-invite').hidden = true; await refreshOverview();
});
document.getElementById('settings-form').addEventListener('input', () => { settingsDirty = true; });
document.getElementById('settings-form').addEventListener('change', () => { settingsDirty = true; });
document.getElementById('reload-config').addEventListener('click', () => {
  if (settingsDirty && !confirm('Discard your unsaved settings and reload?')) return;
  reloadConfiguration().catch(error => { message.textContent = error.message; });
});
document.getElementById('settings-form').addEventListener('submit', event => { event.preventDefault(); saveSettings(false).catch(error => { message.textContent = error.message; }); });
document.getElementById('save-restart').addEventListener('click', () => saveSettings(true).catch(error => { message.textContent = error.message; }));
document.getElementById('apply-config').addEventListener('click', async () => {
  try {
    if (!configuration) throw new Error('Load configuration first.');
    if (!confirm('Apply saved live transport settings? Disabling WebSockets can interrupt active connections.')) return;
    const result = await api('/api/config/apply', {csrf, revision: configuration.revision});
    message.textContent = result.restart_required ? `Live settings saved, but transport construction also requires restarting ${overview?.lifecycle.restart_policy === 'application' ? overview.lifecycle.application_name : 'the host'}.` : 'Saved live settings applied.';
    await reloadConfiguration(); await refreshOverview();
  } catch (error) { message.textContent = error.message; }
});
document.getElementById('invite-form').addEventListener('submit', async event => {
  event.preventDefault(); const fields = new FormData(event.currentTarget);
  try {
    const result = await api('/api/invitations/create', {csrf, label: fields.get('label'), ttl_secs: Number(fields.get('ttl_secs'))});
    document.getElementById('invite-link').value = result.link; document.getElementById('new-invite').hidden = false;
    await refreshOverview();
  } catch (error) { message.textContent = error.message; }
});
document.getElementById('copy-invite').addEventListener('click', async () => {
  const field = document.getElementById('invite-link');
  try { await navigator.clipboard.writeText(field.value); message.textContent = 'Invitation link copied.'; }
  catch { field.select(); message.textContent = 'Select and copy the invitation link.'; }
});
setInterval(() => { if (csrf && !document.hidden) refreshOverview(); }, 5000);
initialize();
