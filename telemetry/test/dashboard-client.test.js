import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { DASHBOARD_BOOT, DASHBOARD_SCRIPT } from '../src/dashboard-client.js';

// A deliberately small DOM: selectors must match the server-rendered contract.
// In particular, the SVG itself owns .world-map; there is no .world-map wrapper.
class Element {
  constructor(tagName = 'div', dataset = {}) {
    this.tagName = tagName;
    this.dataset = dataset;
    this.events = {};
    this.attributes = {};
    this.textContent = '';
    this.classes = new Set();
    this.selectors = new Map();
    this.classList = {
      add: (name) => this.classes.add(name),
      toggle: (name, enabled) => enabled ? this.classes.add(name) : this.classes.delete(name),
    };
  }
  addEventListener(name, handler) { (this.events[name] ||= []).push(handler); }
  emit(name, event = {}) { for (const handler of this.events[name] || []) handler(event); }
  setAttribute(name, value) { this.attributes[name] = value; }
  getAttribute(name) { return this.attributes[name] ?? null; }
  removeAttribute(name) { delete this.attributes[name]; }
  querySelector(selector) { return this.selectors.get(selector) || null; }
  querySelectorAll(selector) { return this.selectors.get(selector) || []; }
  focus() { this.focused = true; this.emit('focus'); }
}

function fixture({ empty = false, storageBlocked = false, hash = '', observer = false } = {}) {
  const root = new Element('html');
  const ids = new Map();
  const queries = new Map();
  const create = (id, tagName = 'div') => {
    const element = new Element(tagName);
    ids.set(id, element);
    return element;
  };
  const theme = create('theme-picker', 'select');
  const refresh = create('refresh-dashboard', 'button');
  const chart = create('activity-chart');
  const chartReadout = create('chart-readout', 'output');
  const countryReadout = create('country-readout', 'output');
  countryReadout.textContent = 'Select a country';
  const clear = create('clear-country', 'button');
  const zoomIn = create('map-zoom-in', 'button');
  const zoomOut = create('map-zoom-out', 'button');
  const zoomReset = create('map-zoom-reset', 'button');
  const metricLabel = new Element('span');
  const points = ['2026-09-08', '2026-09-09', '2026-09-10'].map((day, index) => {
    const item = new Element('li', { day, users: String(index * 2), new: String(index) });
    const button = new Element('button');
    item.selectors.set('button.chart-hit', button);
    return { item, button };
  });
  chart.selectors.set('ol.bars > li[data-day]', points.map(({ item }) => item));
  chart.selectors.set('.metric-label', metricLabel);
  const views = ['bars', 'line'].map((chartView) => new Element('button', { chartView }));
  const series = ['users', 'new'].map((metric) => new Element('button', { series: metric }));
  queries.set('button[data-chart-view]', views);
  queries.set('button[data-series]', series);
  const marker = new Element('g', { country: 'PH', name: 'Philippines', users: '5' });
  marker.attributes.transform = 'translate(810 285)';
  const country = new Element('button', { ...marker.dataset });
  const outline = new Element('path', { country: 'PH' });
  queries.set('.map-marker[data-country][data-name][data-users], button.country-select[data-country][data-name][data-users]', [marker, country]);
  queries.set('.map-country[data-country]', [outline]);
  const map = new Element('svg');
  map.attributes.viewBox = '0 0 960 460';
  queries.set('svg.world-map', map);
  const heading = new Element();
  queries.set('.page-heading', heading);
  const navigation = ['overview', 'activity', 'geography', 'environment', 'daily-data'].map((id) => {
    const link = new Element('a');
    link.attributes.href = '#' + id;
    create(id, id === 'daily-data' ? 'details' : 'section');
    return link;
  });
  const viewData = new Element('a');
  viewData.attributes.href = '#daily-data';
  queries.set('.main-nav a[href^="#"]', navigation);
  queries.set('a[href^="#"]', [...navigation, viewData]);
  const document = {
    documentElement: root,
    getElementById: (id) => empty ? null : ids.get(id) || null,
    querySelector: (selector) => empty ? null : queries.get(selector) || null,
    querySelectorAll: (selector) => empty ? [] : queries.get(selector) || [],
  };
  const saved = {};
  const location = { hash, reload: () => { location.refreshed = true; } };
  const window = new Element();
  const observed = [];
  let intersect;
  const context = vm.createContext({
    document, location, window, Intl,
    localStorage: {
      getItem: () => { if (storageBlocked) throw new Error('denied'); return 'latte'; },
      setItem: (key, value) => { if (storageBlocked) throw new Error('denied'); saved[key] = value; },
    },
    ...(observer ? {
      IntersectionObserver: class {
        constructor(callback) { intersect = callback; }
        observe(element) { observed.push(element); }
      },
    } : {}),
  });
  vm.runInContext(DASHBOARD_BOOT, context);
  vm.runInContext(DASHBOARD_SCRIPT, context);
  return { root, ids, theme, refresh, chart, points, chartReadout, metricLabel, views, series, marker, country, outline, countryReadout, clear, map, zoomIn, zoomOut, zoomReset, navigation, viewData, heading, location, window, saved, observed, intersect };
}

test('theme selection persists, rejects unsupported values, and refresh is explicit', () => {
  const page = fixture();
  assert.equal(page.root.dataset.theme, 'latte');
  assert.equal(page.theme.value, 'latte');
  assert(page.root.classes.has('js'));
  assert.equal(page.location.refreshed, undefined);
  page.theme.value = 'nord';
  page.theme.emit('change');
  assert.equal(page.root.dataset.theme, 'nord');
  assert.equal(page.saved['alter-zero-telemetry-theme'], 'nord');
  page.theme.value = '<script>';
  page.theme.emit('change');
  assert.equal(page.root.dataset.theme, 'system');
  page.refresh.emit('click');
  assert.equal(page.location.refreshed, true);
});

test('blocked storage and an empty dashboard still enhance safely', () => {
  const page = fixture({ storageBlocked: true });
  assert.equal(page.root.dataset.theme, 'system');
  page.theme.value = 'dracula';
  page.theme.emit('change');
  assert.equal(page.root.dataset.theme, 'dracula');
  assert(fixture({ empty: true }).root.classes.has('js'));
});

test('chart keyboard navigation keeps one tab stop and reads the inspected day', () => {
  const { points, chartReadout } = fixture();
  const stops = () => points.filter(({ button }) => button.tabIndex === 0);
  assert.equal(stops().length, 1);
  assert.equal(stops()[0], points[2]);
  let prevented = 0;
  const key = (index, value) => points[index].button.emit('keydown', { key: value, preventDefault: () => { prevented++; } });
  key(2, 'Home');
  assert.equal(stops()[0], points[0]);
  assert(points[0].button.focused);
  key(0, 'ArrowLeft');
  assert.equal(stops()[0], points[0]);
  key(0, 'ArrowRight');
  assert.equal(stops()[0], points[1]);
  assert.match(chartReadout.textContent, /2 active · 1 new$/);
  key(1, 'End');
  assert.equal(stops()[0], points[2]);
  key(2, 'ArrowRight');
  assert.equal(stops()[0], points[2]);
  key(2, 'Tab');
  assert.equal(prevented, 5);
  points[0].button.emit('mouseenter');
  assert.match(chartReadout.textContent, /0 active · 0 new$/);
  assert.equal(stops().length, 1);
});

test('chart views and series update their pressed states and metric legend', () => {
  const { chart, views, series, metricLabel } = fixture();
  views[1].emit('click');
  assert.equal(chart.dataset.view, 'line');
  assert.equal(views[1].getAttribute('aria-pressed'), 'true');
  assert.equal(views[0].getAttribute('aria-pressed'), 'false');
  series[1].emit('click');
  assert.equal(chart.dataset.metric, 'new');
  assert.equal(metricLabel.textContent, 'New installs');
  assert.equal(series[1].getAttribute('aria-pressed'), 'true');
  series[0].emit('click');
  assert.equal(metricLabel.textContent, 'Daily active users');
});

test('country selection synchronizes markers, rows, outlines, and accessible readout', () => {
  const { marker, country, outline, countryReadout, clear } = fixture();
  marker.emit('mouseenter');
  assert.equal(countryReadout.textContent, 'Philippines · 5 active installs');
  marker.emit('mouseleave');
  assert.equal(countryReadout.textContent, 'Select a country');
  marker.emit('keydown', { key: 'Enter', preventDefault() {} });
  assert.equal(marker.getAttribute('aria-pressed'), 'true');
  assert.equal(country.getAttribute('aria-pressed'), 'true');
  assert(outline.classes.has('is-selected'));
  assert.equal(clear.hidden, false);
  marker.emit('mouseleave');
  assert.equal(countryReadout.textContent, 'Philippines · 5 active installs');
  marker.emit('keydown', { key: ' ', preventDefault() {} });
  assert.equal(country.getAttribute('aria-pressed'), 'false');
  country.emit('click');
  assert.equal(marker.getAttribute('aria-pressed'), 'true');
  clear.emit('click');
  assert.equal(marker.getAttribute('aria-pressed'), 'false');
  assert.equal(clear.hidden, true);
  assert.equal(countryReadout.textContent, 'Select a country');
});

test('map zoom uses the rendered SVG, clamps its extent, pans to focused countries and resets', () => {
  const { map, marker, country, zoomIn, zoomOut, zoomReset } = fixture();
  assert.equal(zoomOut.disabled, true);
  assert.equal(zoomReset.textContent, '1×');
  for (let i = 0; i < 12; i++) zoomIn.emit('click');
  assert.equal(zoomIn.disabled, true);
  assert.equal(zoomReset.textContent, '2.5×');
  let bounds = map.getAttribute('viewBox').split(' ').map(Number);
  assert.equal(bounds[2], 384);
  marker.focus();
  bounds = map.getAttribute('viewBox').split(' ').map(Number);
  assert(bounds[0] <= 810 && bounds[0] + bounds[2] >= 810);
  assert(bounds[1] <= 285 && bounds[1] + bounds[3] >= 285);
  assert(bounds[0] >= 0 && bounds[0] + bounds[2] <= 960);
  zoomReset.emit('click');
  assert.equal(map.getAttribute('viewBox'), '0 0 960 460');
  assert.equal(zoomReset.textContent, '1×');
  country.emit('click');
  zoomIn.emit('click');
  assert.notEqual(map.getAttribute('viewBox'), '96 46 768 368');
  for (let i = 0; i < 12; i++) zoomOut.emit('click');
  assert.equal(map.getAttribute('viewBox'), '0 0 960 460');
  assert.equal(zoomOut.disabled, true);
});

test('navigation and chart links open daily data, and scroll observers track the current section', () => {
  const page = fixture({ observer: true });
  page.navigation[2].emit('click');
  assert.equal(page.navigation[2].getAttribute('aria-current'), 'location');
  assert.equal(page.navigation[0].getAttribute('aria-current'), null);
  page.viewData.emit('click');
  assert.equal(page.ids.get('daily-data').open, true);
  assert(page.navigation[4].classes.has('active'));
  assert(page.observed.includes(page.heading), 'overview uses its heading, not the entire workspace');
  page.intersect([{ target: page.ids.get('environment'), isIntersecting: true, boundingClientRect: { top: 50 } }]);
  assert.equal(page.navigation[3].getAttribute('aria-current'), 'location');
  page.location.hash = '#activity';
  page.window.emit('hashchange');
  assert.equal(page.navigation[1].getAttribute('aria-current'), 'location');
  assert.equal(fixture({ hash: '#daily-data' }).ids.get('daily-data').open, true);
});
