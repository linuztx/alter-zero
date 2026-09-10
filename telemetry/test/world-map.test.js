import test from 'node:test';
import assert from 'node:assert/strict';
import { renderWorldMap } from '../src/world-map.js';
import { COUNTRY_ANCHORS, COUNTRY_OUTLINES } from '../src/world-map-data.js';

test('the map includes geographic outlines and fixed country anchors within its viewBox', () => {
  assert.ok(COUNTRY_OUTLINES.length > 170);
  assert.ok(COUNTRY_ANCHORS.length > 240);
  for (const [code, , x, y] of COUNTRY_ANCHORS) {
    assert.match(code, /^[A-Z]{2}$/);
    assert.ok(x >= 0 && x <= 960 && y >= 0 && y <= 460, `${code} must fit the map`);
  }
  const anchors = new Map(COUNTRY_ANCHORS.map(([code, , x, y]) => [code, [x, y]]));
  // Independent rough bounds catch swapped latitude/longitude and misplaced
  // small-country markers that the 1:110m country polygons would not include.
  for (const [code, [left, top, right, bottom]] of Object.entries({
    PH: [770, 185, 800, 220], US: [200, 100, 270, 160],
    SG: [733, 221, 746, 232], GB: [460, 70, 490, 110],
    GF: [340, 210, 360, 230], FR: [475, 105, 500, 125],
  })) {
    const [x, y] = anchors.get(code);
    assert.ok(x >= left && x <= right && y >= top && y <= bottom, `${code} geographic placement`);
  }
});

test('known countries have accessible interactive markers and inactive outlines stay subtle', () => {
  const html = renderWorldMap([{ country: 'PH', users: 42 }, { country: 'SG', users: 1 }]);
  assert.match(html, /viewBox="0 0 960 460"/);
  assert.match(html, /class="map-marker" role="button" tabindex="0" aria-pressed="false" aria-label="Philippines: 42 active installs" data-country="PH" data-name="Philippines" data-users="42"/);
  assert.match(html, /aria-label="Singapore: 1 active install"/);
  assert.match(html, /class="map-country is-active" data-country="PH"/);
  assert.match(html, /class="map-country" data-country="US"/);
  assert.match(html, /class="map-status" role="status" aria-live="polite"/);
  assert.match(html, /No precise locations/);
  assert.doesNotMatch(html, /<script|<image|(?:src|href)="https?:\/\//);
});

test('unknown and unmapped totals remain visible without invented positions or HTML injection', () => {
  const html = renderWorldMap([
    { country: 'ZZ', users: 8 },
    { country: '\"><script>alert(1)</script>', users: 2 },
    { country: 'XX', users: 1 },
    { country: 'QQ', users: 3 },
    { country: 'PH', users: Infinity },
    { country: 'US', users: -4 },
    null,
  ]);
  assert.match(html, /Unknown country: 11/);
  assert.match(html, /Unmapped regions: 3/);
  assert.match(html, /No mapped country activity in this window/);
  assert.doesNotMatch(html, /class="map-marker"|alert\(1\)|<script|NaN|Infinity/);
});

test('normalization merges duplicate country rows and supports an empty or absent dataset', () => {
  const html = renderWorldMap([{ country: 'ph', users: 4 }, { country: ' PH ', users: '6' }]);
  assert.equal((html.match(/class="map-marker"/g) ?? []).length, 1);
  assert.match(html, /data-country="PH" data-name="Philippines" data-users="10"/);
  assert.match(html, /data-mapped-countries="1"/);
  assert.match(renderWorldMap(), /No mapped country activity/);
  assert.match(renderWorldMap(null), /No mapped country activity/);
});
