import { COUNTRY_ANCHORS, COUNTRY_OUTLINES } from './world-map-data.js';

// Bundled Natural Earth geometry keeps the dashboard private and works offline.
// Label anchors are fixed per country; they never represent device positions.
// See world-map-data.md for the source, projection and simplification details.
const anchors = new Map(COUNTRY_ANCHORS.map(([code, name, x, y]) => [code, { name, x, y }]));
const numberFormat = new Intl.NumberFormat('en');
const regionNames = typeof Intl.DisplayNames === 'function'
  ? new Intl.DisplayNames(['en'], { type: 'region' })
  : null;

function escape(value) {
  return String(value).replace(/[&<>"']/g, (character) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[character]);
}

function countryName(code, fallback) {
  const name = regionNames?.of(code);
  return name && name !== code ? name : fallback;
}

function positiveCount(value) {
  const count = Number(value);
  return Number.isSafeInteger(count) && count > 0 ? count : 0;
}

/**
 * Render country-level telemetry as a self-contained SVG and accessible status.
 * `countries` is the stats array of { country, users }. No network is involved.
 * The dashboard binds click/Enter/Space handlers to `.map-marker`; each marker
 * provides data-country, data-name and data-users. Paths also have data-country
 * so a selection can highlight its outline. The initial viewBox is 0 0 960 460.
 */
export function renderWorldMap(countries = []) {
  const counts = new Map();
  let unknownUsers = 0;
  let unmappedUsers = 0;
  for (const row of Array.isArray(countries) ? countries : []) {
    if (!row || typeof row !== 'object') continue;
    const count = positiveCount(row.users);
    if (!count) continue;
    const code = typeof row.country === 'string' ? row.country.trim().toUpperCase() : '';
    if (!/^[A-Z]{2}$/.test(code) || code === 'ZZ' || code === 'XX') {
      unknownUsers += count;
    } else if (!anchors.has(code)) {
      unmappedUsers += count;
    } else {
      counts.set(code, (counts.get(code) ?? 0) + count);
    }
  }

  const maximum = Math.max(1, ...counts.values());
  const outlines = COUNTRY_OUTLINES.map(([code, path]) => {
    const active = counts.has(code);
    return `<path class="map-country${active ? ' is-active' : ''}" data-country="${code}" d="${path}" fill="var(${active ? '--accent' : '--surface-2'})"${active ? ' fill-opacity=".16"' : ''} stroke="var(--border)" stroke-width=".7" vector-effect="non-scaling-stroke"/>`;
  }).join('');

  // Draw smaller circles last so nearby large countries cannot cover them.
  const markers = [...counts].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).map(([code, users]) => {
    const anchor = anchors.get(code);
    const name = countryName(code, anchor.name);
    const radius = (3.5 + 4.5 * Math.sqrt(users / maximum)).toFixed(1);
    const halo = (Number(radius) + 5).toFixed(1);
    const label = `${name}: ${numberFormat.format(users)} active ${users === 1 ? 'install' : 'installs'}`;
    return `<g class="map-marker" role="button" tabindex="0" aria-pressed="false" aria-label="${escape(label)}" data-country="${code}" data-name="${escape(name)}" data-users="${users}" transform="translate(${anchor.x} ${anchor.y})">
      <title>${escape(label)}</title>
      <circle class="map-marker-hit" r="16" fill="transparent"/>
      <circle class="map-marker-halo" r="${halo}" fill="var(--accent)" fill-opacity=".15"/>
      <circle class="map-marker-dot" r="${radius}" fill="var(--accent)" stroke="var(--surface-2)" stroke-width="2" vector-effect="non-scaling-stroke"/>
    </g>`;
  }).join('');

  const status = counts.size
    ? 'Select a country to explore its activity.'
    : 'No mapped country activity in this window.';
  const missing = [
    unknownUsers ? `Unknown country: ${numberFormat.format(unknownUsers)}` : '',
    unmappedUsers ? `Unmapped regions: ${numberFormat.format(unmappedUsers)}` : '',
  ].filter(Boolean).join(' · ');

  return `<div class="world-map-wrap" data-mapped-countries="${counts.size}">
    <svg class="world-map" viewBox="0 0 960 460" xmlns="http://www.w3.org/2000/svg" role="group" aria-labelledby="world-map-title world-map-description">
      <title id="world-map-title">Activity around the world</title>
      <desc id="world-map-description">${counts.size} ${counts.size === 1 ? 'country' : 'countries'} with activity. Select a marked country to inspect its total. Markers show fixed country positions, never device locations.</desc>
      <g class="map-grid" fill="none" stroke="var(--border)" stroke-width=".65" opacity=".45" aria-hidden="true">
        <path d="M30 80H930M30 155H930M30 230H930M30 305H930M30 380H930M180 20V440M330 20V440M480 20V440M630 20V440M780 20V440"/>
      </g>
      <g class="map-land" fill-rule="evenodd" aria-hidden="true">${outlines}</g>
      <g class="map-markers">${markers}</g>
    </svg>
    <p class="map-status" role="status" aria-live="polite" data-default-status="${status}">${status}</p>
    <div class="map-footnote"><span>Country-level activity · No precise locations</span>${missing ? `<span class="map-unmapped">${missing}</span>` : ''}<span class="map-attribution">Made with Natural Earth</span></div>
  </div>`;
}
