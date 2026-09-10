// These scripts contain application code only. Dashboard values are supplied
// through escaped HTML attributes and read as text, never embedded in JavaScript.
export const DASHBOARD_BOOT = String.raw`
(() => {
  const themes = ['system', 'mocha', 'macchiato', 'frappe', 'latte', 'nord', 'dracula'];
  let theme = 'system';
  try {
    const saved = localStorage.getItem('alter-zero-telemetry-theme');
    if (themes.includes(saved)) theme = saved;
  } catch {}
  document.documentElement.dataset.theme = theme;
})();
`;

export const DASHBOARD_SCRIPT = String.raw`
(() => {
  const root = document.documentElement;
  const themes = ['system', 'mocha', 'macchiato', 'frappe', 'latte', 'nord', 'dracula'];
  const number = new Intl.NumberFormat();
  const date = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric', timeZone: 'UTC' });
  const count = (value) => {
    const parsed = Number(value);
    return Number.isFinite(parsed) && parsed >= 0 ? parsed : 0;
  };
  const themePicker = document.getElementById('theme-picker');
  if (themePicker) {
    themePicker.value = themes.includes(root.dataset.theme) ? root.dataset.theme : 'system';
    themePicker.addEventListener('change', () => {
      const theme = themes.includes(themePicker.value) ? themePicker.value : 'system';
      root.dataset.theme = theme;
      themePicker.value = theme;
      try { localStorage.setItem('alter-zero-telemetry-theme', theme); } catch {}
    });
  }

  const refresh = document.getElementById('refresh-dashboard');
  if (refresh) refresh.addEventListener('click', () => location.reload());

  const chart = document.getElementById('activity-chart');
  if (chart) {
    const readout = document.getElementById('chart-readout');
    const points = Array.from(chart.querySelectorAll('ol.bars > li[data-day]'))
      .map((item) => ({ item, button: item.querySelector('button.chart-hit') }))
      .filter((point) => point.button);
    let activeIndex = points.length - 1;
    const inspect = (index) => {
      if (!points[index]) return;
      activeIndex = index;
      points.forEach((point, i) => {
        point.button.tabIndex = i === index ? 0 : -1;
        point.item.classList.toggle('is-active', i === index);
      });
      if (readout) {
        const data = points[index].item.dataset;
        const day = new Date(data.day + 'T00:00:00Z');
        const label = Number.isNaN(day.getTime()) ? data.day : date.format(day);
        readout.textContent = label + ' · ' + number.format(count(data.users)) + ' active · ' + number.format(count(data.new)) + ' new';
      }
    };
    points.forEach((point, index) => {
      point.button.addEventListener('focus', () => inspect(index));
      point.button.addEventListener('mouseenter', () => inspect(index));
      point.button.addEventListener('click', () => inspect(index));
      point.button.addEventListener('keydown', (event) => {
        let next = activeIndex;
        if (event.key === 'ArrowLeft') next = Math.max(0, index - 1);
        else if (event.key === 'ArrowRight') next = Math.min(points.length - 1, index + 1);
        else if (event.key === 'Home') next = 0;
        else if (event.key === 'End') next = points.length - 1;
        else return;
        event.preventDefault();
        inspect(next);
        points[next].button.focus();
      });
    });
    inspect(activeIndex);

    const views = Array.from(document.querySelectorAll('button[data-chart-view]'));
    const setView = (view) => {
      if (view !== 'bars' && view !== 'line') return;
      chart.dataset.view = view;
      views.forEach((button) => button.setAttribute('aria-pressed', String(button.dataset.chartView === view)));
    };
    views.forEach((button) => button.addEventListener('click', () => setView(button.dataset.chartView)));
    setView(chart.dataset.view === 'line' ? 'line' : 'bars');

    const series = Array.from(document.querySelectorAll('button[data-series]'));
    const metricLabel = chart.querySelector('.metric-label');
    const setMetric = (metric) => {
      if (metric !== 'users' && metric !== 'new') return;
      chart.dataset.metric = metric;
      if (metricLabel) metricLabel.textContent = metric === 'new' ? 'New installs' : 'Daily active users';
      series.forEach((button) => button.setAttribute('aria-pressed', String(button.dataset.series === metric)));
    };
    series.forEach((button) => button.addEventListener('click', () => setMetric(button.dataset.series)));
    setMetric(chart.dataset.metric === 'new' ? 'new' : 'users');
  }

  const countryReadout = document.getElementById('country-readout');
  const initialCountryReadout = countryReadout ? countryReadout.textContent : '';
  const clearCountry = document.getElementById('clear-country');
  const countries = Array.from(document.querySelectorAll('.map-marker[data-country][data-name][data-users], button.country-select[data-country][data-name][data-users]'));
  const outlines = Array.from(document.querySelectorAll('.map-country[data-country]'));
  let selectedCountry = null;
  let focusMapCountry = () => {};
  const describeCountry = (element) => {
    if (!countryReadout) return;
    if (!element) {
      countryReadout.textContent = initialCountryReadout;
      return;
    }
    const users = count(element.dataset.users);
    countryReadout.textContent = element.dataset.name + ' · ' + number.format(users) + ' active install' + (users === 1 ? '' : 's');
  };
  const restoreCountry = () => describeCountry(countries.find((element) => element.dataset.country === selectedCountry));
  const selectCountry = (country) => {
    selectedCountry = country;
    countries.forEach((element) => {
      const selected = country !== null && element.dataset.country === country;
      element.classList.toggle('is-selected', selected);
      element.setAttribute('aria-pressed', String(selected));
    });
    outlines.forEach((element) => element.classList.toggle('is-selected', country !== null && element.dataset.country === country));
    if (clearCountry) clearCountry.hidden = country === null;
    focusMapCountry(country);
    restoreCountry();
  };
  countries.forEach((element) => {
    element.addEventListener('mouseenter', () => describeCountry(element));
    element.addEventListener('mouseleave', restoreCountry);
    element.addEventListener('focus', () => {
      describeCountry(element);
      focusMapCountry(element.dataset.country);
    });
    element.addEventListener('blur', restoreCountry);
    const toggle = () => selectCountry(selectedCountry === element.dataset.country ? null : element.dataset.country);
    element.addEventListener('click', toggle);
    if (element.tagName.toLowerCase() !== 'button') {
      element.setAttribute('tabindex', '0');
      element.setAttribute('role', 'button');
      element.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        toggle();
      });
    }
  });
  if (clearCountry) clearCountry.addEventListener('click', () => selectCountry(null));
  selectCountry(null);

  const map = document.querySelector('svg.world-map');
  const zoomIn = document.getElementById('map-zoom-in');
  const zoomOut = document.getElementById('map-zoom-out');
  const zoomReset = document.getElementById('map-zoom-reset');
  const zoomLabel = document.getElementById('map-zoom-level');
  if (map) {
    const bounds = (map.getAttribute('viewBox') || '').trim().split(/[\s,]+/).map(Number);
    if (bounds.length === 4 && bounds.every(Number.isFinite) && bounds[2] > 0 && bounds[3] > 0) {
      let zoom = 1;
      let centerX = bounds[0] + bounds[2] / 2;
      let centerY = bounds[1] + bounds[3] / 2;
      const updateZoom = (value) => {
        zoom = Math.min(2.5, Math.max(1, value));
        const width = bounds[2] / zoom;
        const height = bounds[3] / zoom;
        const x = Math.max(bounds[0], Math.min(bounds[0] + bounds[2] - width, centerX - width / 2));
        const y = Math.max(bounds[1], Math.min(bounds[1] + bounds[3] - height, centerY - height / 2));
        map.setAttribute('viewBox', [x, y, width, height].join(' '));
        if (zoomIn) zoomIn.disabled = zoom >= 2.5;
        if (zoomOut) zoomOut.disabled = zoom <= 1;
        if (zoomReset) {
          zoomReset.disabled = zoom === 1;
          zoomReset.textContent = number.format(zoom) + '×';
        }
        if (zoomLabel) zoomLabel.textContent = Math.round(zoom * 100) + '%';
      };
      focusMapCountry = (country) => {
        if (country === null) {
          centerX = bounds[0] + bounds[2] / 2;
          centerY = bounds[1] + bounds[3] / 2;
        } else {
          const marker = countries.find((element) => element.dataset.country === country && element.tagName.toLowerCase() !== 'button');
          const position = marker && (marker.getAttribute('transform') || '').match(/^translate\(\s*([-\d.]+)[,\s]+([-\d.]+)\s*\)$/);
          if (!position || !Number.isFinite(Number(position[1])) || !Number.isFinite(Number(position[2]))) return;
          centerX = Number(position[1]);
          centerY = Number(position[2]);
        }
        updateZoom(zoom);
      };
      if (zoomIn) zoomIn.addEventListener('click', () => updateZoom(zoom + 0.25));
      if (zoomOut) zoomOut.addEventListener('click', () => updateZoom(zoom - 0.25));
      if (zoomReset) zoomReset.addEventListener('click', () => {
        centerX = bounds[0] + bounds[2] / 2;
        centerY = bounds[1] + bounds[3] / 2;
        updateZoom(1);
      });
      updateZoom(1);
    }
  }

  const navigation = Array.from(document.querySelectorAll('.main-nav a[href^="#"]'));
  const setActiveSection = (id) => {
    if (!navigation.some((link) => link.getAttribute('href') === '#' + id)) return;
    navigation.forEach((link) => {
      const active = link.getAttribute('href') === '#' + id;
      link.classList.toggle('active', active);
      if (active) link.setAttribute('aria-current', 'location');
      else link.removeAttribute('aria-current');
    });
  };
  const openSection = (hash) => {
    if (!hash || hash[0] !== '#') return;
    const id = hash.slice(1);
    const section = document.getElementById(id);
    if (section && section.tagName.toLowerCase() === 'details') section.open = true;
    setActiveSection(id);
  };
  document.querySelectorAll('a[href^="#"]').forEach((link) => {
    link.addEventListener('click', () => openSection(link.getAttribute('href')));
  });
  if (typeof location !== 'undefined') openSection(location.hash);
  if (typeof window !== 'undefined') window.addEventListener('hashchange', () => openSection(location.hash));
  if (typeof IntersectionObserver !== 'undefined' && navigation.length) {
    const sectionIds = new Map();
    const visibleSections = new Map();
    const observer = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) visibleSections.set(entry.target, entry);
        else visibleSections.delete(entry.target);
      });
      const nearest = Array.from(visibleSections.values()).sort((a, b) => Math.abs(a.boundingClientRect.top) - Math.abs(b.boundingClientRect.top))[0];
      if (nearest) setActiveSection(sectionIds.get(nearest.target));
    }, { rootMargin: '-8% 0px -55% 0px', threshold: 0 });
    navigation.forEach((link) => {
      const id = link.getAttribute('href').slice(1);
      const section = id === 'overview' ? document.querySelector('.page-heading') || document.getElementById(id) : document.getElementById(id);
      if (section) {
        sectionIds.set(section, id);
        observer.observe(section);
      }
    });
  }

  root.classList.add('js');
})();
`;
