'use strict';
// Run the shipped handlers against a small DOM adapter. No browser or public services required.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const root = path.resolve(__dirname, '../..');
class Element {
  constructor() {
    this.value = ''; this.hidden = false; this.checked = false; this.disabled = false;
    this.textContent = ''; this.listeners = {}; this.children = []; this.dataset = {};
    const classes = new Set();
    this.classList = {toggle(name, force) { if (force) classes.add(name); else classes.delete(name); }, contains(name) { return classes.has(name); }};
  }
  addEventListener(name, handler) { this.listeners[name] = handler; }
  setAttribute() {}
  replaceChildren(...children) { this.children = children; }
  append(...children) { this.children.push(...children); }
  appendChild(child) { this.children.push(child); }
  select() {}
}
function fixture() {
  const elements = new Map();
  const element = key => {
    if (!elements.has(key)) elements.set(key, new Element());
    return elements.get(key);
  };
  const page = element('#KauswayConfigPage');
  page.querySelector = element;
  const intervals = [];
  const window = {addEventListener: (name, handler) => { window[name] = handler; }, isSecureContext: true};
  const document = {getElementById: element, querySelector: element, querySelectorAll: () => [], createElement: () => new Element()};
  const clipboard = [];
  const context = vm.createContext({document, window, Node: Element,
    URLSearchParams, location: {hash: '', pathname: '/admin'}, history: {replaceState() {}},
    setInterval: callback => { intervals.push(callback); return intervals.length; }, clearInterval() {},
    navigator: {clipboard: {writeText: async value => { clipboard.push(value); }}},
    fetch: () => new Promise(() => {}),
    FormData: class {get(name) { return {label: 'Phone', ttl_secs: '900', short_code: 'on'}[name]; }},
    Dashboard: {alert() {}}, ApiClient: {accessToken: () => 'test-token', getUrl: value => value},
  });
  return {context, element, page, window, intervals, clipboard};
}
const invitation = () => ({id: 'inv-test', link: 'kausway://invite/test-secret', short_code: 'K1-000000-111111-222222',
  expires_at: Math.floor(Date.now() / 1000) + 600, rendezvous: {state: 'pending'}});
const settle = () => new Promise(resolve => setImmediate(resolve));
async function admin() {
  const f = fixture();
  vm.runInContext(fs.readFileSync(path.join(root, 'rust/crates/kausway-admin/src/assets/app.js'), 'utf8'), f.context);
  const requests = [];
  f.context.payload = invitation();
  f.context.apiAdapter = async (url, body) => { requests.push({url, body}); return f.context.payload; };
  vm.runInContext('api = apiAdapter; refreshOverview = async () => {};', f.context);
  await f.element('invite-form').listeners.submit({preventDefault() {}, currentTarget: {}});
  assert.equal(requests[0].body.short_code, true);
  assert.equal(f.element('invite-link').value, f.context.payload.link);
  assert.equal(f.element('invite-code').value, f.context.payload.short_code);
  assert.equal(f.element('new-invite').hidden, false);
  assert.match(f.element('invite-publication').textContent, /Publishing/);
  await f.element('copy-code').listeners.click();
  assert.equal(f.clipboard[0], f.context.payload.short_code);
  vm.runInContext('activeInvitation.expires_at = 1; updateInvitationExpiry();', f.context);
  assert.equal(f.element('invite-link').value, '');
  assert.equal(f.element('invite-code').value, '');
  assert.equal(f.element('new-invite').hidden, true);
  await f.element('invite-form').listeners.submit({preventDefault() {}, currentTarget: {}});
  f.window.pagehide();
  assert.equal(f.element('invite-link').value, '');
  assert.equal(f.element('invite-code').value, '');
  const retry = vm.runInContext("publicationText({rendezvous:{state:'retrying',error:'too_large'}})", f.context);
  assert.match(retry, /full invitation URL/);
  assert.match(retry, /does not fit/);
}
async function jellyfin() {
  const f = fixture();
  let payload = invitation(); let missing = false; const requests = [];
  f.context.fetch = async (url, options) => {
    requests.push({url, options});
    const response = options.method === 'POST' ? payload : {invitations: missing ? [] : [{id: payload.id, label: 'Phone', expires_at: payload.expires_at, rendezvous: payload.rendezvous}]};
    return {ok: true, status: 200, text: async () => JSON.stringify(response)};
  };
  const html = fs.readFileSync(path.join(root, 'integrations/jellyfin/plugin/Configuration/configPage.html'), 'utf8');
  const script = [...html.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g)].map(match => match[1]).join('\n');
  vm.runInContext(script, f.context);
  f.element('#kauswayInviteLabel').value = 'Phone'; f.element('#kauswayInviteTtl').value = '900';
  f.element('#kauswayShortCode').checked = true;
  const submit = async () => { f.element('#kauswayInviteForm').listeners.submit({preventDefault() {}}); await settle(); };
  await submit();
  assert.equal(JSON.parse(requests[0].options.body).short_code, true);
  assert.equal(f.element('#kauswayInviteCode').value, payload.short_code);
  assert.equal(f.element('#kauswayInviteLink').value, payload.link);
  f.element('#kauswayCopyCode').listeners.click(); await settle();
  assert.equal(f.clipboard[0], payload.short_code);
  f.page.listeners.pagehide();
  assert.equal(f.element('#kauswayInviteCode').value, '');
  assert.equal(f.element('#kauswayInviteLink').value, '');
  payload = {...invitation(), expires_at: 1}; await submit();
  assert.equal(f.element('#kauswayInviteCode').value, '');
  assert.equal(f.element('#kauswayInviteLink').value, '');
  payload = invitation(); missing = true; await submit();
  assert.equal(f.element('#kauswayInviteCode').value, '');
  assert.equal(f.element('#kauswayInviteLink').value, '');
}
(async () => { await admin(); await jellyfin(); console.log('Invitation UI handlers passed: opt-in, both inputs, copying, status, expiry, page exit and retirement.'); })().catch(error => { console.error(error); process.exitCode = 1; });
