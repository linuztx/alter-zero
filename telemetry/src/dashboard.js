// Server-rendered telemetry. Enhancements read escaped DOM attributes, never
// interpolate telemetry or authentication values into executable JavaScript.
import { countryLabel, dashboardHref, escapeHtml, windowDays } from './lib.js';
import { renderWorldMap } from './world-map.js';
import { DASHBOARD_BOOT, DASHBOARD_SCRIPT } from './dashboard-client.js';
import { DASHBOARD_STYLE } from './dashboard-style.js';

const WINDOWS = [7, 30, 90, 365];
const THEMES = [['system', 'System'], ['mocha', 'Catppuccin Mocha'], ['macchiato', 'Catppuccin Macchiato'], ['frappe', 'Catppuccin Frappé'], ['latte', 'Catppuccin Latte'], ['nord', 'Nord'], ['dracula', 'Dracula']];
const rows = (value) => Array.isArray(value) ? value.filter((row) => row && typeof row === 'object') : [];
const count = (value) => Number.isFinite(Number(value)) ? Math.max(0, Math.trunc(Number(value))) : 0;
const number = (value) => count(value).toLocaleString('en-US');
const share = (part, total) => total > 0 ? Math.min(100, count(part) / total * 100).toFixed(2) : '0';
const safe = (value) => escapeHtml(value ?? '');

const ICONS = {
  terminal: '<path d="m5 7 5 5-5 5m8 0h6"/>',
  overview: '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  activity: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
  globe: '<circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18"/>',
  system: '<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8m-4-4v4"/>',
  table: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M9 9v12"/>',
  shield: '<path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Z"/><path d="m8 12 3 3 5-6"/>',
  refresh: '<path d="M20 7v5h-5M4 17v-5h5"/><path d="M6 6a8 8 0 0 1 13 3M5 15a8 8 0 0 0 13 3"/>',
  code: '<path d="m8 7-5 5 5 5m8-10 5 5-5 5M14 4l-4 16"/>',
  chevron: '<path d="m9 5 7 7-7 7"/>',
  calendar: '<rect x="3" y="5" width="18" height="16" rx="2"/><path d="M7 3v4m10-4v4M3 11h18"/>',
  palette: '<circle cx="8" cy="9" r=".5"/><circle cx="12" cy="6" r=".5"/><circle cx="17" cy="9" r=".5"/><path d="M12 3a9 9 0 1 0 0 18h1a2 2 0 0 0 0-4h-1a2 2 0 0 1 0-4h6a3 3 0 0 0 3-3c0-4-5-7-9-7Z"/>',
  users: '<circle cx="9" cy="8" r="3"/><path d="M3 21v-3a6 6 0 0 1 12 0v3m1-16a3 3 0 0 1 0 6m2 4a5 5 0 0 1 3 5"/>',
  box: '<path d="m12 3 9 5v9l-9 5-9-5V8l9-5ZM3 8l9 5 9-5m-9 5v9M7.5 5.5l9 5v4"/>',
};
function icon(name, className = '') {
  return `<svg class="icon ${className}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">${ICONS[name] ?? ICONS.terminal}</svg>`;
}
function shortDay(raw) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(raw ?? ''));
  return match ? `${['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'][Number(match[2]) - 1] ?? match[2]} ${Number(match[3])}` : String(raw ?? '');
}
function instant(raw) {
  const match = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})/.exec(String(raw ?? ''));
  return match ? `${match[1]} ${match[2]} UTC` : String(raw ?? 'Unknown');
}
function titleBlock(title, description, extra = '') {
  return `<div class="card-heading"><div><h2>${title}</h2><p>${description}</p></div>${extra}</div>`;
}
function kpi(label, value, note, symbol, tone = '') {
  return `<article class="kpi ${tone}"><div class="kpi-top"><span>${label}</span>${icon(symbol)}</div><b>${number(value)}</b><div class="kpi-note">${note}</div></article>`;
}

function activityChart(daily, view) {
  const peak = daily.reduce((max, day) => Math.max(max, count(day.users), count(day.new_installs)), 0);
  if (!peak) return '<div class="chart-empty">' + icon('activity') + '<strong>No pings in this window yet.</strong><span>Activity will appear after an install checks in.</span></div>';
  const ceiling = Math.max(1, Math.ceil(peak / 4) * 4);
  const bars = daily.map((day, index) => {
    const users = count(day.users), fresh = count(day.new_installs);
    const description = `${day.day ?? ''} · ${number(users)} users · ${number(fresh)} new installs`;
    return `<li class="bar-slot" style="--h:${share(users, ceiling)}%;--n:${share(fresh, ceiling)}%" data-day="${safe(day.day)}" data-users="${users}" data-new="${fresh}"><span class="bar-users"></span><span class="bar-new"></span><button class="chart-hit" type="button" tabindex="${index === daily.length - 1 ? 0 : -1}" aria-label="${safe(description)}" title="${safe(description)}"></button></li>`;
  }).join('');
  const x = (index) => ((index + .5) / daily.length * 1000).toFixed(2);
  const points = (field) => daily.map((day, index) => `${x(index)},${(200 - count(day[field]) / ceiling * 200).toFixed(2)}`).join(' ');
  const line = (field, type) => `<g class="line-${type}"><polygon class="line-area" points="${x(0)},200 ${points(field)} ${x(daily.length - 1)},200"/><polyline points="${points(field)}"/>${daily.length === 1 ? `<circle cx="500" cy="${200 - count(daily[0][field]) / ceiling * 200}" r="4"/>` : ''}</g>`;
  const latest = daily[daily.length - 1];
  return `<div id="activity-chart" data-view="bars" data-metric="users" style="--day-count:${daily.length}">
    <div class="chart-meta"><span class="legend-item"><i></i><span class="metric-label">Daily active users</span></span><output id="chart-readout" aria-live="polite">${safe(shortDay(latest.day))} · ${number(latest.users)} users · ${number(latest.new_installs)} new</output></div>
    <div class="chart-plot"><div class="chart-scale" aria-hidden="true">${[4,3,2,1,0].map((n) => `<span>${number(ceiling * n / 4)}</span>`).join('')}</div><div class="plot-inner">
      <div class="chart-grid" aria-hidden="true">${'<span></span>'.repeat(5)}</div>
      <svg class="line-chart" viewBox="0 0 1000 200" preserveAspectRatio="none" aria-hidden="true">${line('users', 'users')}${line('new_installs', 'new')}</svg>
      <ol class="bars" aria-label="Daily activity from ${safe(view.since)} to ${safe(view.until)}">${bars}</ol>
    </div></div>
    <div class="chart-axis"><span>${safe(shortDay(daily[0].day))}</span><span>${daily.length > 4 ? safe(shortDay(daily[Math.floor((daily.length - 1) / 2)].day)) : ''}</span><span>${daily.length > 1 ? safe(shortDay(latest.day)) : ''}</span></div>
    <div class="chart-bottom"><span>One check-in per install, per UTC day</span><a href="#daily-data">View daily data ${icon('chevron')}</a></div>
  </div>`;
}

function countryCell(row) {
  const label = countryLabel(row.country);
  return `<button type="button" class="country-select" data-country="${safe(label.code)}" data-name="${safe(label.name)}" data-users="${count(row.users)}" aria-pressed="false"><span class="flag" aria-hidden="true">${safe(label.flag)}</span><span class="country-name">${safe(label.name)}</span><span class="code">${safe(label.code)}</span></button>`;
}
function plainCell(value) { return `<span class="txt">${safe(String(value ?? '').trim()) || '<span class="muted">(blank)</span>'}</span>`; }
function platformCell(row) { return `${plainCell(row.platform)}${row.version ? `<span class="code">${safe(row.version)}</span>` : ''}`; }

function tableRows(values, decorate, peak) {
  return values.map((row) => `<tr><th scope="row"><span class="name">${decorate(row)}</span></th><td class="n">${number(row.users)}</td><td class="share"><span class="meter" aria-hidden="true"><span style="width:${share(row.users, peak)}%"></span></span></td></tr>`).join('');
}
function breakdownTable(values, heading, decorate, limit = 6) {
  const peak = values.reduce((max, row) => Math.max(max, count(row.users)), 0);
  const head = `<thead><tr><th scope="col">${heading}</th><th scope="col" class="n">Users</th><th scope="col" class="share"><span class="sr-only">Relative count</span></th></tr></thead>`;
  const table = (content) => `<table>${head}<tbody>${content}</tbody></table>`;
  return table(values.length ? tableRows(values.slice(0, limit), decorate, peak) : '<tr><td colspan="3" class="empty-cell">No activity recorded yet.</td></tr>') + (values.length > limit ? `<details class="more-rows"><summary>+ ${number(values.length - limit)} more ${icon('chevron')}</summary>${table(tableRows(values.slice(limit), decorate, peak))}</details>` : '');
}

function osPanel(values) {
  const total = values.reduce((sum, row) => sum + count(row.users), 0);
  const colors = ['var(--accent)', 'var(--purple)', 'var(--peach)', 'var(--good)', 'var(--blue)'];
  let offset = 0;
  const stops = values.map((row, index) => {
    const start = offset;
    offset += total ? count(row.users) / total * 100 : 0;
    return `${colors[index % colors.length]} ${start.toFixed(3)}% ${offset.toFixed(3)}%`;
  });
  return `<section class="card os-card">${titleBlock('Operating systems', 'Platform mix in this window')}
    <div class="os-content"><div class="os-ring" style="--segments:${total ? `conic-gradient(${stops.join(',')})` : 'var(--surface-2)'}" aria-hidden="true"><div>${icon('system')}<b>${number(values.filter((row) => count(row.users) > 0).length)}</b><span>systems</span></div></div>
    <div class="os-list">${values.length ? values.map((row, index) => `<div class="os-row"><span><i style="background:${colors[index % colors.length]}"></i>${safe(row.os)}</span><b>${number(row.users)}</b></div>`).join('') : '<p class="muted">No systems reported yet.</p>'}</div></div>
    <p class="card-note">An install may appear in more than one system.</p></section>`;
}

/** Pure renderer, including graceful fallbacks for a partial stats document. */
export function renderDashboard(stats = {}, options = {}) {
  const token = typeof options.token === 'string' && options.token ? options.token : null;
  const view = stats.window ?? {}, days = windowDays(view.days);
  const daily = rows(stats.daily), countries = rows(stats.countries), totals = stats.totals ?? {}, today = stats.today ?? {};
  const hasActivity = daily.some((day) => count(day.users) > 0 || count(day.new_installs) > 0);
  const yesterday = daily.length >= 2 ? count(daily[daily.length - 2].users) : null;
  const delta = yesterday === null ? null : count(today.users) - yesterday;
  const trend = delta === null ? '<span class="muted">Today, in UTC</span>' : `<span class="trend ${delta > 0 ? 'positive' : delta < 0 ? 'negative' : ''}">${delta > 0 ? '↗ +' : delta < 0 ? '↘ −' : '↔ '}${number(Math.abs(delta))}</span><span>vs. yesterday</span>`;
  const windows = [...new Set([...WINDOWS, days])].sort((a, b) => a - b).map((n) => `<a href="${safe(dashboardHref({ days: n, token }))}"${n === days ? ' aria-current="page"' : ''}>${n}d</a>`).join('');
  const json = safe(dashboardHref({ days, token, path: '/v1/stats' }));
  const datedRows = daily.slice().reverse().map((day) => `<tr><th scope="row">${safe(day.day)}</th><td class="n">${number(day.users)}</td><td class="n">${number(day.new_installs)}</td></tr>`).join('');
  const countryCount = countries.filter((row) => count(row.users) > 0 && countryLabel(row.country).name !== 'Unknown' && countryLabel(row.country).flag !== countryLabel('ZZ').flag).length;
  return `<!doctype html>
<html lang="en" data-theme="system">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="robots" content="noindex, nofollow"><meta name="referrer" content="no-referrer">
<title>Telemetry · Alter Zero</title><link rel="icon" href="data:,"><script id="theme-init">${DASHBOARD_BOOT}</script><style>${DASHBOARD_STYLE}</style></head>
<body><a class="skip-link" href="#main">Skip to telemetry</a>
<aside class="sidebar"><a class="brand" href="#overview" aria-label="Alter Zero telemetry overview"><span class="brand-mark">${icon('terminal')}</span><span>Alter Zero<span class="brand-sub">TELEMETRY</span></span></a>
<div class="sidebar-section">WORKSPACE</div><nav class="main-nav" aria-label="Dashboard sections">
${[['overview','Overview','overview'],['activity','Activity','activity'],['geography','Geography','globe'],['environment','Environment','system'],['daily-data','Daily data','table']].map(([id,label,symbol], i) => `<a href="#${id}" aria-label="${label}" title="${label}"${i === 0 ? ' class="active" aria-current="location"' : ''}>${icon(symbol)}<span>${label}</span>${i === 0 ? '<span class="nav-dot"></span>' : ''}</a>`).join('')}
</nav><div class="sidebar-bottom"><div class="privacy-note">${icon('shield')}<div><strong>Anonymous by design</strong><p>Country-level insights.<br>No IP addresses stored.</p></div></div><div class="sidebar-signature"><span class="status-dot"></span>Built for the terminal<span class="mono">/_</span></div></div></aside>
<div class="workspace" id="overview"><header class="topbar"><div class="breadcrumb">Workspace ${icon('chevron')}<span>Telemetry</span></div><div class="topbar-tools"><span class="snapshot-label"><span class="status-dot"></span>Daily snapshots</span><a class="json-link" href="${json}" aria-label="View telemetry as JSON">${icon('code')}<span>JSON</span></a></div></header>
<main id="main"><div class="page-heading"><div><div class="eyebrow"><span></span>ALTER ZERO / INSIGHTS</div><h1>Telemetry<span class="heading-dot">.</span></h1><p>A pulse on the terminals running Alter Zero.</p></div><div class="heading-controls"><label class="theme-control enhanced">${icon('palette')}<span class="sr-only">Color theme</span><select id="theme-picker">${THEMES.map(([value,label]) => `<option value="${value}">${label}</option>`).join('')}</select></label><button class="icon-button enhanced" id="refresh-dashboard" type="button" title="Refresh telemetry" aria-label="Refresh telemetry">${icon('refresh')}</button></div></div>
<div class="window-toolbar"><div class="date-range">${icon('calendar')}<span>${safe(shortDay(view.since))}${view.since ? ' — ' : ''}${safe(shortDay(view.until))}</span><span class="utc-badge">UTC</span></div><nav class="window-picker" aria-label="Window">${windows}</nav></div>
<section class="kpis" aria-label="Telemetry summary">${kpi('Users today', today.users, trend, 'activity', 'kpi-highlight')}${kpi('Users · last 7 days', totals.users_7d, 'Distinct installs in the last week', 'users')}${kpi('Users · last 30 days', totals.users_30d, 'Distinct installs in the last month', 'calendar')}${kpi('Installs seen', totals.installs, 'Across all retained history', 'box')}</section>
<div class="activity-layout" id="activity"><section class="card activity-card">${titleBlock('Activity', `Daily check-ins over ${days} days`, `<div class="chart-views enhanced" role="group" aria-label="Chart style"${hasActivity ? '' : ' hidden'}><button type="button" data-chart-view="bars" aria-pressed="true">Bars</button><button type="button" data-chart-view="line" aria-pressed="false">Line</button></div>`)}<div class="series-switch enhanced" role="group" aria-label="Activity metric"${hasActivity ? '' : ' hidden'}><button type="button" data-series="users" aria-pressed="true"><i></i>Users</button><button type="button" data-series="new" aria-pressed="false"><i></i>New installs</button></div>${activityChart(daily, view)}</section>${osPanel(rows(stats.os))}</div>
<section class="card geography-card" id="geography">${titleBlock('Around the world', 'Where Alter Zero checks in', `<span class="count-badge">${icon('globe')}${number(countryCount)} ${countryCount === 1 ? 'country' : 'countries'}</span>`)}<div class="geography-layout"><div class="map-panel"><div class="map-toolbar"><span class="map-legend"><i></i>Country activity</span><div class="map-controls enhanced" role="group" aria-label="Map zoom"><button id="map-zoom-out" type="button" aria-label="Zoom out">−</button><button id="map-zoom-reset" type="button" aria-label="Reset map zoom">1×</button><button id="map-zoom-in" type="button" aria-label="Zoom in">+</button></div></div>${renderWorldMap(countries)}<div class="country-inspector"><span class="inspector-icon">${icon('globe')}</span><output id="country-readout" aria-live="polite">${countryCount ? 'Select a country to see its activity.' : 'No mapped country activity in this window.'}</output><button class="text-button enhanced" id="clear-country" type="button" hidden>Clear</button></div></div><div class="country-panel"><h3>Countries <span>${days} days</span></h3>${breakdownTable(countries, 'Country', countryCell)}<p class="card-note">Distinct installs per country. An install can appear in multiple countries.</p></div></div></section>
<div class="environment-layout" id="environment"><section class="card">${titleBlock('Versions', 'Release adoption in this window', `<span class="section-icon">${icon('code')}</span>`)}${breakdownTable(rows(stats.versions), 'Version', (row) => plainCell(row.version))}</section><section class="card">${titleBlock('Platforms', 'Distributions and system versions', `<span class="section-icon">${icon('system')}</span>`)}${breakdownTable(rows(stats.platforms), 'Platform', platformCell)}</section></div>
<details class="card daily-data" id="daily-data"><summary><span class="daily-title">${icon('table')}<span>Daily data<span>Every day in this window (${number(daily.length)})</span></span></span><span class="summary-end">View table ${icon('chevron')}</span></summary><div class="daily-scroll"><table><thead><tr><th scope="col">Day (UTC)</th><th scope="col" class="n">Users</th><th scope="col" class="n">New installs</th></tr></thead><tbody>${datedRows || '<tr><td colspan="3" class="empty-cell">No daily data yet.</td></tr>'}</tbody></table></div></details>
<footer><div class="footer-top"><span>${icon('terminal')}<strong>Alter Zero</strong><span class="footer-divider">/</span>Telemetry</span><span>Generated ${safe(instant(stats.generated_at))}</span></div><p>A user is an anonymous install ID. New installs are IDs first seen within retained history. Dates use the UTC day the ping arrived. No address is stored, logged, or derived into anything finer than the country.</p><a href="${json}">View the same numbers as JSON ${icon('chevron')}</a></footer>
</main></div><script id="dashboard-script">${DASHBOARD_SCRIPT}</script></body></html>`;
}
