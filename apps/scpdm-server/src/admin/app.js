// The server's administration page: plain JavaScript, no libraries. Everything shown is built
// with DOM calls (never innerHTML), so names and comments cannot inject markup.
"use strict";

const app = document.getElementById("app");
let me = null;
let refreshTimer = null;

// ---- helpers ----

/** An element: h("td", {class: "num"}, "3"). Attributes starting with "on" are listeners. */
function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (value === true) el.setAttribute(key, "");
    else el.setAttribute(key, value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : String(child));
  }
  return el;
}

const svg = (tag, attrs, ...children) => {
  const el = document.createElementNS("http://www.w3.org/2000/svg", tag);
  for (const [k, v] of Object.entries(attrs || {})) el.setAttribute(k, v);
  for (const c of children) el.append(c instanceof Node ? c : String(c));
  return el;
};

class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

async function api(method, path, body) {
  const options = { method, credentials: "same-origin", headers: { "X-PDM-Admin": "1" } };
  if (body !== undefined) {
    options.headers["Content-Type"] = "application/json";
    options.body = JSON.stringify(body);
  }
  const response = await fetch(path, options);
  if (response.status === 401 && path !== "/api/admin/login") {
    me = null;
    showLogin("Please log in again.");
    throw new ApiError(401, "log in first");
  }
  if (!response.ok) {
    let message = response.statusText;
    try { message = (await response.json()).message || message; } catch { /* not JSON */ }
    throw new ApiError(response.status, message);
  }
  return response.status === 200 ? response.json() : null;
}

function bytes(n) {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < units.length - 1);
  return `${n.toFixed(n < 10 ? 1 : 0)} ${units[i]}`;
}

function ago(secs, now = Date.now() / 1000) {
  if (!secs) return "never";
  const d = Math.max(0, now - secs);
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
  if (d < 30 * 86400) return `${Math.floor(d / 86400)} d ago`;
  return when(secs);
}

function when(secs) {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

function duration(secs) {
  const d = Math.floor(secs / 86400), hrs = Math.floor((secs % 86400) / 3600), m = Math.floor((secs % 3600) / 60);
  return d ? `${d} d ${hrs} h` : hrs ? `${hrs} h ${m} min` : `${m} min`;
}

const ACCESS = { read: "read", write: "read and change", admin: "administer" };

function toast(text, bad = false) {
  const el = h("div", { class: bad ? "toast bad" : "toast", role: "status" }, text);
  document.body.append(el);
  setTimeout(() => el.remove(), bad ? 6000 : 3000);
}

/** A modal dialog with a form; `submit` gets the form's values and may throw to show an error. */
function dialog(title, fields, submitLabel, submit, { danger = false, note = null } = {}) {
  const error = h("div", { class: "error" });
  const form = h("form", { method: "dialog" },
    h("h2", {}, title),
    note && h("p", {}, note),
    fields,
    error,
    h("div", { class: "buttons" },
      h("button", { type: "button", onclick: () => box.close() }, "Cancel"),
      h("button", { type: "submit", class: danger ? "danger primary" : "primary" }, submitLabel)));
  const box = h("dialog", {}, form);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const values = Object.fromEntries(new FormData(form));
    for (const input of form.querySelectorAll("input[type=checkbox]")) values[input.name] = input.checked;
    const button = form.querySelector("button[type=submit]");
    button.disabled = true;
    error.textContent = "";
    try {
      await submit(values);
      box.close();
    } catch (e) {
      if (e.status !== 401) error.textContent = e.message;
    } finally {
      button.disabled = false;
    }
  });
  box.addEventListener("close", () => box.remove());
  document.body.append(box);
  box.showModal();
  box.querySelector("input, select")?.focus();
}

const confirmDialog = (title, note, label, action) => dialog(title, [], label, action, { danger: true, note });

// ---- login ----

function showLogin(message) {
  clearInterval(refreshTimer);
  const error = h("div", { class: "error" }, message && message !== "log in first" ? message : "");
  const form = h("form", {},
    h("div", { class: "brand" }, h("img", { src: "icon.svg", alt: "" }), h("div", {}, h("b", {}, "SolidCraft PDM"), h("span", {}, "Server administration"))),
    h("label", { for: "user" }, "User"),
    h("input", { id: "user", name: "user", autocomplete: "username", required: true }),
    h("label", { for: "password" }, "Password"),
    h("input", { id: "password", name: "password", type: "password", autocomplete: "current-password", required: true }),
    error,
    h("div", { class: "buttons" }, h("button", { type: "submit", class: "primary" }, "Log in")));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const values = Object.fromEntries(new FormData(form));
    error.textContent = "";
    try {
      me = await api("POST", "/api/admin/login", values);
      route();
    } catch (e) {
      error.textContent = e.message;
    }
  });
  app.replaceChildren(h("div", { class: "login" }, h("div", { class: "panel form" }, form)));
  form.querySelector("input").focus();
}

// ---- frame ----

const PAGES = { dashboard: ["Dashboard", dashboard], users: ["Users", users], vaults: ["Vaults", vaults], backups: ["Backups", backups] };

function frame(page, content) {
  const nav = h("nav", { class: "nav" }, Object.entries(PAGES).map(([key, [label]]) =>
    h("a", { href: `#/${key}`, class: key === page ? "on" : null }, label)));
  const logout = async () => {
    try { await api("POST", "/api/admin/logout"); } catch { /* gone anyway */ }
    me = null;
    showLogin();
  };
  app.replaceChildren(h("div", { class: "shell" },
    h("aside", { class: "side" },
      h("div", { class: "brand" }, h("img", { src: "icon.svg", alt: "" }), h("div", {}, h("b", {}, me.server), h("span", {}, `PDM server ${me.version}`))),
      nav,
      h("div", { class: "who" }, "Logged in as ", h("b", {}, me.user), h("br"), h("a", { href: "#", onclick: (e) => { e.preventDefault(); logout(); } }, "Log out"))),
    h("main", {}, content)));
}

async function route() {
  clearInterval(refreshTimer);
  if (!me) {
    try { me = await api("GET", "/api/admin/me"); } catch (e) { if (e.status !== 401) showLogin(e.message); return; }
  }
  const page = (location.hash.match(/^#\/(\w+)/) || [])[1];
  const [title, render] = PAGES[page] || PAGES.dashboard;
  document.title = `${title} · PDM Server Admin`;
  try {
    frame(page in PAGES ? page : "dashboard", await render());
  } catch (e) {
    if (e.status !== 401) frame(page, h("div", { class: "panel error" }, e.message));
  }
}

window.addEventListener("hashchange", route);

// ---- dashboard ----

function chart(perDay) {
  const width = 600, height = 150, top = 6, bottom = 2;
  const max = Math.max(1, ...perDay.map(([, n]) => n));
  const slot = width / perDay.length;
  const bars = perDay.map(([day, n], i) => {
    const barHeight = n ? Math.max(3, (n / max) * (height - top - bottom)) : 2;
    const date = new Date(day * 86400 * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" });
    return svg("rect", { class: n ? "bar" : "bar zero", x: i * slot + 2, y: height - bottom - barHeight, width: slot - 4, height: barHeight, rx: 2 },
      svg("title", {}, `${date}: ${n} version${n === 1 ? "" : "s"}`));
  });
  // Labels in HTML: the drawing stretches to the width, text would stretch with it.
  const date = (i) => new Date(perDay[i][0] * 86400 * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" });
  return h("div", {},
    svg("svg", { class: "chart", viewBox: `0 0 ${width} ${height - bottom + 2}`, preserveAspectRatio: "none", role: "img", "aria-label": "Versions checked in per day" },
      svg("line", { class: "base", x1: 0, x2: width, y1: height - bottom + 0.5, y2: height - bottom + 0.5 }),
      ...bars),
    h("div", { class: "axis" }, h("span", {}, date(0)), h("span", {}, `most in a day: ${max}`), h("span", {}, date(perDay.length - 1))));
}

/** The dashboard's backup card: when the last one worked, or that it failed or is off. */
function backupCard(b, now) {
  const failed = b.last && !b.last.ok;
  const value = b.last_ok_at ? ago(b.last_ok_at, now) : "none yet";
  const sub = failed ? h("span", { class: "bad" }, "last run failed") : b.enabled ? "automatic" : h("span", { class: "warn" }, "not automatic");
  return h("a", { class: "card link", href: "#/backups" }, h("div", { class: "n small-n" }, value), h("div", { class: "l" }, "Last backup"), h("div", { class: "s" }, sub));
}

function card(n, label, sub) {
  return h("div", { class: "card" }, h("div", { class: "n" }, n), h("div", { class: "l" }, label), sub && h("div", { class: "s" }, sub));
}

async function dashboard() {
  const o = await api("GET", "/api/admin/overview");
  const t = o.totals;
  const total30 = o.per_day.reduce((sum, [, n]) => sum + n, 0);
  const breakLock = (c) => confirmDialog(`Break ${c.user}'s check-out?`,
    `${c.path} in ${c.vault}. Their changes stay in their workspace, but they can no longer check them in without checking it out again.`,
    "Break check-out", async () => {
      await api("POST", `/api/admin/vaults/${encodeURIComponent(c.vault)}/unlock`, { path: c.path });
      toast(`${c.path} is no longer checked out`);
      route();
    });
  const content = [
    h("div", { class: "head" }, h("h1", { class: "grow" }, "Dashboard"), h("span", { class: "muted small" }, `Up for ${duration(o.now - o.started_at)}`)),
    h("div", { class: "cards" },
      card(t.users, "Users", `${t.online} online · ${t.admins} admin${t.admins === 1 ? "" : "s"}`),
      card(t.vaults, "Vaults"),
      card(t.designs, "Designs", `${t.releases} release${t.releases === 1 ? "" : "s"}`),
      card(t.versions, "Versions", `${total30} in 30 days`),
      card(t.checked_out, "Checked out"),
      card(bytes(o.disk_bytes), "On disk", `${bytes(t.stored_bytes)} of versions`),
      backupCard(o.backup, o.now)),
    h("div", { class: "panel" }, h("h2", {}, "Check-ins, last 30 days"), chart(o.per_day)),
    h("div", { class: "grid" },
      h("div", { class: "panel" }, h("h2", {}, "Recent activity"),
        o.recent.length === 0 ? h("div", { class: "empty" }, "Nothing checked in yet.") :
          h("div", { class: "wide" }, h("table", {},
            h("thead", {}, h("tr", {}, h("th", {}, "Design"), h("th", {}, "By"), h("th", {}, "When"))),
            h("tbody", {}, o.recent.map((a) => h("tr", {},
              h("td", {}, h("div", { class: "path" }, `${a.path} `, h("span", { class: "muted nowrap" }, `v${a.number}`)),
                h("div", { class: "muted small" }, `${a.vault}${a.comment ? " · " + a.comment : ""}`)),
              h("td", {}, a.author),
              h("td", { class: "muted small nowrap", title: when(a.created_at) }, ago(a.created_at, o.now)))))))),
      h("div", { class: "panel" }, h("h2", {}, "Checked out now"),
        o.checked_out.length === 0 ? h("div", { class: "empty" }, "Nothing is checked out.") :
          h("div", { class: "wide" }, h("table", {},
            h("thead", {}, h("tr", {}, h("th", {}, "Design"), h("th", {}, "By"), h("th", {}, "Since"), h("th", {}))),
            h("tbody", {}, o.checked_out.map((c) => h("tr", {},
              h("td", {}, h("div", { class: "path" }, c.path), h("div", { class: "muted small" }, c.vault)),
              h("td", {}, c.user),
              h("td", { class: "muted small nowrap", title: when(c.since) }, ago(c.since, o.now)),
              h("td", { class: "actions" }, h("button", { class: "small danger", onclick: () => breakLock(c) }, "Break"))))))))),
    h("div", { class: "panel" }, h("h2", {}, "Server"),
      h("table", {}, h("tbody", {},
        h("tr", {}, h("td", { class: "muted" }, "Name"), h("td", {}, o.name)),
        h("tr", {}, h("td", { class: "muted" }, "Version"), h("td", {}, o.version)),
        h("tr", {}, h("td", { class: "muted" }, "Running since"), h("td", {}, when(o.started_at))),
        h("tr", {}, h("td", { class: "muted" }, "Data folder"), h("td", { class: "path" }, o.data)),
        h("tr", {}, h("td", { class: "muted" }, "Connect the app to"), h("td", { class: "path" }, location.origin))))),
  ];
  // Kept current while the page is open.
  refreshTimer = setInterval(() => { if (!document.hidden && !document.querySelector("dialog[open]")) route(); }, 30000);
  return content;
}

// ---- users ----

async function users() {
  const [list, vaultList] = await Promise.all([api("GET", "/api/admin/users"), api("GET", "/api/admin/vaults")]);
  const now = Date.now() / 1000;
  const change = async (name, body, done) => {
    await api("POST", `/api/admin/users/${encodeURIComponent(name)}`, body);
    toast(done);
    route();
  };
  const addUser = () => dialog("Add a user", [
    h("label", { for: "n" }, "Name"), h("input", { id: "n", name: "name", required: true, pattern: "[A-Za-z0-9._\\-]+", title: "letters, digits, . _ -" }),
    h("label", { for: "p" }, "Password (at least 8 characters)"), h("input", { id: "p", name: "password", type: "password", minlength: 8, required: true, autocomplete: "new-password" }),
    h("label", { class: "check" }, h("input", { type: "checkbox", name: "admin" }), "Administrator (every vault, this page)"),
  ], "Add user", async (v) => {
    await api("POST", "/api/admin/users", { name: v.name, password: v.password, admin: v.admin });
    toast(`Added ${v.name}`);
    route();
  });
  const password = (u) => dialog(`New password for ${u.name}`, [
    h("label", { for: "p" }, "Password (at least 8 characters)"), h("input", { id: "p", name: "password", type: "password", minlength: 8, required: true, autocomplete: "new-password" }),
  ], "Set password", (v) => change(u.name, { password: v.password }, `New password for ${u.name}; their logins have ended`),
  { note: "Their open logins end: the app asks for the new password." });
  const access = (u) => dialog(`Access for ${u.name}`, vaultList.length === 0 ? [h("p", {}, "There are no vaults yet.")] :
    vaultList.map((v) => [
      h("label", { for: `a-${v.name}` }, v.name),
      h("select", { id: `a-${v.name}`, name: v.name },
        ["none", "read", "write", "admin"].map((a) => h("option", { value: a, selected: (u.access[v.name] || "none") === a }, a === "none" ? "no access" : ACCESS[a]))),
    ]), "Save", async (values) => {
    for (const v of vaultList) {
      const wanted = values[v.name] === "none" ? null : values[v.name];
      if (wanted !== (u.access[v.name] || null)) {
        await api("PUT", `/api/admin/users/${encodeURIComponent(u.name)}/access/${encodeURIComponent(v.name)}`, { access: wanted });
      }
    }
    toast(`Access for ${u.name} saved`);
    route();
  }, { note: u.admin ? "Administrators may use every vault anyway." : null });
  const remove = (u) => confirmDialog(`Remove ${u.name}?`, "Their account, logins and access are deleted. Versions they checked in keep their name.",
    "Remove", async () => {
      await api("DELETE", `/api/admin/users/${encodeURIComponent(u.name)}`);
      toast(`Removed ${u.name}`);
      route();
    });
  const rows = list.map((u) => {
    const online = u.last_seen && now - u.last_seen < 300;
    const self = u.name === me.user;
    return h("tr", {},
      h("td", {}, h("b", {}, u.name), " ", u.admin && h("span", { class: "badge admin" }, "admin"), self && h("span", { class: "muted small" }, " (you)")),
      h("td", { class: "small nowrap" }, online && h("span", { class: "dot", title: "online" }), u.last_seen ? ago(u.last_seen, now) : h("span", { class: "muted" }, "never")),
      h("td", { class: "num" }, u.sessions),
      h("td", {}, u.admin ? h("span", { class: "muted small" }, "every vault") :
        Object.keys(u.access).length === 0 ? h("span", { class: "muted small" }, "none") :
          h("div", { class: "chips" }, Object.entries(u.access).map(([v, a]) => h("span", { class: "chip" }, `${v} `, h("i", {}, a))))),
      h("td", { class: "actions" },
        h("button", { class: "small", onclick: () => access(u) }, "Access…"), " ",
        h("button", { class: "small", onclick: () => password(u) }, "Password…"), " ",
        h("button", { class: "small", disabled: self, title: self ? "Not your own" : null,
          onclick: () => change(u.name, { admin: !u.admin }, u.admin ? `${u.name} is no longer an administrator` : `${u.name} is an administrator`) },
        u.admin ? "Remove admin" : "Make admin"), " ",
        h("button", { class: "small", disabled: u.sessions === 0, onclick: () => change(u.name, { end_sessions: true }, `${u.name} is logged out everywhere`) }, "Log out"), " ",
        h("button", { class: "small danger", disabled: self, onclick: () => remove(u) }, "Remove")));
  });
  return [
    h("div", { class: "head" }, h("h1", { class: "grow" }, "Users"), h("button", { class: "primary", onclick: addUser }, "Add user")),
    h("div", { class: "panel wide" }, h("table", {},
      h("thead", {}, h("tr", {}, h("th", {}, "User"), h("th", {}, "Last seen"), h("th", { class: "num" }, "Logins"), h("th", {}, "Vaults"), h("th", {}))),
      h("tbody", {}, rows))),
  ];
}

// ---- vaults ----

async function vaults() {
  const list = await api("GET", "/api/admin/vaults");
  const setCheckout = async (name, required) => {
    try {
      await api("PUT", `/api/admin/vaults/${encodeURIComponent(name)}/checkout`, { required });
      toast(required ? `${name}: designs are checked out to be changed` : `${name}: changes are checked in without checking out`);
    } catch (e) {
      if (e.status !== 401) toast(e.message, true);
    }
    route();
  };
  const addVault = () => dialog("Add a vault", [
    h("label", { for: "n" }, "Name"), h("input", { id: "n", name: "name", required: true, pattern: "[A-Za-z0-9._\\-]+", title: "letters, digits, . _ -" }),
  ], "Add vault", async (v) => {
    await api("POST", "/api/admin/vaults", { name: v.name });
    toast(`Added ${v.name}: give users access on the Users page`);
    route();
  }, { note: "An empty vault. Users get access to it on the Users page." });
  return [
    h("div", { class: "head" }, h("h1", { class: "grow" }, "Vaults"), h("button", { class: "primary", onclick: addVault }, "Add vault")),
    h("div", { class: "panel wide" }, list.length === 0 ? h("div", { class: "empty" }, "No vaults yet.") : h("table", {},
      h("thead", {}, h("tr", {}, h("th", {}, "Vault"), h("th", { class: "num" }, "Designs"), h("th", { class: "num" }, "Versions"),
        h("th", { class: "num" }, "Releases"), h("th", { class: "num" }, "Checked out"), h("th", { class: "num" }, "Stored"),
        h("th", { class: "num" }, "Users"), h("th", {}, "Last change"), h("th", {}, "Check-out"))),
      h("tbody", {}, list.map((v) => h("tr", {},
        h("td", {}, h("b", {}, v.name)),
        h("td", { class: "num" }, v.designs),
        h("td", { class: "num" }, v.versions),
        h("td", { class: "num" }, v.releases),
        h("td", { class: "num" }, v.checked_out),
        h("td", { class: "num" }, bytes(v.stored_bytes)),
        h("td", { class: "num" }, v.users),
        h("td", { class: "muted small", title: v.last_change ? when(v.last_change) : null }, v.last_change ? ago(v.last_change) : "never"),
        h("td", {}, h("select", {
          class: "compact",
          "aria-label": `Check-out in ${v.name}`,
          title: "Required: one person at a time changes a design (a team). Optional: changes are checked in without checking out (one person).",
          onchange: (e) => setCheckout(v.name, e.target.value === "required"),
        }, h("option", { value: "required", selected: v.checkout_required }, "required"), h("option", { value: "optional", selected: !v.checkout_required }, "optional")))))))),
    h("p", { class: "muted small" }, "Users: with access of their own; administrators may use every vault. Check-out: required for a team (one person changes a design at a time); optional for a vault one person uses."),
  ];
}

// ---- backups ----

function inHours(secs, now) {
  const d = secs - now;
  if (d <= 60) return "in a moment";
  if (d < 3600) return `in ${Math.round(d / 60)} min`;
  if (d < 86400) return `in ${Math.round(d / 3600)} h`;
  return `on ${when(secs)}`;
}

const TRIGGERS = { schedule: "scheduled", page: "this page", "command line": "command line" };
const INTERVALS = [1, 2, 3, 6, 12, 24, 48, 72, 168];

function intervalText(hours) {
  if (hours === 1) return "every hour";
  if (hours === 24) return "every day";
  if (hours === 168) return "every week";
  return hours % 24 === 0 ? `every ${hours / 24} days` : `every ${hours} hours`;
}

async function backups() {
  const b = await api("GET", "/api/admin/backups");
  const now = Date.now() / 1000;
  const failed = b.runs_total - b.runs_ok;
  const last = b.runs[0];
  const runNow = async () => {
    try {
      await api("POST", "/api/admin/backups/run");
      toast("Backup started");
      route();
    } catch (e) {
      if (e.status !== 401) toast(e.message, true);
    }
  };

  const warnings = [];
  if (!b.target) warnings.push("No backup folder yet: set one below.");
  else if (!b.target_exists) warnings.push(`${b.target} is not there. If it is a share, mount it; backups fail until then.`);
  if (b.target && b.same_disk) warnings.push("The backup folder is on the same disk as the server's data: a failing disk takes both. A second disk or a share (SMB, NFS) is safer.");
  if (last && !last.ok) warnings.push(`The last backup failed: ${last.message}`);

  const s = b.settings;
  const form = h("form", { class: "form settings" },
    h("label", { class: "check" }, h("input", { type: "checkbox", name: "enabled", checked: s.enabled }), "Back up automatically"),
    h("div", { class: "fields" },
      h("div", {}, h("label", { for: "interval" }, "How often"),
        h("select", { id: "interval", name: "interval_hours" },
          INTERVALS.concat(INTERVALS.includes(s.interval_hours) ? [] : [s.interval_hours]).map((n) =>
            h("option", { value: n, selected: n === s.interval_hours }, intervalText(n))))),
      h("div", {}, h("label", { for: "keep" }, "Keep the newest"),
        h("div", { class: "unit" }, h("input", { id: "keep", name: "keep_count", type: "number", min: 1, max: 1000, value: s.keep_count, required: true }), h("span", {}, "backups"))),
      h("div", {}, h("label", { for: "days" }, "Delete older than"),
        h("div", { class: "unit" }, h("input", { id: "days", name: "keep_days", type: "number", min: 0, max: 3650, value: s.keep_days, required: true }), h("span", {}, "days")))),
    h("div", { class: "muted small" }, "The newest backup always stays. 0 days: no age limit, only the number counts."),
    h("label", { for: "target" }, "Backup folder"),
    b.target_fixed
      ? h("div", {}, h("div", { class: "path fixed" }, b.target), h("div", { class: "muted small" }, "Set when the server was started (--backup-dir or SCPDM_BACKUP_DIR; in Docker the /backups volume)."))
      : h("div", {}, h("input", { id: "target", name: "target", value: s.target || "", placeholder: "/mnt/nas/pdm-backups", spellcheck: "false" }),
        h("div", { class: "muted small" }, "A folder on the server: another disk, or an SMB or NFS share mounted there. It must exist; the server never creates it, so an unmounted share is noticed.")),
    h("div", { class: "error" }),
    h("div", { class: "buttons" }, h("button", { type: "submit", class: "primary" }, "Save")));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const v = Object.fromEntries(new FormData(form));
    const error = form.querySelector(".error");
    error.textContent = "";
    try {
      await api("PUT", "/api/admin/backups/settings", {
        enabled: form.querySelector("[name=enabled]").checked,
        interval_hours: Number(v.interval_hours),
        keep_count: Number(v.keep_count),
        keep_days: Number(v.keep_days),
        target: b.target_fixed ? s.target : (v.target || "").trim() || null,
      });
      toast("Backup settings saved");
      route();
    } catch (e) {
      if (e.status !== 401) error.textContent = e.message;
    }
  });

  const runs = b.runs.length === 0 ? h("div", { class: "empty" }, "No backups have run yet.") : h("div", { class: "wide" }, h("table", {},
    h("thead", {}, h("tr", {}, h("th", {}, "Started"), h("th", {}, "Result"), h("th", {}, "By"), h("th", { class: "num" }, "Took"),
      h("th", { class: "num" }, "Files"), h("th", { class: "num" }, "Size"), h("th", { class: "num" }, "Written"))),
    h("tbody", {}, b.runs.map((r) => h("tr", {},
      h("td", { class: "nowrap", title: when(r.started_at) }, ago(r.started_at, now)),
      h("td", {}, r.ok ? h("span", { class: "pill ok" }, "Done") : [h("span", { class: "pill bad" }, "Failed"), " ", h("span", { class: "muted small" }, r.message)]),
      h("td", { class: "muted small nowrap" }, TRIGGERS[r.trigger] || r.trigger),
      h("td", { class: "num muted small" }, duration(r.finished_at - r.started_at) === "0 min" ? `${r.finished_at - r.started_at} s` : duration(r.finished_at - r.started_at)),
      h("td", { class: "num" }, r.ok ? r.files : "–"),
      h("td", { class: "num" }, r.ok ? bytes(r.bytes) : "–"),
      h("td", { class: "num", title: "Copied; the rest is shared with the backup before" }, r.ok ? bytes(r.copied) : "–"))))));

  const stored = b.stored.length === 0 ? h("div", { class: "empty" }, "None in the folder.") : h("div", { class: "wide" }, h("table", {},
    h("thead", {}, h("tr", {}, h("th", {}, "Backup"), h("th", {}, "Made"), h("th", { class: "num" }, "Size"), h("th", {}, "Vaults"))),
    h("tbody", {}, b.stored.map((x) => h("tr", {},
      h("td", { class: "path" }, x.name),
      h("td", { class: "nowrap muted small", title: when(x.created_at) }, when(x.created_at)),
      h("td", { class: "num" }, bytes(x.bytes)),
      h("td", { class: "muted small" }, x.vaults.join(", ")))))));

  refreshTimer = setInterval(() => { if (!document.hidden && !document.querySelector("dialog[open]") && !form.contains(document.activeElement)) route(); }, b.running ? 2000 : 30000);
  return [
    h("div", { class: "head" }, h("h1", { class: "grow" }, "Backups"),
      b.running && h("span", { class: "muted small" }, h("span", { class: "spinner" }), "Backing up…"),
      h("button", { class: "primary", disabled: b.running || !b.target, onclick: runNow }, "Back up now")),
    warnings.length > 0 && h("div", { class: "notes" }, warnings.map((w) => h("div", { class: "note" }, w))),
    h("div", { class: "cards" },
      card(b.last_ok_at ? ago(b.last_ok_at, now) : "–", "Last good backup", last ? (last.ok ? "the latest run worked" : h("span", { class: "bad" }, "the latest run failed")) : "none yet"),
      card(b.next_at ? inHours(b.next_at, now) : "off", "Next backup", s.enabled ? intervalText(s.interval_hours) : "only by hand"),
      card(b.runs_total ? `${Math.round((b.runs_ok / b.runs_total) * 100)} %` : "–", "Successful", `${b.runs_ok} of ${b.runs_total} run${b.runs_total === 1 ? "" : "s"}${failed ? ` · ${failed} failed` : ""}`),
      card(b.stored.length, "Kept", b.stored[0] ? `newest ${bytes(b.stored[0].bytes)}` : `up to ${s.keep_count}`)),
    h("div", { class: "grid" },
      h("div", { class: "panel" }, h("h2", {}, "Settings"), form),
      h("div", { class: "panel" }, h("h2", {}, "Restoring"),
        h("p", { class: "prose" }, "Each backup is a complete data folder: users, access and every vault with all its versions."),
        h("ol", { class: "prose" },
          h("li", {}, "Stop the server."),
          h("li", {}, "Move the data folder aside, and copy the backup's folder in its place."),
          h("li", {}, "Start the server again. Workspaces catch up on their next refresh; changes made since the backup are still in them and can be checked in again.")),
        h("p", { class: "muted small" }, "The data folder: ", h("span", { class: "path" }, b.data)))),
    h("div", { class: "panel" }, h("h2", {}, "Recent runs"), runs),
    h("div", { class: "panel" }, h("h2", {}, "In the backup folder"), stored),
  ];
}

route();
